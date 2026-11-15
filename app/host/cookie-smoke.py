#!/usr/bin/env python3
"""Smoke-test cookie forwarding through the real fetchline-host binary (S13 finish).

Drives the binary with native-messaging frames against a fresh
database (no HTTP server needed — the host only enqueues; the
drain is covered by the engine tests). Checks:
  C1 ping works
  C2 download_link with cookies is accepted, the pending row
     stores exactly the sent pairs, and the reply never contains
     the cookie value (no cookie is ever logged or echoed back)
  C3 download_link with a cookie value containing ';' is refused,
     nothing enqueued (a ';' would smuggle a second cookie into
     the header)
  C4 download_link with a cookie name containing a space is
     refused, nothing enqueued
  C5 download_link without cookies is accepted and the row stores
     NULL cookies (no cookies field, no empty list)
  C6 video_found with cookies is accepted, the row stores them,
     source 'video'
  C7 download_link to an excluded site WITH cookies is refused,
     nothing enqueued (exclusion still wins)
All cookie values below are test-only dummies, never real ones.
Usage: python3 app/host/cookie-smoke.py <host-binary> <workdir>
Exit 0 = all checks pass.
"""
import json
import os
import sqlite3
import struct
import subprocess
import sys

# Test-only dummy cookie values, never real ones.
DUMMY_COOKIES = [{"name": "session", "value": "test-dummy-value"}]
DUMMY_VALUE = "test-dummy-value"


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
        "SELECT url, source, status, cookies FROM pending_links ORDER BY created_at, rowid"
    ).fetchall()
    conn.close()
    return rows


def main() -> int:
    host_bin, workdir = sys.argv[1], sys.argv[2]
    os.makedirs(workdir, exist_ok=True)
    db_path = os.path.join(workdir, "app.db")

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
        check("C1 ping", bool(r and r.get("ok")), str(r))

        url = "http://127.0.0.1/file.zip"
        r = send({"type": "download_link", "url": url, "cookies": DUMMY_COOKIES})
        check("C2 download_link with cookies is accepted", bool(r and r.get("ok")), str(r))
        check(
            "C2b reply never contains the cookie value",
            DUMMY_VALUE not in json.dumps(r),
            str(r),
        )
        rows = pending_rows(db_path)
        check(
            "C2c row stores exactly the sent pairs",
            len(rows) == 1 and json.loads(rows[0][3]) == DUMMY_COOKIES,
            str(rows),
        )

        r = send({
            "type": "download_link",
            "url": "http://127.0.0.1/other.zip",
            "cookies": [{"name": "session", "value": "a;b"}],
        })
        check("C3 cookie value with ';' is refused", bool(r and not r.get("ok")), str(r))
        check("C3b nothing enqueued", len(pending_rows(db_path)) == 1, str(pending_rows(db_path)))

        r = send({
            "type": "download_link",
            "url": "http://127.0.0.1/other.zip",
            "cookies": [{"name": "bad name", "value": "x"}],
        })
        check("C4 cookie name with a space is refused", bool(r and not r.get("ok")), str(r))
        check("C4b nothing enqueued", len(pending_rows(db_path)) == 1, str(pending_rows(db_path)))

        r = send({"type": "download_link", "url": "http://127.0.0.1/plain.zip"})
        check("C5 download_link without cookies is accepted", bool(r and r.get("ok")), str(r))
        rows = pending_rows(db_path)
        check(
            "C5b row stores NULL cookies",
            len(rows) == 2 and rows[1][3] is None,
            str(rows),
        )

        r = send({
            "type": "video_found",
            "page_url": "http://127.0.0.1/watch",
            "variants": [{"url": "http://127.0.0.1/clip.mp4", "quality": "720p"}],
            "cookies": DUMMY_COOKIES,
        })
        check("C6 video_found with cookies is accepted", bool(r and r.get("ok")), str(r))
        rows = pending_rows(db_path)
        check(
            "C6b row stores the cookies with source 'video'",
            len(rows) == 3 and rows[2][1] == "video"
            and json.loads(rows[2][3]) == DUMMY_COOKIES,
            str(rows),
        )

        set_settings(db_path, "UPDATE settings SET excluded_sites = '[\"127.0.0.1\"]' WHERE id = 1")
        r = send({"type": "download_link", "url": url, "cookies": DUMMY_COOKIES})
        check("C7 excluded site with cookies is refused", bool(r and not r.get("ok")), str(r))
        check("C7b nothing enqueued", len(pending_rows(db_path)) == 3, str(pending_rows(db_path)))
        set_settings(db_path, "UPDATE settings SET excluded_sites = '[]' WHERE id = 1")
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)

    failed = [c for c in checks if not c[1]]
    print(f"{len(checks) - len(failed)}/{len(checks)} cookie smoke checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
