#!/usr/bin/env python3
"""Tests for the PipeCDP transport in scripts/capture-test.py.

Covers the two platform launch contracts (CPython's subprocess
docs + Lib/subprocess.py, Chromium's pipe_builder.cc):

* POSIX: Chrome reads commands on fd 3 and writes replies on
  fd 4; the child ends are kept open with ``pass_fds``.
* Windows: CPython rejects ``pass_fds`` outright
  (``assert not pass_fds, "pass_fds not supported on
  Windows."`` — the run #3 crash). The child inherits the two
  pipe HANDLEs because they are flagged inheritable and the
  process is created with bInheritHandles (Popen with
  ``close_fds=False``); inherited HANDLEs keep their numeric
  value in the child, which is what
  ``--remote-debugging-io-pipes`` names.

The Windows branch cannot run for real on this Linux VM, so
its tests drive the real ``PipeCDP.__init__`` code path with
``os.name`` patched to ``"nt"``, a fake ``msvcrt`` module, and
a Popen mock that raises exactly as CPython does on Windows
whenever ``pass_fds`` is passed. The POSIX tests drive the
real transport against a fake Chrome peer (a small script
speaking the NUL-separated JSON protocol on fds 3/4).

Run: python3 scripts/test_capture_pipe.py
"""

import importlib.util
import os
import re
import stat
import sys
import tempfile
import types
import unittest
from unittest import mock

_HERE = os.path.dirname(os.path.abspath(__file__))
_SPEC = importlib.util.spec_from_file_location(
    "capture_test", os.path.join(_HERE, "capture-test.py")
)
ct = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(ct)

# Fake Chrome peer: ignores its argv, speaks the CDP pipe
# protocol on fds 3 (commands in) and 4 (replies out). The
# scenario comes from PIPE_PEER_SCENARIO.
PEER_SCRIPT = """#!/usr/bin/env python3
import json
import os
import sys
import time

scenario = os.environ.get("PIPE_PEER_SCENARIO", "normal")


def read_msg():
    buf = b""
    while b"\\0" not in buf:
        chunk = os.read(3, 65536)
        if not chunk:
            return None
        buf += chunk
    raw, _rest = buf.split(b"\\0", 1)
    return json.loads(raw.decode("utf-8"))


def send(obj):
    os.write(4, json.dumps(obj).encode("utf-8") + b"\\0")


if scenario == "exit_early":
    sys.exit(0)

cmd = read_msg()
if cmd is None:
    sys.exit(0)
mid = cmd.get("id")
if scenario == "eventful":
    send({"method": "Test.event", "params": {}})
    send({"id": mid, "result": {"version": "1"}})
elif scenario == "chunked":
    payload = json.dumps({"id": mid, "result": {"version": "1"}}).encode("utf-8") + b"\\0"
    half = len(payload) // 2
    os.write(4, payload[:half])
    time.sleep(0.2)
    os.write(4, payload[half:])
elif scenario == "error":
    send({"id": mid, "error": {"code": -32000, "message": "boom"}})
else:  # normal
    send({"id": mid, "result": {"version": "1"}})
# Stay alive until the parent closes the pipe (or is killed).
try:
    while read_msg() is not None:
        pass
except OSError:
    pass
"""


class FakeProc:
    """Stand-in for a Popen object: never "running" checks fail."""

    def __init__(self):
        self.returncode = None
        self.terminated = False

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = -15

    def wait(self, timeout=None):
        return self.returncode

    def kill(self):
        self.returncode = -9


class WindowsBranchTests(unittest.TestCase):
    """The run #3 regression: pass_fds must never reach Popen
    on Windows, and the handles must be inherited instead."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fl-pipe-test-")
        self.stderr_path = os.path.join(self.tmp, "stderr.log")
        fake_msvcrt = types.ModuleType("msvcrt")
        # Stable fake HANDLE values, derived from the fd so the
        # test can tell the two pipe ends apart.
        fake_msvcrt.get_osfhandle = lambda fd: fd + 1000
        self.fake_msvcrt = fake_msvcrt

    def tearDown(self):
        import shutil

        shutil.rmtree(self.tmp, ignore_errors=True)

    def _make_pipe(self):
        """Build a PipeCDP with os.name faked to nt and Popen
        mocked to behave like CPython on Windows: it raises
        AssertionError("pass_fds not supported on Windows.")
        if pass_fds is passed at all."""
        calls = []
        fake_proc = FakeProc()

        def fake_popen(*args, **kwargs):
            calls.append((args, kwargs))
            if "pass_fds" in kwargs:
                raise AssertionError("pass_fds not supported on Windows.")
            return fake_proc

        patchers = [
            mock.patch.dict(sys.modules, {"msvcrt": self.fake_msvcrt}),
            mock.patch.object(os, "name", "nt"),
            mock.patch.object(ct.subprocess, "Popen", side_effect=fake_popen),
        ]
        for p in patchers:
            p.start()
        self.addCleanup(lambda: [p.stop() for p in reversed(patchers)])
        pipe = ct.PipeCDP(
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
            self.tmp,
            self.stderr_path,
        )
        return pipe, calls, fake_proc

    def test_windows_launch_never_passes_pass_fds(self):
        pipe, calls, _proc = self._make_pipe()
        try:
            self.assertEqual(len(calls), 1)
            _args, kwargs = calls[0]
            self.assertNotIn("pass_fds", kwargs)
            # close_fds=False is what makes CreateProcess pass
            # bInheritHandles, so the inheritable pipe HANDLEs
            # reach the child with their values intact.
            self.assertIs(kwargs.get("close_fds"), False)
            flat = " ".join(str(a) for a in _args[0])
            m = re.search(r"--remote-debugging-io-pipes=(\d+),(\d+)", flat)
            self.assertIsNotNone(
                m, "--remote-debugging-io-pipes missing from Chrome args"
            )
            self.assertNotEqual(m.group(1), m.group(2))
        finally:
            pipe.close()

    def test_windows_launch_fails_loud_when_handles_not_inheritable(self):
        # set_inheritable neutered: the handles stay
        # non-inheritable (PEP 446 default), so the setup must
        # refuse to launch and say so, naming the platform —
        # instead of starting a Chrome with no DevTools pipe.
        calls = []
        fake_proc = FakeProc()

        def fake_popen(*args, **kwargs):
            calls.append((args, kwargs))
            return fake_proc

        with (
            mock.patch.dict(sys.modules, {"msvcrt": self.fake_msvcrt}),
            mock.patch.object(os, "name", "nt"),
            mock.patch.object(os, "set_inheritable", lambda fd, flag: None),
            mock.patch.object(ct.subprocess, "Popen", side_effect=fake_popen),
        ):
            with self.assertRaises(RuntimeError) as ctx:
                ct.PipeCDP(
                    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
                    self.tmp,
                    self.stderr_path,
                )
            self.assertIn("not inheritable", str(ctx.exception))
            self.assertIn("nt", str(ctx.exception))
        self.assertEqual(calls, [], "Chrome must not launch on a bad setup")


class PosixPeerTests(unittest.TestCase):
    """Drive the real POSIX transport (fds 3/4 + pass_fds)
    against a fake Chrome peer."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fl-pipe-test-")
        self.peer = os.path.join(self.tmp, "fake-chrome.py")
        with open(self.peer, "w") as f:
            f.write(PEER_SCRIPT)
        os.chmod(self.peer, os.stat(self.peer).st_mode | stat.S_IXUSR)
        self.stderr_path = os.path.join(self.tmp, "stderr.log")

    def tearDown(self):
        import shutil

        shutil.rmtree(self.tmp, ignore_errors=True)

    def _make_pipe(self, scenario):
        env = dict(os.environ, PIPE_PEER_SCENARIO=scenario)
        with mock.patch.dict(os.environ, env, clear=False):
            # The peer inherits the scenario through the
            # environment (Popen passes the parent env through).
            pipe = ct.PipeCDP(self.peer, self.tmp, self.stderr_path)
        return pipe

    def test_normal_reply(self):
        pipe = self._make_pipe("normal")
        try:
            result = pipe.call("Browser.getVersion", timeout=10)
            self.assertEqual(result.get("version"), "1")
        finally:
            pipe.close()

    def test_event_skipped_before_reply(self):
        pipe = self._make_pipe("eventful")
        try:
            result = pipe.call("Browser.getVersion", timeout=10)
            self.assertEqual(result.get("version"), "1")
        finally:
            pipe.close()

    def test_chunked_reply_reassembled(self):
        pipe = self._make_pipe("chunked")
        try:
            result = pipe.call("Browser.getVersion", timeout=10)
            self.assertEqual(result.get("version"), "1")
        finally:
            pipe.close()

    def test_error_reply_raises(self):
        pipe = self._make_pipe("error")
        try:
            with self.assertRaises(RuntimeError) as ctx:
                pipe.call("Browser.getVersion", timeout=10)
            self.assertIn("boom", str(ctx.exception))
        finally:
            pipe.close()

    def test_peer_exits_early_raises(self):
        pipe = self._make_pipe("exit_early")
        try:
            with self.assertRaises(RuntimeError) as ctx:
                pipe.call("Browser.getVersion", timeout=10)
            self.assertIn("exited", str(ctx.exception))
        finally:
            pipe.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
