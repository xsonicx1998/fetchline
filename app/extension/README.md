# Fetchline browser extension (S13)

A Manifest V3 WebExtension for Chrome, Edge and Firefox that catches
links and downloads from the browser and hands them to the Fetchline
desktop app. Written fresh for this project — no code, artwork or
wording from any other download manager.

## How the handoff works

```
Browser right-click / download
  -> extension (background.js, service worker)
  -> Native Messaging: 4-byte little-endian length + JSON on stdin/stdout
  -> fetchline-host (app/host/, standalone Rust binary)
  -> pending_links table in the desktop app's SQLite database
  -> desktop app drains the queue (startup + every 30 s, same tick as
     the scheduler) through the REAL add_download path
  -> download appears queued in the main window
```

Messages (see `replica/architecture.md` for the shape):

| message | status |
| --- | --- |
| `download_link {url, referrer, cookies?}` | Implemented — context menu, download watcher. `cookies` (S13 finish) carries the target URL's cookies as name/value pairs, only when the browser has any for that URL; the host attaches them to that download's probe and segment requests for that host only. |
| `download_all {page_url, links[]}` | Implemented, with one deviation: the extension sends `links: []` because it runs no content script and cannot list a page's links itself. The host expands the page with the engine's `fetch_page_links` (fetch + probe) and enqueues what it finds. When links *are* supplied, each is enqueued directly. |
| `video_found {page_url, variants[], cookies?}` | Implemented (S08 / F05) — the extension's video panel sends the single quality the user picked; the host validates each variant (http(s) only, known media extension, dedup, excluded sites, DRM refused) and enqueues it into the same `pending_links` queue with source `"video"`. `cookies` carries the chosen variant URL's cookies, same rules as `download_link`. A message that queues nothing is an error, not `ok: true`. |
| `ping` / `get_settings` | Implemented (not in architecture.md) — the popup's connection check, and the extension reading the desktop `browser_integration` flag and `excluded_sites` list. |

The drain creates each download with `add_download`, so the
excluded-sites refusal, the `file_types_auto` category auto-assign
and the duplicate-path guard (BUG-002) all apply exactly as they do
to a manually added link. A drained link is **started at once**
when `settings.browser_start_drained` is on (the default, since
Milestone 2) — through the normal `start_download`, so the speed
limiter, proxy and segment machinery all apply. Turn
`browser_start_drained` off (Options > General, "Start
browser-caught downloads at once") to get the old behaviour: the
download is left queued for the user to start by hand. If the
start itself fails, the download stays queued and the failure is
recorded in the drain summary's `start_failed` list, not as a
rejection. One bad link never stops the rest of the queue: it is
marked `rejected` with the reason, and the row stays in
`pending_links` as history.

### Download all links on this page

The background worker adds a second context-menu item,
**"Download all links on this page with Fetchline"** (page
context — right-click the page background, not a link). The
extension runs no content script, so it cannot list the page's
links itself: it sends `download_all {page_url, links: []}` and
the host expands the page with the engine's `fetch_page_links`
(fetch + probe, the same code the desktop Download All dialog
uses), then enqueues what it finds. Dedup, excluded-site and
file-type rules apply exactly as in the desktop dialog — a dead
link is skipped at expansion, an excluded page is refused before
it is fetched (BUG-012: the expansion also honours the desktop
browser-integration off switch). No new permission was needed:
`contextMenus` already covers a page-context item, and the host
does the fetching, not the extension. Verified end to end against
the real `fetchline-host` binary on a local page only — see
`app/host/download-all-smoke.py` (14/14: 3 good links enqueued,
duplicate de-duplicated, dead link skipped, excluded page and
integration-off both refused, malformed JSON answered with a
framed error).

### Popup (Round 68, v2)

The popup is deliberately simple: the connection pill
("Connected" / "Not connected", from a `ping` to the host), a
Developer card (Mohand Kandil, with a Facebook link), one note
line about auto-capture, and the version footer. There are no
toggles or buttons any more — catching is governed by the
desktop app's browser-integration setting, per-site opt-outs
live in the desktop Settings, and the background worker still
records every hand-off (`lastHandoff`, `pendingCount`) in
`chrome.storage.local` for its own use. The pill is tested
without a browser (`npm run test:extension-popup`, 3 tests).

### Video grabber (S08 / F05)

The right-click menu's **"Find videos on this page"** item asks
the background worker to inject `video-detect.js` + `content.js` into
the ACTIVE tab only, via `chrome.scripting` + `activeTab` — a user
gesture, nothing automatic. There is no `content_scripts` entry in
the manifest, no host permission and no `<all_urls>`, so no page
is ever scanned unprompted. Excluded sites and a switched-off
desktop flag refuse the scan before anything is injected.

On the page, `content.js` shows a Sonic-branded floating
"Download" button at the top right of a video player that has a
downloadable stream, opening a small dark panel that
lists that video's qualities/formats (MP4 / WebM / FLV / MP3
where actually available). Picking one sends `video_found` with
`variants: [chosen]` — only the chosen variant, so the desktop
app queues exactly what the user picked. Quality is never
invented: it comes from the element's real `videoWidth` /
`videoHeight`, a `720p` / `1080p` / `4K` label in the URL or file
name, or the literal "Unknown quality". A DRM-protected video, or
one with no downloadable stream, shows an honest "can't be
downloaded" note and NO download button. A live stream's `.m3u8`
is offered as-is, labelled as a playlist file — HLS/DASH
segmented streams are never merged into a single video file, and
no cookies are ever sent.

The detector (`video-detect.js`) is shared pure logic, tested
without a browser: `npm run test:extension-video` runs it over
the fixtures in `tests/fixtures/` (video with several qualities,
audio-only, no video, duplicates, live stream, DRM-marked video).
The Rust engine mirrors the same rules in
`engine/src/video.rs`, and its tests assert the same fixture
counts, so the two detectors cannot drift apart silently.

### Three layers of exclusion (defence in depth)

1. **Extension** — the download watcher and the context menu skip
   sites in the extension-local opt-out list (popup toggle) and in
   the desktop list it last fetched.
2. **Host boundary** — `handle_host_message` (engine,
   `app/engine/src/native_host.rs`) refuses an excluded link or a
   link sent while desktop browser integration is off, before
   anything is enqueued. The store's `enqueue_pending_link` refuses
   excluded sites too.
3. **Drain** — `drain_pending_links` re-checks the flag and the list
   (a row enqueued before the setting changed must not slip
   through), and `add_download` checks exclusion a third time
   before any probe.

### Direction of truth: the desktop setting wins

`settings.browser_integration` (Options > General in the desktop
app) is the master switch. The host reports it, the popup shows it
as the connection state, and when it is off the extension must not
catch — the background worker skips interception. The extension's
own catch toggle (default ON, stored in `chrome.storage.local`)
only matters while the desktop flag is on; since popup v2 (Round
68) it has no popup UI any more, so it stays at whatever it was
last set to (ON for a fresh install). Per-site opt-outs live in
two places: the desktop `excluded_sites` list (edited in
Settings) and the extension-local list (no popup UI since v2 —
an earlier opt-out stays in force); either one opting a site out
is enough to skip it.

### Privacy

- Cookies ARE sent now (S13 finish), but only under strict rules:
  the extension reads the cookies for the exact URL being handed
  over — nothing else, no other site, no other page — and sends
  only their name/value pairs (no domain, path, expiry or flags).
  The host attaches them as a `Cookie` header on that download's
  probe and segment requests, for that host only. They are never
  logged (the host never logs a message body, and no reply ever
  carries a cookie value), never written to settings, and wiped
  from the `pending_links` row the moment the drain decides the
  link; the desktop app keeps them in memory only, so a restart
  drops them and a paused download resumed after a restart fails
  with a clear 401 instead of silently reusing a stale session.
  A `download_all` page expansion carries no cookies — the host
  fetches the page itself, so a login-gated page cannot be
  expanded; right-click the individual links instead, which do
  carry their cookies.
- The extension requests no host permissions. It injects the video
  detector into a page only when the user picks "Find videos on
  this page" in the right-click menu (activeTab + scripting, that
  one tab, that one gesture); it never reads page content unprompted.
- The host never logs a message body — only the message type goes
  to stderr.

## Permissions — each one justified

| permission | why |
| --- | --- |
| `activeTab` | The video scan (right-click menu) needs the current tab. Granted only on that gesture — no standing access to tabs. |
| `contextMenus` | The "Download with Fetchline" right-click item on links and media elements. |
| `downloads` | The download watcher: cancel a browser download that Fetchline is taking over (`downloads.cancel` / `erase`). |
| `storage` | Persist the catch toggle and the per-site opt-out list, and cache the desktop settings the host last reported. |
| `cookies` | Read the cookies for the exact URL being handed over (S13 finish), so a login-only link can download. Only name/value pairs are sent, only for that URL, never logged, never stored past the drain — see Privacy above. |
| `nativeMessaging` | Talk to `fetchline-host` over stdin/stdout. |
| `scripting` | Inject the video detector (`video-detect.js` + `content.js`) into the ACTIVE tab only, and only when the user picks "Find videos on this page" in the right-click menu. `activeTab` grants that one tab, on that one gesture — there is still no `content_scripts` entry and no host permission, so no page is ever scanned without the user asking. |

Not requested, on purpose: `tabs` (only
the active tab, via `activeTab`), host permissions / `<all_urls>`
(no standing page access), `webRequest` (downloads are observed, never intercepted at the network layer).

## Install — load the extension

Chrome / Edge (developer mode):

1. Open `chrome://extensions` (Edge: `edge://extensions`), turn on
   **Developer mode**.
2. **Load unpacked** and pick this folder (`app/extension/`).
3. The extension ID shown on the card is stable:
   `mioamdgogndabpcjbdekggcnlkkfpmbj`. It is derived from the
   public `key` in `manifest.json`, so it is the same on every
   machine — the installer ships a host manifest that already
   names it, and nothing has to be replaced. (If the card shows a
   different ID, the extension was not loaded from this folder.)

Firefox:

1. Open `about:debugging#/runtime/this-firefox`, **Load Temporary
   Add-on**, pick `manifest.json` in this folder. (Temporary: it
   is removed when Firefox closes. A signed build is needed for a
   permanent install.)
2. Firefox uses the same `com.fetchline.host` name but a different
   host-manifest FILE (`com.fetchline.host.firefox.json` — see
   below) and a different location. A temporary add-on gets a
   random id on every load, so the Firefox handshake only works
   with a signed build (or a fixed id): the manifest declares
   `browser_specific_settings.gecko.id` (`fetchline@fetchline.app`)
   and the Firefox host manifest names that id in
   `allowed_extensions`.

## Install — the native host (desktop side)

The extension alone can do nothing: it needs `fetchline-host`
talking to the same SQLite file the desktop app uses.

**On Windows, the installer does all of this for you.** The MSI
and NSIS installers ship `fetchline-host.exe` and both host
manifests next to the app binary, and write the three registry
keys that point the browsers at them (see the checklist below).
Install Fetchline, load the extension as above, and capture
works — no manual steps. The rest of this section is for
development builds and non-Windows machines only.

### 1. Build the host (dev only)

```sh
cargo build --release -p fetchline-host   # from app/host/
# binary: app/host/target/release/fetchline-host[.exe]
```

### 2. Point the host manifest at the binary (dev only)

Copy `com.fetchline.host.json` (Chrome / Edge) or
`com.fetchline.host.firefox.json` (Firefox) to the location for
your browser and edit two fields:

- `path` — the absolute path to the `fetchline-host` binary.
- Chrome / Edge: `allowed_origins` already holds the stable
  extension id
  (`chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/`,
  derived from the `key` in `manifest.json`) — do not change it.
  Firefox: `allowed_extensions` already holds the add-on id
  (`fetchline@fetchline.app`, from the extension manifest's
  `browser_specific_settings.gecko.id`) — do not put a
  `chrome-extension://…` origin there; Firefox rejects
  `allowed_origins` outright (BUG-022).

### 3. Host-manifest locations

**Windows (Chrome):** registry key
`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host`
(default value = full path to the manifest JSON file).
Edge uses
`HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host`.
Firefox uses a registry key too:
`HKCU\Software\Mozilla\NativeMessagingHosts\com.fetchline.host`.

**Linux dev VM (Chrome):** `~/.config/google-chrome/NativeMessagingHosts/com.fetchline.host.json`
Chromium: `~/.config/chromium/NativeMessagingHosts/`
Edge on Linux: `~/.config/microsoft-edge/NativeMessagingHosts/`
Firefox: `~/.mozilla/native-messaging-hosts/com.fetchline.host.json`

**macOS (Chrome):**
`~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`

The host finds the database itself (`--db-path <path>` or
`FETCHLINE_DB_PATH` override it): it tries the Tauri app-data
candidates first (`<app-data>/com.fetchline.app/fetchline/app.db`,
then `<app-data>/fetchline/app.db`). If your desktop app was built
with a different identifier, point the host at the file
explicitly — a host pointed at the wrong file silently queues
links the app never sees.

## Verify end-to-end

1. Start the Fetchline desktop app once, so its database exists.
2. Open the extension popup: it should say **"Connected"**. If it
   says "Not connected", the host manifest path, the extension ID
   in `allowed_origins`, or the binary path is wrong — check the
   browser's extension error page first.
3. Right-click any link to a `.zip` / `.pdf` / `.mp4` file and
   choose **Download with Fetchline**. The Add URL dialog opens
   in the desktop app with the link pre-filled (Round 67) — pick
   Start Download, and the file downloads. (With "Start
   browser-caught downloads at once" off in Options > General, it
   appears queued instead.)
4. Right-click the page background and choose **Download all
   links on this page with Fetchline**: every downloadable link
   on the page is queued (dead links skipped), and they start
   downloading the same way.
5. Start a download of a `.zip` in the browser with catching on:
   the browser download is cancelled and the file appears in
   Fetchline instead. Turn browser integration off in the desktop
   app and the browser downloads normally again.
6. Add a site to the desktop app's excluded-sites list (Settings)
   and repeat step 5 on that site: the browser download proceeds
   untouched.
7. Turn browser integration off in the desktop app (Options >
   General): catching stops. The popup still says "Connected"
   (the host is reachable) — integration is a separate setting.
8. Right-click a page with a video and choose **Find videos on
   this page**: a "Download" button appears at the top right of
   the video; click it, pick a quality, and the Add URL dialog
   opens with that stream pre-filled.

Without a browser handy, the host can be exercised directly — see
`app/engine/tests/extension_tests.rs` (framing, host boundary,
drain, cookies) and `app/engine/tests/video_tests.rs` (video
validation + end-to-end drain) and the smoke tests
`app/host/video-smoke.py` (real binary, local .mp4 fixture),
`app/host/download-all-smoke.py` (real binary, local page with
links — the Download All path) and `app/host/cookie-smoke.py`
(real binary, cookie-gated fixture):
`python3 app/host/download-all-smoke.py <path to fetchline-host> <workdir>`.
The video detector is tested without a browser:
`npm run test:extension-video` (Node, same fixtures as the Rust
tests). The popup and background worker are tested without a
browser too: `npm run test:extension-popup` and
`npm run test:extension-background` (Node, fake `chrome` API),
and the download watcher with a simulated downloads stream:
`npm run test:extension-watcher` (Node, fake `chrome.downloads`
events — downloadable link, excluded site, integration off, type
outside the catch list, duplicate URL, cookies attached).
`npm run test:extension` validates both manifests and both host
manifests (`validate_manifest.py`,
`validate_host_manifest.py`).

## Windows verification checklist (installer now ships the host + keys)

The desktop installer now ships `fetchline-host.exe` and both
host manifests, and writes the three registry keys (MSI: WiX
fragment; NSIS: installer hooks). The CI `extension-capture-test`
job proves items 1–3 on the Windows runner on every push: it
reads the installed key + manifest, checks they point at the
installed host with the stable extension id, loads the unpacked
extension in headless Chrome, and verifies the popup reports a
connection and the app drains the queue and downloads the pinned
file byte-for-byte. What is still marked **needs-Windows** below
is what a headless runner cannot do — a human right-click, the
download watcher, and the off switches in a real browser.
Work through them in order on a Windows machine with the desktop
app installed from its real installer:

1. **Registry key** — **proven by CI** (verify by hand with
   `reg query`). The installer writes
   `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host`
   (Edge: `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host`;
   Firefox: `HKCU\Software\Mozilla\NativeMessagingHosts\com.fetchline.host`)
   whose default value is the full path to the installed
   `com.fetchline.host.json` (Firefox: the `.firefox.json` file),
   next to the installed `fetchline-host.exe`. Confirm the keys
   exist with `reg query` and that the JSON file each points at is
   readable. Uninstalling removes all three keys.
   **Verified-by-inspection:** the key paths and the default-value
   rule match Chrome's docs exactly (HKCU or HKLM under
   `SOFTWARE\Google\Chrome\NativeMessagingHosts\<name>`); Edge
   substitutes `Microsoft\Edge`, Firefox `Mozilla`.
2. **Manifest contents** — **proven by CI** (verify by hand if
   needed). In the installed JSON, `path` must be the absolute
   path to the installed `fetchline-host.exe` and
   `allowed_origins` must contain exactly
   `chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/` — the
   stable id derived from the `key` in `manifest.json`. There is
   no placeholder any more: the repo copy names the real id, and
   `validate_host_manifest.py` FAILS on the old
   `__FETCHLINE_EXTENSION_ID__` placeholder. A wrong ID is the
   most common failure: the popup then says the desktop app is
   not reachable.
   **Verified-by-inspection:** `npm run test:extension` runs
   `validate_host_manifest.py`, which checks the repo copies —
   name `com.fetchline.host`, type `stdio`, a Windows absolute
   `path` ending in `fetchline-host.exe`, and every
   `allowed_origins` entry matching
   `chrome-extension://[a-z]{32}/` (32 lowercase a–p characters,
   trailing slash mandatory, no wildcards — the format Chrome's
   docs require) and equal to the id derived from
   `manifest.json`'s `key`.
   **Firefox is different** (BUG-022, fixed in the S13 finish):
   Firefox rejects `allowed_origins` — its allow-list key is
   `allowed_extensions`, holding the add-on id from the
   extension's `browser_specific_settings.gecko.id`
   (`fetchline@fetchline.app`), not a `chrome-extension://…`
   origin. The repo now ships a second file,
   `com.fetchline.host.firefox.json`, for Firefox, and the
   validator checks that it uses `allowed_extensions` and that
   the id matches `manifest.json`. The old README said Firefox
   takes the same file in a different location — that would never
   have connected.
3. **App-data path** — **needs-Windows** (with the candidate order
   **verified-by-inspection**). Start the desktop app once and confirm
   which SQLite file it actually opens (expected:
   `%APPDATA%\com.fetchline.app\fetchline\app.db` — the Tauri
   app-data folder plus `fetchline/app.db`; the host tries that
   first, then `%APPDATA%\fetchline\app.db`). Send a link from
   the browser and check the row lands in THAT file's
   `pending_links` — a host pointed at a different file silently
   queues links the app never sees. If the installed identifier
   differs, point the host at the file explicitly with
   `--db-path` / `FETCHLINE_DB_PATH`.
   **Verified-by-inspection:** `app/host/src/main.rs`
   `db_candidates()` lists exactly these two `%APPDATA%`
   candidates in this order (the code reads the `APPDATA`
   environment variable, which is how the path is spelled on
   Windows), and the host takes the first candidate that exists.
4. **First live handoff** — **needs-Windows.** Right-click a link and choose
   "Download with Fetchline": the popup's pending count should
   rise to 1 and the last hand-off read "Sent", then the desktop
   app should start the download within 30 s (or on its next
   start) — with "Start browser-caught downloads at once" off,
   it should appear queued instead.
5. **Download All** — **needs-Windows.** Right-click a page background and choose
   "Download all links on this page": the page's links start
   downloading (dead links skipped). Confirm the count in the
   popup matches the links enqueued.
6. **Off switches** — **needs-Windows.** Turn browser integration off in the desktop
   app: the popup toggle disables itself and catching stops.
   Opt a site out in the popup: that site's downloads proceed in
   the browser untouched.
7. **No leftover dev state** — **needs-Windows.** Uninstall the dev-loaded unpacked
   extension and the dev host manifest (Linux paths under
   `~/.config/...` do not apply on Windows); the installed
   extension + registry key are the only handshake left.

## Honest limits

- **No run in a real browser by a human yet.** The Windows
  installers are built by CI, the runner smoke test proves the
  app launches and stays alive, and the CI capture test proves
  the installed host + registry keys work end to end in headless
  Chrome (popup connects, queue drains, file downloads
  byte-for-byte). What is still unproven is a human right-click
  ("Download with Fetchline"), the download watcher, and the off
  switches in a real browser on a user's machine — see the
  checklist above for exactly what still needs proving.
- **Cookies: sent, with limits.** A login-only link now downloads
  with the browser's session cookie for that URL (see Privacy
  above) — but a paused download resumed after an app restart
  loses its cookies and fails with a clear 401 (cookies live in
  memory only), and a login-gated page cannot be expanded by
  Download All (no cookies on the host's page fetch — right-click
  the individual links). Site logins (S09 tab) are not built
  either.
- **HLS playlists are saved as-is.** An `.m3u8` is offered with a
  note saying it is a playlist file, not a merged video; merging
  segmented streams into one file is out of scope.
- **The extension cannot launch the desktop app.** The popup's
  "Check the desktop app" button only checks the connection; if
  Fetchline is not running, open it from the Start menu.
- **Download All needs the host.** The extension sends only the
  page URL; the host fetches the page itself, so a page that
  blocks non-browser clients will fail the same way the desktop
  Download All dialog would.
