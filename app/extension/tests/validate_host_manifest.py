#!/usr/bin/env python3
"""Validate the native-messaging host manifests (S13 finish).

Checks `com.fetchline.host.json` (Chromium family: Chrome, Edge)
and `com.fetchline.host.firefox.json` (Firefox family) in
app/extension/ against the rules Chrome and Firefox actually
enforce, verified against Chrome's native-messaging docs
(developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
and Firefox's (MDN: Native messaging):

  * both files are valid JSON with name "com.fetchline.host",
    type "stdio", a non-empty description, and a Windows absolute
    path to fetchline-host.exe (the installer edits `path` to the
    installed location);
  * the Chromium file lists callers in `allowed_origins`, one entry
    per extension, each exactly `chrome-extension://<id>/` — the
    id is 32 lowercase a-p characters and the trailing slash is
    mandatory; no wildcards are allowed. The id is the stable one
    derived from the public `key` in manifest.json (Chrome hashes
    the key, so the id is the same on every machine) — the file
    must name exactly that id, and the installer ships it as-is;
    the `__FETCHLINE_EXTENSION_ID__` placeholder must never
    survive (it is a FAILURE now that the id is stable — the
    installer no longer replaces anything). A wrong id is the most
    common handshake failure (the popup then says the desktop app
    is not reachable).
  * the Firefox file lists callers in `allowed_extensions`, NOT
    `allowed_origins` — Firefox rejects a manifest that contains
    `allowed_origins` instead; the keys are not interchangeable,
    so the two files must stay separate (BUG-022: the README used
    to say Firefox takes the same file, which would never
    connect). Each entry is the add-on id from
    manifest.json's `browser_specific_settings.gecko.id`.

Run with `npm run test:extension` from app/, or directly:
    python3 app/extension/tests/validate_host_manifest.py
Exit code 0 means both manifests are valid; anything else prints
the problems and exits 1.
"""
import base64
import hashlib
import json
import re
import sys
from pathlib import Path

HOST_NAME = "com.fetchline.host"
# A Chrome extension id is 32 characters drawn from a-p (it is the
# first 32 characters of the public key's hash, base32-encoded).
CHROME_ORIGIN_RE = re.compile(r"^chrome-extension://[a-p]{32}/$")
PLACEHOLDER_ORIGIN = "chrome-extension://__FETCHLINE_EXTENSION_ID__/"
WINDOWS_EXE_RE = re.compile(r"^[A-Za-z]:\\.+fetchline-host\.exe$")


def chrome_id_from_key(key_b64):
    """Chrome's extension id: SHA-256 of the DER public key, first
    16 bytes, each nibble mapped to a-p. Returns None if the key
    is not usable."""
    try:
        der = base64.b64decode(key_b64, validate=True)
    except (ValueError, base64.binascii.Error):
        return None
    digest = hashlib.sha256(der).digest()[:16]
    return "".join(
        chr(ord("a") + ((b >> 4) & 0xF)) + chr(ord("a") + (b & 0xF))
        for b in digest
    )


def load(root, name, problems):
    path = root / name
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as e:
        problems.append(f"cannot read {path}: {e}")
        return None


def check_common(manifest, name, problems):
    if manifest.get("name") != HOST_NAME:
        problems.append(f"{name}: name must be {HOST_NAME}")
    if manifest.get("type") != "stdio":
        problems.append(f"{name}: type must be stdio")
    if not manifest.get("description", "").strip():
        problems.append(f"{name}: description must not be empty")
    path = manifest.get("path", "")
    if not isinstance(path, str) or not WINDOWS_EXE_RE.match(path):
        problems.append(
            f"{name}: path must be a Windows absolute path to fetchline-host.exe, got {path!r}"
        )


def check_chromium(root, problems, warnings):
    manifest = load(root, "com.fetchline.host.json", problems)
    if manifest is None:
        return
    check_common(manifest, "com.fetchline.host.json", problems)
    if "allowed_extensions" in manifest:
        problems.append(
            "com.fetchline.host.json: allowed_extensions belongs in the Firefox file, not here"
        )
    # The id must be the stable one derived from the public key in
    # manifest.json — that is the id Chrome assigns to the
    # unpacked extension on every machine, and the id the
    # installer ships. Anything else (including the old
    # placeholder) can never connect.
    expected_id = None
    try:
        ext = json.loads((root / "manifest.json").read_text())
        expected_id = chrome_id_from_key(ext.get("key", ""))
    except (OSError, json.JSONDecodeError) as e:
        problems.append(f"cannot read manifest.json for the Chrome id: {e}")
    if expected_id is None:
        problems.append(
            "com.fetchline.host.json: cannot derive the expected Chrome id "
            "from manifest.json's key — fix the key first"
        )
    origins = manifest.get("allowed_origins")
    if not isinstance(origins, list) or not origins:
        problems.append("com.fetchline.host.json: allowed_origins must be a non-empty list")
        return
    for origin in origins:
        if origin == PLACEHOLDER_ORIGIN:
            problems.append(
                "com.fetchline.host.json: allowed_origins still carries the "
                "__FETCHLINE_EXTENSION_ID__ placeholder — the id is stable "
                "now (manifest.json key), so the file must name the real id"
            )
        elif not CHROME_ORIGIN_RE.match(origin):
            problems.append(
                f"com.fetchline.host.json: allowed_origins entry {origin!r} is not "
                "chrome-extension://<32 lowercase a-p chars>/ (trailing slash mandatory, no wildcards)"
            )
        elif expected_id is not None and origin != f"chrome-extension://{expected_id}/":
            problems.append(
                f"com.fetchline.host.json: allowed_origins entry {origin!r} is not "
                f"the stable id from manifest.json's key (chrome-extension://{expected_id}/)"
            )


def check_firefox(root, problems, warnings):
    manifest = load(root, "com.fetchline.host.firefox.json", problems)
    if manifest is None:
        return
    check_common(manifest, "com.fetchline.host.firefox.json", problems)
    if "allowed_origins" in manifest:
        problems.append(
            "com.fetchline.host.firefox.json: Firefox rejects allowed_origins — "
            "its allow-list key is allowed_extensions (BUG-022)"
        )
    # The Firefox allow-list must name the add-on id the extension
    # declares in manifest.json, or nothing can connect.
    try:
        ext = json.loads((root / "manifest.json").read_text())
        gecko_id = ext.get("browser_specific_settings", {}).get("gecko", {}).get("id", "")
    except (OSError, json.JSONDecodeError) as e:
        problems.append(f"cannot read manifest.json for the Firefox id: {e}")
        return
    allowed = manifest.get("allowed_extensions")
    if not isinstance(allowed, list) or not allowed:
        problems.append(
            "com.fetchline.host.firefox.json: allowed_extensions must be a non-empty list"
        )
        return
    for entry in allowed:
        if entry != gecko_id:
            problems.append(
                f"com.fetchline.host.firefox.json: allowed_extensions entry {entry!r} "
                f"must be the extension's Firefox id {gecko_id!r} from manifest.json"
            )
    if not gecko_id:
        warnings.append(
            "com.fetchline.host.firefox.json: manifest.json sets no gecko id, "
            "so the Firefox allow-list cannot be checked"
        )


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    problems = []
    warnings = []
    check_chromium(root, problems, warnings)
    check_firefox(root, problems, warnings)
    for w in warnings:
        print(f"WARN: {w}")
    if problems:
        print("FAIL: host manifest problems:")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("OK: host manifests are valid (Chrome: allowed_origins, Firefox: allowed_extensions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
