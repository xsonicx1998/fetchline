#!/usr/bin/env python3
"""Smoke-test the real fetchline-host binary (S08 / F05).

Drives the binary with native-messaging frames against a local HTTP
server serving a real .mp4 fixture (deterministic bytes, HEAD +
Range support). Checks:
  V1 ping works
  V2 video_found with a good .mp4 enqueues one pending_links row
     (source 'video', referrer = page URL)
  V3 video_found with a DRM variant is an error, nothing enqueued
  V4 video_found with a bad-scheme variant is an error
  V5 video_found with empty variants is an error
  V6 video_found to an excluded site is an error
  V7 download_link (S13 regression) still enqueues
  V8 malformed JSON gets a framed error and the host stays alive
  V9 video_found with a real .m3u8 playlist enqueues (offered
     as-is, never merged — Milestone 3 HLS decision)
  V10 video_found with a .mpd DASH manifest enqueues as-is
The drain (pending_links -> add_download) is covered by the engine
test video_found_end_to_end_drains_into_a_real_download (real
.mp4) and video_found_m3u8_drains_into_a_real_playlist_download
(real .m3u8, byte-exact), which run the same handle_host_message
code the binary calls.
Usage: python3 /tmp/fetchline-video-smoke.py <host-binary> <workdir>
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

PAYLOAD = bytes((i % 251) for i in range(100 * 1024))
PLAYLIST = b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXT-X-ENDLIST\n"
MANIFEST = b'<?xml version="1.0"?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static"><Period><AdaptationSet><Representation width="1280" height="720"/></AdaptationSet></Period></MPD>'


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _head(self, body, ctype):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", '"mp4-1"')
        self.send_header("Connection", "close")
        self.end_headers()

    def do_HEAD(self):
        if self.path == "/clip.mp4":
            self._head(PAYLOAD, "video/mp4")
        elif self.path == "/stream.m3u8":
            self._head(PLAYLIST, "application/vnd.apple.mpegurl")
        elif self.path == "/stream.mpd":
            self._head(MANIFEST, "application/dash+xml")
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()

    def do_GET(self):
        if self.path == "/clip.mp4":
            self._head(PAYLOAD, "video/mp4")
            self.wfile.write(PAYLOAD)
        elif self.path == "/stream.m3u8":
            self._head(PLAYLIST, "application/vnd.apple.mpegurl")
            self.wfile.write(PLAYLIST)
        elif self.path == "/stream.mpd":
            self._head(MANIFEST, "application/dash+xml")
            self.wfile.write(MANIFEST)
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()


def frame(msg: dict) -> bytes:
    raw = json.dumps(msg).encode()
    return struct.pack("<I", len(raw)) + raw


def read_frame(proc, timeout=10):
    proc.stdout.flush()
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


def main() -> int:
    host_bin, workdir = sys.argv[1], sys.argv[2]
    os.makedirs(workdir, exist_ok=True)
    db_path = os.path.join(workdir, "app.db")

    server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}"
    video_url = f"{base}/clip.mp4"
    page_url = f"{base}/watch"

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
        check("V1 ping", bool(r and r.get("ok")), str(r))

        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": video_url, "quality": "720p", "format": "MP4"}],
        })
        check("V2 video_found enqueues", bool(r and r.get("ok") and r.get("enqueued") == 1), str(r))

        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": video_url, "quality": "720p", "format": "MP4", "drm": True}],
        })
        check("V3 DRM variant refused", bool(r and not r.get("ok") and "DRM" in (r.get("error") or "")), str(r))

        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": "ftp://example.com/clip.mp4"}],
        })
        check("V4 bad-scheme variant refused", bool(r and not r.get("ok")), str(r))

        r = send({"type": "video_found", "page_url": page_url, "variants": []})
        check("V5 empty variants refused", bool(r and not r.get("ok")), str(r))

        # Exclude the fixture host, then retry: must refuse.
        conn = sqlite3.connect(db_path)
        conn.execute("UPDATE settings SET excluded_sites = '[\"127.0.0.1\"]' WHERE id = 1")
        conn.commit()
        conn.close()
        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": video_url}],
        })
        check("V6 excluded site refused", bool(r and not r.get("ok") and "excluded" in (r.get("error") or "")), str(r))
        conn = sqlite3.connect(db_path)
        conn.execute("UPDATE settings SET excluded_sites = '[]' WHERE id = 1")
        conn.commit()
        conn.close()

        r = send({"type": "download_link", "url": video_url, "referrer": page_url})
        check("V7 S13 download_link still works", bool(r and r.get("ok")), str(r))

        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": f"{base}/stream.m3u8", "live": False, "format": "M3U8"}],
        })
        check("V9 video_found .m3u8 playlist enqueues as-is", bool(r and r.get("ok") and r.get("enqueued") == 1), str(r))

        r = send({
            "type": "video_found",
            "page_url": page_url,
            "variants": [{"url": f"{base}/stream.mpd", "format": "MPD"}],
        })
        check("V10 video_found .mpd manifest enqueues as-is", bool(r and r.get("ok") and r.get("enqueued") == 1), str(r))

        proc.stdin.write(b"\x09\x00\x00\x00{not json")
        proc.stdin.flush()
        r = read_frame(proc)
        check("V8 malformed JSON gets a framed error", bool(r and not r.get("ok")), str(r))
        r = send({"type": "ping"})
        check("V8b host stays alive after malformed JSON", bool(r and r.get("ok")), str(r))
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)
        server.shutdown()

    conn = sqlite3.connect(db_path)
    rows = conn.execute(
        "SELECT url, referrer, source, status FROM pending_links ORDER BY created_at, rowid"
    ).fetchall()
    conn.close()
    print("pending_links rows:", rows)
    video_rows = [r for r in rows if r[2] == "video"]
    check("DB: exactly three video rows, referrer is the page", len(video_rows) == 3 and all(r[1] == page_url for r in video_rows), str(video_rows))
    check("DB: one browser row from V7", len([r for r in rows if r[2] == "browser"]) == 1, str(rows))

    failed = [c for c in checks if not c[1]]
    print(f"{len(checks) - len(failed)}/{len(checks)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
