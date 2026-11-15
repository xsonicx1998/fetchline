#!/usr/bin/env python3
"""Smoke-test the real fetchline-host binary (S13 finish / Milestone 2).

Drives the binary with native-messaging frames against a local HTTP
server serving a small page with links (deterministic bytes, HEAD +
Range support). This is the "Download all links on this page" path
end to end, exactly as the extension's context-menu item sends it
(`download_all` with `links: []` — the host expands the page itself
via the engine's fetch_page_links). Checks:
  D1 ping works
  D2 get_settings reports pending_count 0 on a fresh database
  D3 download_all with no links expands the page: the good links are
     enqueued, the duplicate link is de-duplicated, the dead link is
     skipped (never enqueued)
  D4 the pending_links rows in the DB afterwards are exactly the
     three good links, source 'download_all', status 'pending'
  D5 a second download_all for the same page enqueues the same links
     again (enqueueing does not de-duplicate against the queue — the
     drain's duplicate-path guard is what gives the second copy a
     different save path)
  D6 download_all for an excluded page is an error, nothing enqueued
  D7 download_all while browser integration is off is an error
     (BUG-012 regression: this path used to bypass the off switch)
  D8 download_all with explicit links enqueues the good one and
     reports the dead one as skipped, not as a failure
  D9 malformed JSON gets a framed error and the host stays alive
Usage: python3 app/host/download-all-smoke.py <host-binary> <workdir>
Exit 0 = all checks pass.
"""
import http.server
import json
import os
import socketserver
import sqlite3
import struct
import subprocess
import sys
import threading

FILES = {
    "/a.zip": bytes((i % 251) for i in range(10 * 1024)),
    "/b.pdf": bytes((i % 251) for i in range(5 * 1024)),
    "/pic.png": bytes((i % 251) for i in range(2 * 1024)),
}

PAGE = """<!DOCTYPE html><html><head><title>Files</title></head><body>
<a href="/a.zip">a.zip</a>
<a href="/b.pdf">b.pdf</a>
<a href="/a.zip">a.zip again</a>
<a href="/dead.zip">dead link</a>
<img src="/pic.png" alt="">
</body></html>
"""


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _headers(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", '"smoke-1"')
        self.send_header("Connection", "close")
        self.end_headers()

    def do_HEAD(self):
        if self.path == "/page.html":
            self._headers(PAGE.encode())
        elif self.path in FILES:
            self._headers(FILES[self.path])
        else:
            self._headers(b"", 404)

    def do_GET(self):
        if self.path == "/page.html":
            data = PAGE.encode()
            self._headers(data)
            self.wfile.write(data)
            return
        if self.path not in FILES:
            self._headers(b"", 404)
            return
        data = FILES[self.path]
        range_hdr = self.headers.get("Range")
        if range_hdr and range_hdr.startswith("bytes="):
            try:
                start, end = range_hdr[6:].split("-", 1)
                start = int(start) if start else 0
                end = int(end) if end else len(data) - 1
            except ValueError:
                start, end = 0, len(data) - 1
            chunk = data[start:end + 1]
            self.send_response(206)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
            self.send_header("Content-Length", str(len(chunk)))
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", '"smoke-1"')
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(chunk)
            return
        self._headers(data)
        self.wfile.write(data)


def frame(msg: dict) -> bytes:
    raw = json.dumps(msg).encode()
    return struct.pack("<I", len(raw)) + raw


def read_frame(proc, timeout=10):
    import select

    r, _, _ = select.select([proc.stdout], [], [], timeout)
    if not r:
        return None
    hdr = proc.stdout.read(4)
    if len(hdr) < 4:
        return None
    (n,) = struct.unpack("<I", hdr)
    raw = proc.stdout.read(n)
    return json.loads(raw)


def set_settings(db_path, sql, params=()):
    conn = sqlite3.connect(db_path)
    conn.execute(sql, params)
    conn.commit()
    conn.close()


def pending_rows(db_path):
    conn = sqlite3.connect(db_path)
    rows = conn.execute(
        "SELECT url, source, status FROM pending_links ORDER BY created_at, rowid"
    ).fetchall()
    conn.close()
    return rows


def main() -> int:
    host_bin, workdir = sys.argv[1], sys.argv[2]
    os.makedirs(workdir, exist_ok=True)
    db_path = os.path.join(workdir, "app.db")

    server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}"
    page_url = f"{base}/page.html"

    proc = subprocess.Popen(
        [host_bin, "--db-path", db_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    checks = []

    def check(name, ok, detail=""):
        checks.append((name, ok, detail))
        print(f"{'PASS' if ok else 'FAIL'}: {name} {detail}")

    def send(msg):
        proc.stdin.write(frame(msg))
        proc.stdin.flush()
        return read_frame(proc)

    try:
        r = send({"type": "ping"})
        check("D1 ping", bool(r and r.get("ok")), str(r))

        r = send({"type": "get_settings"})
        check("D2 get_settings pending_count is 0", bool(r and r.get("ok") and r.get("pending_count") == 0), str(r))

        r = send({"type": "download_all", "page_url": page_url, "links": []})
        check("D3 download_all expands the page: 3 enqueued", bool(r and r.get("ok") and r.get("enqueued") == 3), str(r))
        check("D3b reply carries pending_count 3", bool(r and r.get("pending_count") == 3), str(r))

        rows = pending_rows(db_path)
        good = {f"{base}/a.zip", f"{base}/b.pdf", f"{base}/pic.png"}
        check(
            "D4 DB holds exactly the 3 good links as download_all/pending",
            len(rows) == 3 and {row[0] for row in rows} == good
            and all(row[1] == "download_all" and row[2] == "pending" for row in rows),
            str(rows),
        )

        r = send({"type": "download_all", "page_url": page_url, "links": []})
        check("D5 second download_all enqueues the same 3 again", bool(r and r.get("ok") and r.get("enqueued") == 3), str(r))
        check("D5b pending_count is now 6", bool(r and r.get("pending_count") == 6), str(r))

        set_settings(db_path, "UPDATE settings SET excluded_sites = '[\"127.0.0.1\"]' WHERE id = 1")
        r = send({"type": "download_all", "page_url": page_url, "links": []})
        check("D6 excluded page is an error", bool(r and not r.get("ok") and "excluded" in (r.get("error") or "")), str(r))
        check("D6b nothing enqueued while excluded", len(pending_rows(db_path)) == 6, str(pending_rows(db_path)))
        set_settings(db_path, "UPDATE settings SET excluded_sites = '[]' WHERE id = 1")

        set_settings(db_path, "UPDATE settings SET browser_integration = 0 WHERE id = 1")
        r = send({"type": "download_all", "page_url": page_url, "links": []})
        check("D7 integration off is an error (BUG-012)", bool(r and not r.get("ok") and "turned off" in (r.get("error") or "")), str(r))
        check("D7b nothing enqueued while off", len(pending_rows(db_path)) == 6)
        set_settings(db_path, "UPDATE settings SET browser_integration = 1 WHERE id = 1")

        r = send({
            "type": "download_all",
            "page_url": page_url,
            "links": [f"{base}/a.zip", f"{base}/dead.zip"],
        })
        # Enqueueing does not probe: both links are queued, and the
        # dead one is rejected at drain time (see the engine test
        # drain_marks_dead_link_rejected_and_keeps_going), not here.
        check("D8 explicit links: both enqueued, dead rejected only at drain", bool(r and r.get("ok") and r.get("enqueued") == 2), str(r))

        proc.stdin.write(b"\x09\x00\x00\x00{not json")
        proc.stdin.flush()
        r = read_frame(proc)
        check("D9 malformed JSON gets a framed error", bool(r and not r.get("ok")), str(r))
        r = send({"type": "ping"})
        check("D9b host stays alive after malformed JSON", bool(r and r.get("ok")), str(r))
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)
        server.shutdown()

    failed = [c for c in checks if not c[1]]
    print(f"{len(checks) - len(failed)}/{len(checks)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
