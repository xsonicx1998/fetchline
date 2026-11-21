#!/usr/bin/env python3
"""Validate app/extension/manifest.json (S13 + S08 + S13 finish + Round 72 + Round 73).

Checks the Manifest V3 shape and that the permission set stays
minimal: no tabs, and host permissions ONLY for the two narrow
TikTok patterns (Round 86 — see below). `scripting` is allowed
since S08, and only for one thing — injecting the video detector
into the ACTIVE tab when the user clicks "Find videos on this page"
in the popup (activeTab grants that one tab, on that one gesture).
Round 72 (2026-11-18, user-approved): the detector is ALSO
auto-injected on every http(s) page (`content_scripts`,
`document_idle`) so the floating Download button appears next to
any video without a click, like IDM — the menu-only injection was
the Round 71 video-button root cause (nobody clicked the menu,
so nothing was ever injected). The entry is pinned exactly:
http(s) matches only, `document_idle`, `video-detect.js` +
`content.js` — nothing broader.
Round 73 (2026-11-19): the entry also pins `all_frames` and
`match_about_blank` (both true), so the detector runs inside
iframes and about:blank frames where players embed videos.
The per-site opt-out / switched-off integration check moved
into content.js (a direct chrome.storage.local read, Round 73
— no service-worker round-trip), so an opted-out page still
shows no button.
`cookies` is allowed since the S13 finish, and only for one
thing — reading the cookies for the exact URL being handed over,
so a login-only link can download (see background.js
`cookiesFor`); the extension never reads cookies for any other
URL and never logs them. Firefox needs a stable add-on id
(`browser_specific_settings.gecko.id`) so its host manifest can
name it in `allowed_extensions`. Chrome needs a stable extension
id too: `key` holds the extension's public key (base64 DER), and
Chrome derives the same 32-character id from it on every
machine, so the installer can ship a host manifest with that id
already in `allowed_origins` (see validate_host_manifest.py).
Without `key`, an unpacked load gets a random id on every
machine and no shipped manifest can name it. Run with
`npm run test:extension` from app/, or directly:
    python3 app/extension/tests/validate_manifest.py
Exit code 0 means the manifest is valid; anything else prints the
problems and exits 1.
"""
import base64
import json
import sys
from pathlib import Path

ALLOWED_PERMISSIONS = {"activeTab", "contextMenus", "cookies", "downloads", "storage", "nativeMessaging", "scripting"}
FORBIDDEN_PERMISSIONS = {"tabs", "webRequest", "history", "webNavigation"}

REQUIRED_ICON_SIZES = {"16", "32", "48", "128"}


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    manifest_path = root / "manifest.json"
    problems = []

    try:
        manifest = json.loads(manifest_path.read_text())
    except (OSError, json.JSONDecodeError) as e:
        print(f"FAIL: cannot read {manifest_path}: {e}")
        return 1

    if manifest.get("manifest_version") != 3:
        problems.append("manifest_version must be 3")
    if manifest.get("name") != "Sonic Manager":
        problems.append("name must be Sonic Manager")
    if len(manifest.get("description", "")) <= 10:
        problems.append("description must be a real sentence, not a stub")
    if manifest.get("background", {}).get("service_worker") != "background.js":
        problems.append("background.service_worker must be background.js")
    if manifest.get("action", {}).get("default_popup") != "popup.html":
        problems.append("action.default_popup must be popup.html")

    permissions = manifest.get("permissions", [])
    for p in ALLOWED_PERMISSIONS:
        if p not in permissions:
            problems.append(f"missing permission: {p}")
    for p in permissions:
        if p in FORBIDDEN_PERMISSIONS:
            problems.append(f"permission must not be requested: {p}")
        if p not in ALLOWED_PERMISSIONS:
            problems.append(f"unexpected permission (justify it or remove it): {p}")
    # Round 86 — host_permissions is allowed ONLY for the two
    # narrow TikTok patterns: `chrome.cookies.getAll` needs host
    # permission for the queried URL, and `activeTab` covers only
    # the active tab host (www.tiktok.com), NOT the v16 CDN host
    # — so the R85 cookie refill threw and was swallowed. Anything
    # else — above all <all_urls> — still fails the validation.
    allowed_hosts = {"https://*.tiktok.com/*", "https://*.tiktokcdn.com/*"}
    host_permissions = manifest.get("host_permissions", [])
    if not isinstance(host_permissions, list) or set(host_permissions) != allowed_hosts:
        problems.append(
            "host_permissions must be exactly the two narrow TikTok patterns (Round 86), nothing broader"
        )
    # Round 72 (A) — the auto-injection entry, pinned exactly: one
    # entry, http(s) matches only, document_idle, video-detect.js
    # + content.js. Anything else (another match pattern, another
    # file, another run_at) fails the validation.
    # Round 73 — also pinned: all_frames + match_about_blank,
    # both true, so the detector runs inside iframes and
    # about:blank frames where players embed videos.
    content_scripts = manifest.get("content_scripts")
    if not isinstance(content_scripts, list) or len(content_scripts) != 1:
        problems.append("content_scripts must be exactly one entry (Round 72 auto-injection)")
    else:
        entry = content_scripts[0]
        if entry.get("matches") != ["http://*/*", "https://*/*"]:
            problems.append("content_scripts matches must be exactly http://*/* and https://*/*")
        if entry.get("js") != ["video-detect.js", "content.js"]:
            problems.append("content_scripts js must be exactly video-detect.js + content.js")
        if entry.get("run_at") != "document_idle":
            problems.append("content_scripts run_at must be document_idle")
        if entry.get("all_frames") is not True:
            problems.append("content_scripts all_frames must be true (Round 73)")
        if entry.get("match_about_blank") is not True:
            problems.append("content_scripts match_about_blank must be true (Round 73)")

    icons = manifest.get("icons", {})
    for size in REQUIRED_ICON_SIZES:
        rel = icons.get(size)
        if not rel:
            problems.append(f"missing manifest icon: {size}px")
        elif not (root / rel).is_file():
            problems.append(f"manifest icon file is missing: {rel}")
    popup_icon = manifest.get("action", {}).get("default_icon", {})
    if set(popup_icon) != REQUIRED_ICON_SIZES:
        problems.append("action.default_icon must list 16/32/48/128")

    # Firefox needs a stable add-on id (browser_specific_settings)
    # so the Firefox host manifest can name it in allowed_extensions
    # (see validate_host_manifest.py, BUG-022). A temporary add-on
    # gets a random id on every load, and no host manifest can name
    # that — without this key the Firefox handshake cannot work.
    gecko_id = (
        manifest.get("browser_specific_settings", {}).get("gecko", {}).get("id", "")
    )
    if not isinstance(gecko_id, str) or not gecko_id.strip():
        problems.append(
            "browser_specific_settings.gecko.id must be set (Firefox host manifest names it)"
        )

    # Chrome needs the same stability: `key` is the extension's
    # public key (base64 DER SubjectPublicKeyInfo). Chrome derives
    # the extension id from it, so the id is the same on every
    # machine and the installer can ship a host manifest that
    # already names it (S13 packaging fix). Without `key`, an
    # unpacked load gets a random id and no shipped manifest can
    # name it — the Chrome handshake would never work.
    key = manifest.get("key", "")
    if not isinstance(key, str) or not key.strip():
        problems.append(
            "key must be set (Chrome derives the stable extension id from it)"
        )
    else:
        try:
            der = base64.b64decode(key, validate=True)
        except (ValueError, base64.binascii.Error):
            problems.append("key must be valid base64")
            der = b""
        if der and (len(der) < 100 or der[:2] != b"\x30\x82"):
            problems.append(
                "key must be a DER SubjectPublicKeyInfo (SEQUENCE, RSA-2048 is 294 bytes)"
            )

    for name in ("background.js", "popup.html", "popup.js", "popup.css", "video-detect.js", "content.js"):
        if not (root / name).is_file():
            problems.append(f"missing extension file: {name}")

    if problems:
        print("FAIL: manifest problems:")
        for p in problems:
            print(f"  - {p}")
        return 1
    print("OK: manifest.json is MV3 with the minimal permission set")
    return 0


if __name__ == "__main__":
    sys.exit(main())
