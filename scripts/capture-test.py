#!/usr/bin/env python3
"""Windows-runner browser-capture test for Fetchline (CI job
``extension-capture-test``).

Proves a link from a real browser reaches the desktop app on
Windows, in three phases. Exit 0 only if every enabled phase
passes; any failure prints ``FAIL: <phase>: <reason>`` and exits 1.

Phase 1 — host protocol (deterministic): drive the real
``fetchline-host.exe`` with real Native Messaging frames
(4-byte little-endian length + JSON) against the desktop app's
real ``app.db``: ``ping`` must answer ok, ``get_settings`` must
report browser integration on, and ``download_link`` for the
pinned file must answer ok with a ``pending_link_id`` — then the
row is read back from SQLite and must be there, source
``browser``, status ``pending``.

Phase 2 — real browser transport: load the unpacked extension in
headless Chrome, discover its extension ID, and verify the
GENUINE installed handshake: the MSI must have written the
HKCU native-messaging registry key and shipped the host manifest
+ fetchline-host.exe into the install dir. The test reads the
key and the installed manifest and checks they point at the
installed host with the extension's stable id — it does NOT
write its own manifest or key (that was the old runner-only
workaround for the packaging gap; the installer now provides
both). Then it opens popup.html and polls its status text over
CDP until it reports "Connected to the Fetchline desktop app".
This is the step that proves Chrome itself can spawn the
installed host — Phase 1 alone only proves the host binary, not
Chrome's side of it. The extension id is stable because
manifest.json carries a public `key` (Chrome derives the id
from it); the test derives the expected id from that key and
requires the discovered id to match it. Discovery looks for a
CDP target carrying the EXPECTED id (BUG-042: /json/list also
lists Chrome's built-in Hangouts component extension, whose id
used to be mistaken for ours) and falls back to the Preferences
entry for the extension dir, which Chrome writes on registration.

Phase 3 — drain to file: launch the installed desktop app,
wait for it to drain the queue (startup + 30 s tick) and download
the pinned file through the real engine, then verify the file in
the Downloads folder byte-for-byte (SHA-256).

What this does NOT prove (honest limits): a human right-click
on a link ("Download with Fetchline") or the download watcher
cannot be driven in headless Chrome — there is no mouse and no
real popup window. Those paths use the SAME ``sendNativeMessage``
+ ``handToFetchline`` code Phase 2 exercises (the popup's ping
and the menu's ``download_link`` go through the same transport),
and the menu/watcher wiring itself is covered by the Node
fake-``chrome`` tests (``npm run test:extension*``).

Usage (PowerShell, from the repo root):
  python scripts/capture-test.py --host-exe "C:\\Program Files\\Fetchline\\fetchline-host.exe"
    --db-path <app.db> --extension-dir app/extension
    --chrome-exe "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
    --app-exe "C:\\Program Files\\Fetchline\\fetchline.exe"
    --url <pinned file URL> --expected-sha256 <hex>
    --downloads-dir <Downloads folder>
    [--skip-browser] [--skip-drain]

``--host-exe`` is the INSTALLED host (the MSI ships it next to
the app binary); Phase 2 verifies the installed registry key +
manifest point at it instead of registering its own.

``--skip-browser`` / ``--skip-drain`` exist for local debugging
only (Phase 1 alone can run on Linux against a built host).
Requires ``websocket-client`` (``pip install websocket-client``)
for Phase 2; Phase 1 and 3 are stdlib-only.
"""

import argparse
import base64
import hashlib
import json
import os
import re
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

REPORT = "capture-test-report.txt"
RESULTS = []


def log(msg):
    print(msg, flush=True)
    RESULTS.append(msg)


def fail(phase, reason):
    log(f"FAIL: {phase}: {reason}")
    write_report()
    sys.exit(1)


def write_report():
    try:
        with open(REPORT, "w", encoding="utf-8") as f:
            f.write("\n".join(RESULTS) + "\n")
    except OSError as e:
        print(f"(could not write {REPORT}: {e})", flush=True)


# ---------------------------------------------------------------- framing

def frame(payload):
    data = json.dumps(payload).encode("utf-8")
    return struct.pack("<I", len(data)) + data


def read_exact(stream, n):
    buf = b""
    while len(buf) < n:
        chunk = stream.read(n - len(buf))
        if not chunk:
            raise EOFError(f"short read: got {len(buf)} of {n} bytes")
        buf += chunk
    return buf


def send_and_reply(proc, payload):
    proc.stdin.write(frame(payload))
    proc.stdin.flush()
    raw_len = read_exact(proc.stdout, 4)
    (length,) = struct.unpack("<I", raw_len)
    if length > 16 * 1024 * 1024:
        raise ValueError(f"host reply too large: {length} bytes")
    raw = read_exact(proc.stdout, length)
    return json.loads(raw.decode("utf-8"))


# ---------------------------------------------------------------- Phase 1

def phase_host(host_exe, db_path, url):
    log("PHASE 1: host protocol — drive fetchline-host with real frames")
    if not os.path.isfile(host_exe):
        fail("host", f"fetchline-host.exe not found at {host_exe}")
    if not os.path.isfile(db_path):
        fail("host", f"app.db not found at {db_path} (launch the app once first)")
    proc = subprocess.Popen(
        [host_exe, "--db-path", db_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        reply = send_and_reply(proc, {"type": "ping"})
        if not reply.get("ok"):
            fail("host", f"ping reply not ok: {reply}")
        log(f"  ping ok: {reply.get('type')}")
        reply = send_and_reply(proc, {"type": "get_settings"})
        if not reply.get("ok"):
            fail("host", f"get_settings reply not ok: {reply}")
        if reply.get("browser_integration") is not True:
            fail("host", f"browser_integration is not true: {reply}")
        log(f"  get_settings ok: browser_integration=true, pending_count={reply.get('pending_count')}")
        reply = send_and_reply(proc, {"type": "download_link", "url": url, "referrer": None})
        if not reply.get("ok"):
            fail("host", f"download_link reply not ok: {reply}")
        link_id = reply.get("pending_link_id")
        if not link_id:
            fail("host", f"download_link reply has no pending_link_id: {reply}")
        log(f"  download_link ok: pending_link_id={link_id}, pending_count={reply.get('pending_count')}")
    finally:
        try:
            proc.stdin.close()
        except BrokenPipeError:
            pass
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    # The row must be in the app's own database, source browser, pending.
    uri = "file:" + urllib.parse.quote(db_path) + "?mode=ro"
    try:
        conn = sqlite3.connect(uri, uri=True, timeout=10)
        try:
            row = conn.execute(
                "SELECT url, source, status FROM pending_links WHERE id = ?", (link_id,)
            ).fetchone()
        finally:
            conn.close()
    except sqlite3.Error as e:
        fail("host", f"could not read pending_links from {db_path}: {e}")
    if row is None:
        fail("host", f"no pending_links row with id {link_id} in {db_path}")
    db_url, source, status = row
    if db_url != url or source != "browser" or status != "pending":
        fail("host", f"pending_links row is wrong: url={db_url!r} source={source!r} status={status!r}")
    log(f"  pending_links row confirmed: url matches, source=browser, status=pending")
    log("PHASE 1 PASSED")
    return link_id


# ---------------------------------------------------------------- Phase 2

def chrome_targets(port, timeout=30):
    deadline = time.time() + timeout
    last_err = None
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=5) as r:
                return json.loads(r.read().decode("utf-8"))
        except Exception as e:  # noqa: BLE001 - Chrome may not be up yet
            last_err = e
            time.sleep(1)
    raise TimeoutError(f"Chrome DevTools endpoint did not answer in {timeout}s: {last_err}")


def find_expected_target_id(port, expected_id, timeout=20):
    """Poll /json/list until a target belonging to the extension
    with ``expected_id`` appears, or the deadline passes.

    BUG-042: /json/list returns EVERY target, including Chrome's
    built-in component extensions — the hidden Google Hangouts
    extension (id nkeimhogjdpnpccoofpliimaahmaaome) is always
    present, and its service-worker target used to come first, so
    taking the first chrome-extension:// URL returned Hangouts'
    id, not ours. The only sound way to identify OUR extension
    among the targets is to look for the expected id itself: if
    Chrome loaded our extension with its shipped key, a target
    with that id exists. Returns (id_or_None, sorted_seen_ids)."""
    deadline = time.time() + timeout
    seen = set()
    while time.time() < deadline:
        try:
            targets = chrome_targets(port, timeout=5)
        except TimeoutError:
            time.sleep(1)
            continue
        for t in targets:
            m = re.match(r"chrome-extension://([a-z]{32})/", t.get("url", ""))
            if m:
                seen.add(m.group(1))
        if expected_id in seen:
            return expected_id, sorted(seen)
        time.sleep(1)
    return None, sorted(seen)


def extension_id_from_preferences(profile_dir, extension_dir):
    """Fallback: the ID is stored in the profile's Preferences once
    the unpacked extension is installed; its ``path`` is the dir."""
    prefs_path = os.path.join(profile_dir, "Default", "Preferences")
    try:
        with open(prefs_path, "r", encoding="utf-8") as f:
            prefs = json.load(f)
    except (OSError, ValueError):
        return None
    settings = prefs.get("extensions", {}).get("settings", {})
    want = os.path.normcase(os.path.abspath(extension_dir))
    for ext_id, entry in settings.items():
        if isinstance(entry, dict) and os.path.normcase(entry.get("path", "")) == want:
            return ext_id
    return None


def chrome_id_from_key(key_b64):
    """Chrome's extension id: SHA-256 of the DER public key, first
    16 bytes, each nibble mapped to a-p. The id is stable across
    machines because it is derived from the key in manifest.json."""
    try:
        der = base64.b64decode(key_b64, validate=True)
    except (ValueError, base64.binascii.Error):
        return None
    digest = hashlib.sha256(der).digest()[:16]
    return "".join(
        chr(ord("a") + ((b >> 4) & 0xF)) + chr(ord("a") + (b & 0xF))
        for b in digest
    )


def read_registry_default(key_path):
    """Return the default value of an HKCU key via `reg query`,
    or None if the key/value is absent. (The MSI/NSIS installer
    writes these keys; the test only reads them.)"""
    try:
        out = subprocess.run(
            ["reg", "query", key_path, "/ve"],
            capture_output=True, text=True, timeout=30,
        )
    except (subprocess.SubprocessError, OSError) as e:
        fail("browser", f"reg query failed for {key_path}: {e}")
    if out.returncode != 0:
        return None
    for line in out.stdout.splitlines():
        if "(Default)" in line and "REG_SZ" in line:
            return line.split("REG_SZ", 1)[1].strip()
    return None


def verify_installed_handshake(extension_dir, host_exe):
    """Verify what the installer must have provided: the HKCU
    native-messaging key pointing at the installed Chrome manifest,
    the manifest pointing at the installed host exe, and the
    manifest naming the extension's stable id. Returns the expected
    extension id. Any mismatch fails — this is the genuine
    installed path, not a test-only registration."""
    manifest_path = os.path.join(extension_dir, "manifest.json")
    try:
        with open(manifest_path, encoding="utf-8") as f:
            ext_manifest = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        fail("browser", f"cannot read {manifest_path}: {e}")
    expected_id = chrome_id_from_key(ext_manifest.get("key", ""))
    if not expected_id:
        fail(
            "browser",
            "extension manifest.json has no usable key — no stable id "
            "to expect (the id is derived from the key)",
        )
    log(f"  expected extension id (from manifest.json key): {expected_id}")

    key_path = (
        r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host"
    )
    installed_manifest = read_registry_default(key_path)
    if not installed_manifest:
        fail(
            "browser",
            "registry key HKCU\\Software\\Google\\Chrome\\"
            "NativeMessagingHosts\\com.fetchline.host is missing — the "
            "installer did not register the native host",
        )
    log(f"  registry key -> {installed_manifest}")
    if not os.path.isfile(installed_manifest):
        fail(
            "browser",
            f"registry key points at {installed_manifest}, which does "
            "not exist — the installer did not ship the host manifest",
        )
    try:
        with open(installed_manifest, encoding="utf-8") as f:
            host_manifest = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        fail("browser", f"cannot read installed manifest {installed_manifest}: {e}")
    manifest_host = host_manifest.get("path", "")
    if os.path.normcase(os.path.abspath(manifest_host)) != os.path.normcase(
        os.path.abspath(host_exe)
    ):
        fail(
            "browser",
            f"installed manifest path is {manifest_host!r}, expected the "
            f"installed host {host_exe!r} — the installer shipped a "
            "manifest pointing at the wrong host",
        )
    origins = host_manifest.get("allowed_origins", [])
    expected_origin = f"chrome-extension://{expected_id}/"
    if origins != [expected_origin]:
        fail(
            "browser",
            f"installed manifest allowed_origins is {origins!r}, expected "
            f"[{expected_origin!r}] — the installer shipped a manifest "
            "naming the wrong extension id",
        )
    log("  installed manifest: path + allowed_origins OK")
    return expected_id


def phase_browser(chrome_exe, extension_dir, host_exe):
    log("PHASE 2: real browser — headless Chrome + extension + native host")
    try:
        import websocket  # noqa: F401  (websocket-client, pip-installed)
    except ImportError:
        fail("browser", "websocket-client is not installed (run: python -m pip install websocket-client)")
    if not os.path.isfile(chrome_exe):
        fail("browser", f"chrome.exe not found at {chrome_exe}")
    extension_dir = os.path.abspath(extension_dir)
    if not os.path.isfile(os.path.join(extension_dir, "manifest.json")):
        fail("browser", f"no manifest.json in {extension_dir}")
    # The installer provides the host + registry key; verify them
    # before launching Chrome — a missing key here is an installer
    # bug, and the popup check below would only repeat the same
    # failure more slowly.
    expected_id = verify_installed_handshake(extension_dir, host_exe)
    profile_dir = tempfile.mkdtemp(prefix="fl-chrome-profile-")
    chrome = subprocess.Popen(
        [
            chrome_exe,
            "--headless=new",
            "--no-first-run",
            "--no-sandbox",
            "--disable-gpu",
            f"--user-data-dir={profile_dir}",
            "--remote-debugging-port=9222",
            f"--load-extension={extension_dir}",
            "about:blank",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        try:
            chrome_targets(9222, timeout=30)
        except TimeoutError as e:
            fail("browser", str(e))
        # The extension's service worker starts lazily, so poll for
        # a target carrying the expected id. Any other
        # chrome-extension:// id in the list belongs to a different
        # extension (Chrome's built-in Hangouts component extension
        # is always there — BUG-042) and must never be mistaken for
        # ours.
        ext_id, seen_ids = find_expected_target_id(9222, expected_id, timeout=20)
        if ext_id:
            others = [i for i in seen_ids if i != expected_id]
            log(f"  extension target found in CDP (id {ext_id})"
                + (f"; other chrome-extension ids present (ignored): {others}" if others else ""))
        else:
            log(f"  expected id {expected_id} not among CDP targets "
                f"(seen: {seen_ids or 'none'}); falling back to Preferences")
            # Preferences is written when Chrome registers the
            # extension; poll briefly in case it is not flushed yet.
            pref_deadline = time.time() + 10
            while time.time() < pref_deadline:
                ext_id = extension_id_from_preferences(profile_dir, extension_dir)
                if ext_id:
                    break
                time.sleep(1)
        if not ext_id:
            fail("browser", "could not discover the extension ID (no chrome-extension:// target with the expected id and no Preferences entry)")
        log(f"  extension ID: {ext_id}")
        if ext_id != expected_id:
            fail(
                "browser",
                f"discovered extension id {ext_id} is not the stable id "
                f"{expected_id} derived from manifest.json's key — the "
                "extension was not loaded with its shipped key",
            )
        log("  discovered id matches the stable id from manifest.json's key")

        # Open popup.html as a tab and poll its status text over CDP.
        popup_url = f"chrome-extension://{ext_id}/popup.html"
        req = urllib.request.Request(
            "http://127.0.0.1:9222/json/new?" + urllib.parse.urlencode({"url": popup_url}),
            method="PUT",
        )
        try:
            with urllib.request.urlopen(req, timeout=10) as r:
                popup_target = json.loads(r.read().decode("utf-8"))
        except Exception as e:  # noqa: BLE001
            fail("browser", f"could not open {popup_url} via CDP: {e}")
        ws_url = popup_target.get("webSocketDebuggerUrl")
        if not ws_url:
            fail("browser", f"CDP did not return a websocket URL for the popup target: {popup_target}")
        from websocket import create_connection

        ws = create_connection(ws_url, timeout=10)
        msg_id = [0]

        def evaluate(expression):
            msg_id[0] += 1
            mid = msg_id[0]
            ws.send(json.dumps({
                "id": mid, "method": "Runtime.evaluate",
                "params": {"expression": expression, "returnByValue": True},
            }))
            deadline = time.time() + 10
            while time.time() < deadline:
                try:
                    raw = ws.recv()
                except Exception:  # noqa: BLE001 - timeout: treat as no answer yet
                    return None
                try:
                    msg = json.loads(raw)
                except ValueError:
                    continue
                if msg.get("id") != mid:
                    continue  # event or another call's reply
                result = msg.get("result", {}).get("result", {})
                if "value" in result:
                    return result["value"]
                if "description" in result:
                    return result["description"]
                return None
            return None

        try:
            ws.send(json.dumps({"id": 0, "method": "Runtime.enable"}))
            status_text = ""
            deadline = time.time() + 20
            while time.time() < deadline:
                status_text = evaluate(
                    "document.getElementById('status') ? document.getElementById('status').textContent : ''"
                ) or ""
                if "Connected to the Fetchline desktop app" in status_text:
                    break
                time.sleep(0.5)
        finally:
            try:
                ws.close()
            except Exception:  # noqa: BLE001
                pass
        log(f"  popup status text: {status_text!r}")
        if "Connected to the Fetchline desktop app" not in status_text:
            fail("browser", f"popup did not report a connection (status text: {status_text!r}) — Chrome could not reach the host")
        log("PHASE 2 PASSED")
    finally:
        try:
            chrome.terminate()
            chrome.wait(timeout=10)
        except Exception:  # noqa: BLE001
            try:
                chrome.kill()
            except Exception:  # noqa: BLE001
                pass
        shutil.rmtree(profile_dir, ignore_errors=True)


# ---------------------------------------------------------------- Phase 3

def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def phase_drain(app_exe, downloads_dir, expected_sha256, timeout=120):
    log("PHASE 3: drain — app downloads the pinned file, bytes verified")
    if not os.path.isfile(app_exe):
        fail("drain", f"fetchline.exe not found at {app_exe}")
    if not os.path.isdir(downloads_dir):
        fail("drain", f"Downloads folder not found at {downloads_dir}")
    target = os.path.join(downloads_dir, "LICENSE")
    if os.path.exists(target):
        os.remove(target)  # fresh run: a stale file would fake a pass
    app = subprocess.Popen([app_exe], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.time() + timeout
        got = None
        while time.time() < deadline:
            if os.path.isfile(target):
                try:
                    if sha256_file(target) == expected_sha256:
                        got = target
                        break
                except OSError:
                    pass  # still being written; keep waiting
            if app.poll() is not None:
                fail("drain", f"the app exited (code {app.returncode}) before the download finished")
            time.sleep(2)
        if got is None:
            if os.path.isfile(target):
                fail("drain", f"LICENSE appeared in {downloads_dir} but its SHA-256 is not {expected_sha256} (partial download or wrong bytes)")
            fail("drain", f"no LICENSE file in {downloads_dir} after {timeout}s — the drain did not start/finish the download")
        log(f"  downloaded file verified: {got} SHA-256 matches")
    finally:
        try:
            app.terminate()
            app.wait(timeout=5)
        except Exception:  # noqa: BLE001
            try:
                app.kill()
            except Exception:  # noqa: BLE001
                pass
            if os.name == "nt":
                subprocess.run(["taskkill", "/F", "/IM", "fetchline.exe"],
                               capture_output=True, timeout=15)
    log("PHASE 3 PASSED")


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description="Fetchline Windows-runner capture test")
    ap.add_argument("--host-exe", required=True)
    ap.add_argument("--db-path", required=True)
    ap.add_argument("--extension-dir", required=True)
    ap.add_argument("--chrome-exe", required=True)
    ap.add_argument("--app-exe", required=True)
    ap.add_argument("--url", required=True)
    ap.add_argument("--expected-sha256", required=True)
    ap.add_argument("--downloads-dir", required=True)
    ap.add_argument("--skip-browser", action="store_true",
                    help="local debugging only: skip Phase 2")
    ap.add_argument("--skip-drain", action="store_true",
                    help="local debugging only: skip Phase 3")
    args = ap.parse_args()

    phase_host(args.host_exe, args.db_path, args.url)
    ran = ["host"]
    if not args.skip_browser:
        phase_browser(args.chrome_exe, args.extension_dir, args.host_exe)
        ran.append("browser")
    else:
        log("PHASE 2 SKIPPED (--skip-browser)")
    if not args.skip_drain:
        phase_drain(args.app_exe, args.downloads_dir, args.expected_sha256)
        ran.append("drain")
    else:
        log("PHASE 3 SKIPPED (--skip-drain)")
    log(f"CAPTURE TEST PASSED (phases run: {', '.join(ran)})")
    write_report()


if __name__ == "__main__":
    main()
