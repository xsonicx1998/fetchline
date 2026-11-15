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

Phase 2 — real browser transport: ONE headless Chrome launch
over the CDP pipe (``--remote-debugging-pipe``): install the
unpacked extension with ``Extensions.loadUnpacked`` (branded
Chrome 137+ silently ignores the old ``--load-extension`` flag,
so the extension was never registered: no worker in /json/list,
nothing in Preferences), then open popup.html in the SAME launch
(``Target.createTarget`` + ``attachToTarget``, flatten) and poll
its status text over that pipe session until it reports
"Connected to the Sonic Manager desktop app". There is no second
launch and no /json/list discovery: a loadUnpacked install does
NOT survive a fresh Chrome launch (VPS experiments A–D, Round
38 — the registration is flushed only on a clean shutdown, and
a fresh launch then REMOVES it), so the install's returned id +
the popup handshake are the proof instead. Before launching,
the test verifies the GENUINE installed handshake: the MSI must
have written the HKCU native-messaging registry key and shipped
the host manifest + fetchline-host.exe into the install dir. The
test reads the key and the installed manifest and checks they
point at the installed host with the extension's stable id — it
does NOT write its own manifest or key (that was the old
runner-only workaround for the packaging gap; the installer now
provides both). This is the step that proves Chrome itself can
spawn the installed host — Phase 1 alone only proves the host
binary, not Chrome's side of it. The extension id is stable
because manifest.json carries a public `key` (Chrome derives
the id from it); the test derives the expected id from that key
and requires the install's returned id to match it. Chrome's
stderr is captured to a file (and its tail appended to the
report on failure) instead of being discarded — a silent
extension-load failure leaves no other trace.

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
  python scripts/capture-test.py --host-exe "C:\\Program Files\\Sonic Manager\\fetchline-host.exe"
    --db-path <app.db> --extension-dir app/extension
    --chrome-exe "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
    --app-exe "C:\\Program Files\\Sonic Manager\\fetchline.exe"
    --url <pinned file URL> --expected-sha256 <hex>
    --downloads-dir <Downloads folder>
    [--skip-browser] [--skip-drain]

``--host-exe`` is the INSTALLED host (the MSI ships it next to
the app binary); Phase 2 verifies the installed registry key +
manifest point at it instead of registering its own.

``--skip-browser`` / ``--skip-drain`` exist for local debugging
only (Phase 1 alone can run on Linux against a built host).
Phase 2 is stdlib-only now (the CDP pipe needs no
websocket-client — the old port/websocket popup check was
removed in Round 38); Phase 1 and 3 are stdlib-only too.
"""

import argparse
import base64
import hashlib
import json
import os
import queue
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

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

def chrome_stderr_tail(stderr_path, n=30):
    """Last ``n`` non-empty lines of Chrome's stderr log (the
    launch writes it to a file because the step log is the only
    evidence the CI artifact carries). Missing/unreadable file
    yields an empty list — never a crash."""
    try:
        with open(stderr_path, "r", encoding="utf-8", errors="replace") as f:
            lines = [ln.rstrip("\n") for ln in f if ln.strip()]
    except OSError:
        return []
    return lines[-n:]


def stop_chrome(proc):
    """Terminate a Chrome Popen object if it is still running.
    Idempotent: safe to call twice (the phase's finally block
    calls it again after a failure path already stopped Chrome)."""
    if proc is None:
        return
    try:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=10)
    except Exception:  # noqa: BLE001 - fall back to kill
        try:
            proc.kill()
        except Exception:  # noqa: BLE001
            pass


class PipeCDP:
    """Minimal CDP client over Chrome's --remote-debugging-pipe.

    Chrome speaks JSON messages separated by NUL bytes on the
    pipe: the client writes commands, Chrome writes replies and
    events back. This transport is the ONLY way to reach the
    ``Extensions`` CDP domain (it is unavailable over the
    websocket/port transport), and ``Extensions.loadUnpacked``
    is the supported way to install an unpacked extension in a
    branded Chrome — see install_extension_via_pipe().

    Transport setup (Chromium's own pipe_builder.cc, used by
    ChromeDriver): on POSIX Chrome reads commands on fd 3 and
    writes replies on fd 4; on Windows the parent passes the
    two pipe HANDLE values with --remote-debugging-io-pipes
    (inherited handles keep their value in the child) and
    Chrome reads/writes those handles. The reader runs on a
    thread because select() cannot wait on a pipe on Windows.

    Launch contract, per platform (CPython's subprocess docs
    + Lib/subprocess.py): ``pass_fds`` is POSIX-only — CPython
    rejects it outright on Windows (``assert not pass_fds,
    "pass_fds not supported on Windows."``), which killed run
    #3 in under a second. On Windows the child inherits the
    pipe HANDLEs because they are flagged inheritable AND the
    process is created with bInheritHandles, i.e. Popen with
    ``close_fds=False``; on POSIX the child ends are placed
    on fds 3/4 and kept open with ``pass_fds`` (which forces
    close_fds True). The two mechanisms must never be mixed.
    """

    def __init__(self, chrome_exe, profile_dir, stderr_path):
        self._stderr_path = stderr_path
        self._queue = queue.Queue()
        self._next_id = 0
        self._write_fd = None
        self._read_fd = None
        # Pipe A: parent -> child (commands). Parent keeps wA,
        # child reads rA. Pipe B: child -> parent (replies).
        # Parent reads rB, child writes wB.
        rA, wA = os.pipe()
        rB, wB = os.pipe()
        args = [
            chrome_exe,
            "--headless=new",
            "--no-first-run",
            "--no-sandbox",
            "--disable-gpu",
            f"--user-data-dir={profile_dir}",
            "--remote-debugging-pipe",
            "--enable-unsafe-extension-debugging",
            "about:blank",
        ]
        if os.name == "nt":
            import msvcrt

            os.set_inheritable(rA, True)
            os.set_inheritable(wB, True)
            # Loud and early: the handles MUST be inheritable or
            # Chrome can never see them — close_fds=False only
            # inherits handles flagged inheritable. Fail here,
            # naming the platform, instead of launching a Chrome
            # that silently has no DevTools pipe.
            if not os.get_inheritable(rA) or not os.get_inheritable(wB):
                for fd in (rA, wA, rB, wB):
                    try:
                        os.close(fd)
                    except OSError:
                        pass
                raise RuntimeError(
                    "Windows pipe setup failed: DevTools pipe handles "
                    "are not inheritable (os.name=nt) — Chrome would "
                    "never receive the pipe named by "
                    "--remote-debugging-io-pipes"
                )
            in_handle = msvcrt.get_osfhandle(rA)
            out_handle = msvcrt.get_osfhandle(wB)
            args.append(
                "--remote-debugging-io-pipes=%d,%d" % (in_handle, out_handle)
            )
            # Windows: CPython rejects pass_fds outright (run #3:
            # AssertionError "pass_fds not supported on Windows.").
            # The child inherits the two pipe HANDLEs because they
            # are flagged inheritable and CreateProcess is called
            # with bInheritHandles (close_fds=False); inherited
            # HANDLEs keep their numeric value in the child, which
            # is exactly what --remote-debugging-io-pipes names.
            popen_kwargs = {"close_fds": False}
            self._write_fd = wA
            self._read_fd = rB
        else:
            # POSIX: Chrome reads commands on fd 3, writes on fd 4.
            # Move the parent ends out of the way first (fresh
            # fds), then place the child ends on 3/4: dup2 closes
            # its target first, so a naive order could close a
            # pipe end still needed (e.g. dup2(wB, 4) closing wA
            # when the pipes came back as 3,4,5,6).
            self._write_fd = os.dup(wA)
            self._read_fd = os.dup(rB)
            os.close(wA)
            os.close(rB)
            if rA != 3:
                os.dup2(rA, 3)
                os.close(rA)
            if wB != 4:
                os.dup2(wB, 4)
                os.close(wB)
            # POSIX: pass_fds keeps fds 3/4 open across the exec
            # (it also forces close_fds True — the two must not
            # be combined the other way round).
            popen_kwargs = {"pass_fds": (3, 4)}
        err = open(stderr_path, "wb")
        self._err_file = err
        try:
            self._proc = subprocess.Popen(
                args,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=err,
                **popen_kwargs,
            )
        except AssertionError as e:
            # CPython's Windows guard (Lib/subprocess.py,
            # _execute_child: "pass_fds not supported on
            # Windows."). A bare AssertionError deep in the run
            # tells nobody what went wrong — name the platform
            # and the mechanism. install_extension_via_pipe
            # catches RuntimeError and turns it into a FAIL line.
            err.close()
            raise RuntimeError(
                f"cannot start Chrome on {sys.platform}: {e} "
                "(DevTools pipe handles must be inherited via "
                "close_fds=False, never pass_fds)"
            ) from e
        except Exception:
            err.close()
            raise
        # The parent's copies of the child ends must close, or
        # the reader would never see EOF when Chrome exits.
        if os.name == "nt":
            os.close(rA)
            os.close(wB)
        else:
            os.close(3)
            os.close(4)
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self):
        buf = b""
        try:
            while True:
                chunk = os.read(self._read_fd, 65536)
                if not chunk:
                    break
                buf += chunk
                while b"\0" in buf:
                    raw, buf = buf.split(b"\0", 1)
                    if not raw:
                        continue
                    try:
                        msg = json.loads(raw.decode("utf-8"))
                    except ValueError:
                        continue
                    self._queue.put(msg)
        except OSError:
            pass
        finally:
            self._queue.put(None)  # EOF marker

    def call(self, method, params=None, timeout=30, session_id=None):
        """Send one CDP command and wait for its reply, skipping
        events (messages with no matching id). ``session_id``
        targets a flattened session (Target.attachToTarget) — the
        reply must then carry both the matching id and that
        sessionId. Raises TimeoutError / RuntimeError with
        Chrome's own error text when the reply is an error or the
        browser dies first."""
        self._next_id += 1
        mid = self._next_id
        payload = {"id": mid, "method": method, "params": params or {}}
        if session_id is not None:
            payload["sessionId"] = session_id
        payload = json.dumps(payload).encode("utf-8") + b"\0"
        os.write(self._write_fd, payload)
        deadline = time.time() + timeout
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                raise TimeoutError(
                    f"no reply to {method} in {timeout}s (Chrome may not have started)"
                )
            if self._proc.poll() is not None:
                raise RuntimeError(
                    f"Chrome exited (code {self._proc.returncode}) while waiting for {method}"
                )
            try:
                msg = self._queue.get(timeout=min(remaining, 1))
            except queue.Empty:
                continue
            if msg is None:
                code = self._proc.poll()
                if code is not None:
                    raise RuntimeError(
                        f"Chrome exited (code {code}) — the DevTools pipe closed"
                    )
                raise RuntimeError("Chrome closed the DevTools pipe")
            if msg.get("id") != mid:
                continue  # event, or another call's reply
            if session_id is not None and msg.get("sessionId") != session_id:
                continue  # same id, different session
            if "error" in msg:
                raise RuntimeError(f"{method} failed: {msg['error']}")
            return msg.get("result", {})

    def close(self):
        stop_chrome(self._proc)
        try:
            os.close(self._write_fd)
        except OSError:
            pass
        try:
            os.close(self._read_fd)
        except OSError:
            pass
        self._reader.join(timeout=5)
        try:
            self._err_file.close()
        except Exception:  # noqa: BLE001
            pass


def install_extension(pipe, extension_dir):
    """Install the unpacked extension with Chrome's
    ``Extensions.loadUnpacked`` CDP command on an ALREADY-OPEN
    pipe and return the installed id.

    WHY NOT --load-extension (BUG-042 follow-up, root cause):
    branded Google Chrome REMOVED --load-extension in v137
    (Chromium RFC, March 2025 — the flag was abused to push
    unwanted software). Chrome 126+ had already made it
    ephemeral (extension accessible but never registered in
    the profile: no service worker, no Preferences entry);
    v137 dropped it outright — the flag is accepted and the
    extension is silently never registered. That is exactly
    what run #2 logged: only component extensions in
    /json/list, nothing in Preferences. Adding
    --enable-unsafe-extension-debugging does NOT restore it
    on branded Chrome 137+ (verified against Chrome 151).
    The supported replacements are Chromium / Chrome for
    Testing (where --load-extension still works) or installing
    through CDP — and this test must run against the branded
    Chrome the user actually has, so it uses the CDP route:
    Extensions.loadUnpacked writes to Secure Preferences
    exactly like clicking "Load unpacked" in
    chrome://extensions, so the extension becomes a real
    install with a normal service-worker lifecycle. The
    command exists only on the pipe transport, gated on
    --remote-debugging-pipe + --enable-unsafe-extension-
    debugging (web-ext's approach since Chrome 126).

    The pipe stays OPEN — the caller does the popup check in
    the SAME launch (see phase_browser). A loadUnpacked
    install does NOT survive a fresh Chrome launch (VPS
    experiments A–D, Round 38: the registration is flushed
    only on a clean shutdown, and a fresh launch then REMOVES
    it — its stored location is the unpacked/load kind, tied
    to the installing session), so a second launch can never
    see the worker. The strict check is unchanged: the
    returned id must equal the stable id derived from
    manifest.json's key — a path-derived id would fail here,
    not later."""
    log("  installing extension via CDP pipe (Extensions.loadUnpacked) — "
        "--load-extension is ignored by branded Chrome 137+")
    pipe.call("Browser.getVersion", timeout=60)  # readiness probe
    result = pipe.call(
        "Extensions.loadUnpacked",
        {"path": os.path.abspath(extension_dir)},
        timeout=60,
    )
    installed_id = result.get("id") if isinstance(result, dict) else None
    if not installed_id:
        raise RuntimeError(
            f"Extensions.loadUnpacked returned no id: {result!r}",
        )
    log(f"  CDP install returned id {installed_id}")
    return installed_id


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
    log("PHASE 2: real browser — headless Chrome + extension + native host (single launch)")
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
    stderr_path = os.path.abspath("chrome-stderr.log")
    pipe = None
    try:
        # ONE Chrome launch, over the CDP pipe, for the whole
        # phase: install via Extensions.loadUnpacked, then open
        # the popup in the SAME launch and poll its status text
        # over the same pipe (Target.createTarget +
        # attachToTarget, flatten, + Runtime.evaluate on that
        # session). There is NO second launch and NO /json/list
        # discovery and NO Preferences fallback — a loadUnpacked
        # install does not survive a fresh Chrome launch (VPS
        # experiments A–D, Round 38), so a second launch can
        # never see the worker, and the install's returned id +
        # the popup handshake are the proof instead. Chrome's
        # stderr goes to a file so a load failure leaves evidence
        # in the uploaded report instead of vanishing into
        # DEVNULL (the old code threw it away).
        pipe = PipeCDP(chrome_exe, profile_dir, stderr_path)
        try:
            installed_id = install_extension(pipe, extension_dir)
        except (TimeoutError, RuntimeError, OSError) as e:
            for line in chrome_stderr_tail(stderr_path, 30):
                log("    chrome stderr: " + line)
            fail("browser", f"could not install the extension via CDP pipe: {e}")
        if installed_id != expected_id:
            fail(
                "browser",
                f"CDP install returned id {installed_id}, not the stable id "
                f"{expected_id} derived from manifest.json's key — the "
                "extension was not installed with its shipped key",
            )
        log("  installed id matches the stable id from manifest.json's key")
        # Open popup.html as a tab in the same launch and poll
        # its status text over the pipe session. The popup's
        # refreshStatus() sends a ping through
        # chrome.runtime.sendNativeMessage to the INSTALLED host,
        # so "Connected" proves Chrome itself can spawn the host
        # — Phase 1 alone only proves the host binary, not
        # Chrome's side of it. (BUG-043: the host used to exit(2)
        # on Chrome's launch arguments — origin +
        # --parent-window — so this step could never pass before
        # that fix; a failure here now means Chrome's spawn, not
        # the install.)
        popup_url = f"chrome-extension://{installed_id}/popup.html"
        try:
            target = pipe.call(
                "Target.createTarget", {"url": popup_url}, timeout=15
            )
            target_id = target.get("targetId")
            if not target_id:
                fail(
                    "browser",
                    f"Target.createTarget for {popup_url} returned no "
                    f"targetId: {target!r}",
                )
            attached = pipe.call(
                "Target.attachToTarget",
                {"targetId": target_id, "flatten": True},
                timeout=15,
            )
            session_id = attached.get("sessionId")
            if not session_id:
                fail(
                    "browser",
                    f"Target.attachToTarget returned no sessionId: "
                    f"{attached!r}",
                )
            status_text = ""
            deadline = time.time() + 20
            while time.time() < deadline:
                result = pipe.call(
                    "Runtime.evaluate",
                    {
                        "expression": (
                            "document.getElementById('status') "
                            "? document.getElementById('status').textContent : ''"
                        ),
                        "returnByValue": True,
                    },
                    session_id=session_id,
                    timeout=15,
                )
                value = result.get("result", {}).get("value", "")
                status_text = value if isinstance(value, str) else ""
                if "Connected to the Sonic Manager desktop app" in status_text:
                    break
                time.sleep(0.5)
        except (TimeoutError, RuntimeError, OSError) as e:
            for line in chrome_stderr_tail(stderr_path, 30):
                log("    chrome stderr: " + line)
            fail("browser", f"popup check failed: {e}")
        log(f"  popup status text: {status_text!r}")
        if "Connected to the Sonic Manager desktop app" not in status_text:
            for line in chrome_stderr_tail(stderr_path, 30):
                log("    chrome stderr: " + line)
            fail("browser", f"popup did not report a connection (status text: {status_text!r}) — Chrome could not reach the host")
        log("PHASE 2 PASSED")
    finally:
        if pipe is not None:
            # Clean shutdown first (Browser.close): Chrome flushes
            # the profile on a clean exit; terminate() alone
            # leaves the registration unflushed (VPS experiment
            # A). Best effort — close() still runs either way.
            try:
                pipe.call("Browser.close", timeout=10)
            except Exception:  # noqa: BLE001
                pass
            pipe.close()
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
