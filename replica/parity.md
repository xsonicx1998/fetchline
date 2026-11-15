# Parity: 76.4 / 100 — Fetchline vs IDM

Fresh full replica-diff pass, 2026-11-15 (final verification).
This report is a complete rewrite, not a patch on the previous
one: the old report scored the pre-finish state (58.3) and its
prose described partials that are now finished.

**Feature score: 76.4 / 100** (30 rows counted, 4 skip excluded).
**Must-haves: 16 of 17 done** (1 more partial, 0 missing entirely).
**Verdict: NOT SHIPPABLE** — 1 must-have is not done (proxy /
Kerberos, see below). No open S1 or S2 bug blocks it; the verdict
is decided by that one must-have alone.

## What changed since the last comparison (66.7, 2026-11-15)

Score delta: **+9.7 points** (66.7 → 76.4). Must-haves done
**13 → 16 of 17**.

- **Browser integration 50.0 → 100.0.** S13 finished: cookie
  forwarding end to end (extension reads cookies only for the
  exact handed-over URL, host validates and stores them on the
  pending row, engine sends them on probe + segments for that
  host only), download watcher fixed (BUG-024), Firefox host
  manifest added (BUG-023).
- **Queues 87.5 → 100.0.** Periodic synchronisation finished:
  Last-Modified-only, ETag-only and size-only change detection
  all fixture-proven, interval boundaries pinned (5 min–1 week),
  empty queue, mid-download skip, shrunk file.
- **Settings 40.0 → 70.0.** Options dialog finished to all 9
  tabs (Site logins, Dial-Up, Sounds); proxy finished to FTP +
  NTLMv2 — but see the verifier revert below, which is why
  settings stops at 70.0 instead of 85.0.
- **Media 50.0 → 100.0, downloads 71.1 → 78.9.** The video
  grabber, site grabber and progress dialog finishes from the
  66.7 pass are now all scored `yes` (they were already flipped
  when 66.7 was computed; the score gain here is the four new
  finishes, not them).
- **Bugs: 0 open.** BUG-001–025 are all FIXED. Three new bugs
  were found in the four finish rounds (BUG-022 NTLM stub,
  BUG-023 Firefox manifest, BUG-024 watcher never fired) and one
  by this verification pass (BUG-025 page-scan probes never sent
  a saved site login — fixed failing-test-first, 4 kept tests).
- **Test suite: 330 / 330 pass, 0 failed, twice consecutively**
  (35 unit + 111 integration + 32 extension + 4 final-verify2 +
  16 final-verify + 28 grabber + 14 verifier + 14 options +
  7 progress + 15 proxy + 23 sync + 31 video; was 271 — +59).
  `npm run build` passes; extension JS 44/44; host smoke
  video 13/13, download-all 14/14, cookie 14/14 on a freshly
  rebuilt binary; `cargo check --tests` 0 warnings (3 test-only
  warnings in `proxy_tests.rs` found and fixed in this pass).

### Verifier revert: proxy is `partial`, not `yes`

The proxy worker flipped row 18 to `yes` with Kerberos
explicitly unsupported. This pass **reverts it to `partial`**,
and the score above already reflects that (78.5 with the flip,
76.4 without it). Reason, recorded in `features.csv` and
`build-log.md`: recon F09 names Kerberos as an in-scope
proxy-auth edge ("proxy needs auth (Basic / NTLM / Kerberos)"),
and Kerberos is *unbuilt functionality*, not *unproven*
functionality. Every other `yes` row's remaining gap is
environment-only — the capability is built and only its proof
needs Windows or a real browser. A Kerberos-only proxy is a
real, user-visible case where Fetchline cannot do what IDM
does (it refuses with a clear error, never silently). The
worker's "cannot be implemented or tested on this VM" is a
reason to schedule the work on Windows, not a reason to score
it done. Everything else in the row stands (FTP transport,
NTLMv2 for plain http, Basic, SOCKS5, bypass list).

What did NOT change: licence (S11) is still `no` — verified,
no licence/registration code exists anywhere in the app — and
no must-have besides proxy remains unfinished.

## Feature score by area, weakest first

- security                       0.0  (1 feature)
- updates                        0.0  (1 feature)
- licence                        0.0  (1 feature)
- ui                            42.9  (4 features)
- settings                      70.0  (5 features)
- downloads                     78.9  (8 features)
- browser integration          100.0  (1 feature)
- engine                       100.0  (2 features)
- organisation                 100.0  (1 feature)
- queues                       100.0  (4 features)
- media                        100.0  (2 features)

## Missing, in build order

- [must] settings: Proxy support (HTTP/FTP/SOCKS), partial —
  FTP transport built (PASV/RETR/SIZE/REST resume, per-segment
  connections, FTP proxy with proxy USER/PASS then user@host, or
  direct; bypass list applies; probe via SIZE) and NTLMv2 proxy
  auth for plain http:// (Type 1/2/3 handshake on raw TCP,
  pure-std MD4/MD5/HMAC-MD5; auto-negotiated from a 407 with
  Proxy-Authenticate: NTLM). Still missing: **Kerberos
  (SPNEGO/Negotiate) proxy auth** — refused with a clear error,
  never faked. Also out of scope / refused clearly: https://
  through an NTLM proxy (no TLS tunnel after the handshake),
  FTP through an HTTP proxy (goes direct), NTLM
  signing/sealing, FTPS, active-mode FTP.
- [should] downloads: Drag and drop links in and files out, no
- [should] licence: Registration / 30-day trial, no (Own licence
  model, not IDM's serials)
- [should] security: Automatic antivirus scan on completion, no
  (Hook only; the scanner itself is external)
- [should] ui: Customisable toolbar and skins, no (Order,
  buttons, columns; own skins, not IDM's)
- [should] ui: Multilingual UI, no (IDM ships many languages;
  v1 can ship fewer)
- [should] updates: Quick update check, no (Weekly check, list
  new features, ask before updating)
- [should] settings: Download limits and speed limiter, partial
  (Global KB/s cap enforced by token bucket, live-changeable,
  shown in status bar; MB-per-hours quota not built; no
  per-download cap)
- [could] downloads: Download history and search, no
- [could] downloads: Speed graph history per download, no
- [could] settings: Dial-up connect/disconnect, partial (Dial-Up
  tab built; hang-up after completion is wired to the
  on-completion action path; auto-dial/redial are stored only —
  Windows RAS execution unproven on this VM)

## Left out on purpose (not scored)

- IDM brand, logo, skins and copy: Their trademarked assets;
  rebuild needs its own brand
- Exact proprietary segmentation internals: Rebuild behaviour
  from public HTTP Range knowledge, do not copy code
- DRM-protected streams and paywall bypass: Only openly
  downloadable streams, where the site allows it
- Advanced integration hooking any application: Browser
  extension is in scope; hooking arbitrary apps is not a sane v1

## Layout diff — NOT measured (no score computed)

No layout score exists in this report, and none is claimed. Two
things block the skill's screenshot method, both re-checked in
this pass:

- There are no screenshots of the original IDM on disk
  (`replica/screens/` holds no screenshots), and the brief
  forbids downloading IDM's installer or scraping its site for
  screenshots.
- The only Chromium on this VM (`/opt/meta-chromium/chrome`)
  refused headless startup in the previous pass (Hatch WebGL
  domain policy); no Playwright, Selenium, Firefox, or other
  headless browser is installed. No screenshot was attempted in
  this pass, and no screenshot was faked.
- The clone UI itself does serve: `npm run build` passes and
  the previous pass's Vite preview check returned HTTP 200 with
  `<title>Fetchline</title>`. `replica/clone-screens/` holds no
  screenshots.

What was done instead: a written structural layout comparison,
comparing each clone screen's component hierarchy (read from
`app/src/App.tsx` and `app/src/components/*.tsx`, plus
`app/extension/popup.html` / `content.js`) against the recon's
screen descriptions. Colour is ignored throughout, per the
skill — Fetchline is deliberately rebranded (violet accent,
Inter, Lucide).

### Per-screen structural comparison

- **S01 Main window — structure matches.** Header (app name +
  running-count/speed + theme toggle) / toolbar (Add a link,
  Download all, Batch, Pause, Resume, Cancel, Remove,
  Move-to-category menu, Queues & scheduler, Settings, Start/Stop
  queue) / left Categories + Queues sidebar with counts / main
  download table / status bar (download count, finished count,
  total speed, speed-limit button). Same information, same
  hierarchy as recon S01. Deltas: no tray icon, no context menu,
  columns are not sortable/resizable, no Stop All button
  (Pause/Resume/Cancel are per selected row).
- **S02 Download File Info — structure matches.** Title, URL
  field, filename, save-to folder, category dropdown, Start
  Download / Download Later buttons, inline validation,
  Escape/backdrop close. Deltas: no native folder-picker button
  (plain text input), no description field, no duplicate-file
  prompt (the engine silently appends " (2)").
- **S03 Download progress — structure matches (CHANGED since
  the last pass).** Title / filename, progress bar, live
  speed/ETA/downloaded-of-total, per-segment bars, status badge,
  Pause / Resume / Cancel, error display — plus now the 60 s
  speed graph (SVG sparkline), the engine-driven retry
  countdown ("retrying in Ns", Retry now, Cancel) and "Open
  folder" on completion. Deltas: none left in S03 scope; the
  graph renders only while the dialog is open (no persisted
  per-download speed history — that `could` row stays `no`).
- **S04 Categories pane — structure matches.** Sidebar tree:
  All downloads, builtin categories with counts, custom
  categories, queues section with running badges; add-category
  dialog, delete blocked with a clear error. Deltas: none in
  scope.
- **S05 Scheduler / Queues — structure matches (CHANGED since
  the last pass).** Queue list on the left, Schedule / Files in
  Queue tabs, one-time start/stop, start on startup,
  on-completion action, Start Now, up/down reorder, remove —
  plus now the periodic radio is enabled (interval field, min
  5, default 30), the schedule tab shows last sync with all
  five counts and a "Sync now" button. Deltas: no
  download-limits field in the dialog (the global cap lives in
  Settings / the status bar instead).
- **S06 Site grabber wizard — structure matches.** 3-step
  wizard with a step indicator (1 Project, 2 What to grab,
  3 How to run it) — step 1 project name + start URL, step 2
  file-type checkboxes + depth + page limit, step 3 download
  vs explore-only + optional schedule + per-project queue
  dropdown + on-completion action picker — then a progress view
  (four count cards, live matched-file list with per-file
  status/error, Stop button, finished summary, page-limit
  note) and failed/stopped states. Deltas: a manually Run
  scheduled project starts immediately (explicit override);
  a grabber-armed action and a queue-drain action for the same
  queue share one pending-action slot.
- **S07 Download All — structure matches.** Page URL field,
  checklist grouped by file type with type filter, select all /
  none / invert, live selected count + total size, dead links
  listed with errors and unselectable, already-downloaded
  flagged and unchecked by default, queue picker, Start now /
  Add to queue. Via the browser extension's "Download all links
  on this page" context-menu entry the page URL comes from the
  browser, so it is 3 clicks like the original; the toolbar
  path (paste the page URL) is 4 clicks. Deltas: none left in
  S07 scope.
- **S08 Video grabber — structure matches (CHANGED since the
  last pass).** Extension side: `content.js` floating "Download
  this video" button on hover over a video player, opening a
  small quality/format panel; the popup has a "Find videos on
  this page" button and a scan summary (found / downloadable
  counts). Desktop side: `VideoGrabberDialog.tsx` — paste a
  page link, scan it, pick a video: "Download" opens the S02
  add dialog prefilled, "Later" files it in a queue; dead links
  show their probe error and a "Not downloadable" state; live
  streams are labelled as playlist files, never merged; master
  playlists expand into real variants (capped at 8), encrypted
  playlists are listed as not downloadable, DASH `.mpd` is
  offered as-is labelled a manifest. Deltas: the extension
  panel has never run in a real browser on this VM (verified
  by code review + fixture tests only); the desktop panel
  cannot detect DRM from static HTML (a protected stream is
  only discovered when its probe fails).
- **S09 Options — structure matches (CHANGED since the last
  pass).** Dialog with a tab strip of all 9 tabs (General, File
  Types, Connection, Save To, Downloads, Proxy/Socks, Site
  logins, Dial-Up, Sounds), draft-and-save-at-once, inline
  validation. Deltas: language is stored but the UI stays
  English; start-on-startup is a stored flag only (no Windows
  autostart registration); site logins are Basic auth only
  (NTLM/Kerberos refused at save); auto-dial/redial are stored
  only (no RAS execution here); sound playback is Windows-only
  (never heard on this VM).
- **S10 Batch — structure matches.** Multi-line URL box with
  live link count, queue picker, Start now / Add to queue,
  result listing skipped lines with line numbers. Deltas: no
  filename-pattern option (deliberate, per the build brief).
- **S11 Registration / licence — not built.** No clone screen
  exists to compare (no S11 component in `app/src/`).
- **S12 Speed limiter — structure partly matches.** Global KB/s
  cap in Settings > Connection, live-changeable, current cap
  shown as a status-bar button that opens Settings. Deltas:
  **no MB-per-hours quota**, no per-download cap.
- **S13 Browser integration — structure matches (CHANGED since
  the last pass).** `app/extension/`: `manifest.json` (MV3;
  permissions `activeTab`, `contextMenus`, `cookies`,
  `downloads`, `storage`, `nativeMessaging`, plus `scripting`
  for user-gesture-only video injection; no host permissions,
  no `content_scripts`), `background.js` (context menu
  "Download with Fetchline" on links and image/video/audio
  elements, "Download all links on this page", download watcher
  that cancels a browser download and hands the URL over with
  its cookies), popup (connection status, catch toggle default
  ON, per-site opt-out, send-current-tab, pending count, last
  hand-off status). Firefox host manifest
  `com.fetchline.host.firefox.json` added (allowed_extensions
  + gecko id). Deltas: the extension cannot launch the desktop
  app (the popup only checks the connection); the popup,
  floating button and watcher have never run in a real browser
  on this VM.
- **S14 Completion / antivirus — not built.** No clone screen
  exists to compare. Only the antivirus *command* is stored
  (Settings > Downloads); the scan hook itself does not exist.

## Behaviour diff (flows F01–F10)

Format: flow, original does, clone does, fix or keep.

- **F01 One-click browser download — CHANGED since the last
  pass.** Original: click a link, the extension catches it, S02
  opens, Start Download — 2 clicks. Clone: the S13 extension
  now exists — right-click "Download with Fetchline", or the
  download watcher cancels the browser's own download and hands
  the URL to Fetchline (watcher fixed, BUG-024), which starts
  the download by default (`browser_start_drained`, on) — 2
  clicks, parity on clicks. **Keep.** Wins: cookies are
  forwarded for the exact handed-over URL only, so login-only
  links now download instead of failing at the probe; cookies
  never leak to another host's download, never survive a
  restart, and never appear in a reply or a log; the popup
  shows the pending count and the last hand-off status.
  Differences that remain: the extension has never run in a
  real browser on this VM and the Windows handshake is unproven
  (no .exe yet). **Fix** by proving the handshake on Windows.
- **F02 Manual Add URL.** Original: Add URL → paste → Start —
  3 clicks. Clone: same 3 clicks, plus probe-first prefill of
  filename/folder and category auto-pick. **Keep** — parity,
  with a small win. One difference: on a duplicate file the
  original prompts; the clone silently renames with " (2)".
  **Keep** (fewer clicks), but the silent rename should be
  surfaced in the row / result — minor. New in this pass: a
  login-gated URL now also works from the page-scan paths, not
  only from a manual add (BUG-025 fixed).
- **F03 Pause / resume.** Original: 1 click Resume, continues
  from the last byte. Clone: same, plus ETag/size re-validation
  on resume and restart-from-zero if the server file changed,
  and 2 s SQLite checkpoints. **Keep** — parity or better.
- **F04 Queue + schedule — CHANGED since the last pass.**
  Original: Download Later → pick queue → set times → Start
  Now/wait — 4 clicks. Clone: same 4 clicks, same
  drain-in-order behaviour. **Keep.** Two wins: shutdown /
  hibernate show a 60 s countdown with Run now / Cancel and
  never run silently; the engine rejects malformed times and
  stop-before-start instead of silently never running. New:
  periodic synchronisation is fully built — a periodic queue
  re-probes on the 30 s tick and re-downloads only changed
  files (Last-Modified / ETag / size compared, all three
  fixture-proven; unchanged skipped with 0 GETs;
  remote-deleted reported; empty queue, mid-download skip and
  shrunk file all covered). **Keep.**
- **F05 Capture a video — CHANGED since the last pass.**
  Original: play video → floating "Download this video" button
  → pick quality → S02 — 3 clicks. Clone: popup "Find videos
  on this page" (or the floating button on hover) → pick
  quality/format → S02 prefilled — 3 clicks, parity on clicks.
  **Keep.** Wins: quality is never invented (real dimensions,
  URL label, else "Unknown quality"); DRM / no-stream videos
  show an honest "not downloadable" state with no fake button;
  `.m3u8` is offered as-is, labelled a playlist file, never
  merged; master playlists expand into real variants;
  encrypted playlists are flagged not downloadable. New in
  this pass: the desktop panel's scan now sends a saved site
  login (BUG-025), so a login-gated video lists as downloadable
  instead of 401. Gaps: the extension code has never run in a
  real browser here; the desktop panel cannot detect DRM from
  static HTML. **Fix** by proving it in a real browser on
  Windows.
- **F06 Site grabber.** Original: wizard (name, URL, filters,
  depth) → explore → download, 5 clicks, 2 attempts per file
  then give up. Clone: 3-step wizard → progress view →
  finished summary, 5 clicks, 2 attempts per file then
  recorded as failed. **Keep** — parity on clicks and on the
  2-attempt rule. Wins: polite by measurement (500 ms between
  pages, smallest gap observed 504 ms), robots.txt honoured
  including cross-group Allow (BUG-017 fixed), excluded sites
  re-checked at run time and in the scheduler tick (BUG-018
  fixed), run status written synchronously so the dialog can
  never see a stale state (BUG-019 fixed), per-project queue
  choice and on-completion action reusing the existing
  machinery. Gaps: the crawl has never run against a real
  website (local fixture only). **Fix** the real-website proof
  first. Note: unlike the page scan, the grabber crawl still
  does NOT send site logins — login-required pages are
  reported, as documented.
- **F07 Download all links.** Original: right-click →
  checklist → Start — 3 clicks. Clone: via the extension
  context menu, 3 clicks (parity); via the toolbar button
  (paste page URL), 4 clicks (one extra: the page URL).
  **Keep.** The checklist itself is a win (probe-first sizes,
  dead links excluded with errors, already-downloaded flagged).
  New in this pass: the checklist probes now send a saved site
  login (BUG-025), so gated links list with sizes instead of
  401 errors.
- **F08 Batch download.** Original: paste URLs → Start — 3
  clicks. Clone: same 3 clicks, plus per-line errors with line
  numbers and the dialog staying open with the result when
  lines were skipped. **Keep** — parity or better.
- **F09 Configure connections, proxy, logins — CHANGED since
  the last pass.** Original: Options → Connection →
  Proxy/Socks → Site logins → Save — 5 clicks. Clone: Settings
  button → tab → save; the proxy Test button gives immediate
  feedback (a win), and proxy auth failures are reported as
  errors, not false "OK"s. **Keep** what exists. New: all 9
  tabs are built — Site logins (per-site Basic auth, passwords
  via `secret.rs`, sent on probe + segments, exact-host only),
  Dial-Up (hang-up wired to the on-completion action path;
  auto-dial/redial stored only), Sounds (per-event WAV, engine
  emits the event with the resolved file); proxy now has a real
  FTP transport and NTLMv2 auth. Gaps: **fix** Kerberos proxy
  auth (the one remaining must-have gap); site logins are
  Basic-only (NTLM/Kerberos refused at save — a documented
  limitation, narrower than the proxy gap, and the `could` row
  stays `yes` on its "saved credentials per host" scope);
  FTP through an HTTP proxy goes direct; https:// through an
  NTLM proxy is refused clearly.
- **F10 Organise into categories.** Original: drag to a
  category (1 click) or right-click → Move to Category (3
  clicks). Clone: drag a table row onto a sidebar category
  (1 click) or the "Move to category…" toolbar menu — parity.
  **Keep.** Win: auto-assign covers custom categories too, and
  delete is blocked with a clear error instead of a database
  error.

### Error handling and memory (across flows)

- The clone validates in the **engine**, not only the UI:
  malformed schedule times, stop-before-start, absurd speed
  limits, bad proxy host/port, excluded sites (checked at add
  time, at grabber run time, and in the scheduler tick),
  robots.txt rules, invented video qualities, invalid cookies
  (rejected at the host boundary) and bad site-login input are
  all rejected with clear errors. The original relies on its
  dialogs. **Keep** — strictly better.
- The clone remembers everything the original does for the built
  scope: settings, categories, queues and schedules, download
  list and progress, grabber projects and their progress,
  pending browser links, site logins, sound settings, dial-up
  settings — all persist in SQLite across restarts. Two
  exceptions: pending shutdown/hibernate countdowns live only
  in engine memory and are lost if the app closes before the
  countdown is answered; browser cookies are deliberately
  memory-only and dropped on restart (a resumed gated download
  then fails with a clear 401, by design). **Fix** (persist
  pending actions) or keep as a documented limit.
- What the clone does not remember/do at all: tray state,
  download history/search, speed-graph history, per-download
  speed caps, Kerberos proxy credentials (refused at save for
  site logins; Kerberos proxies refused at use).

## Bugs

**Zero open bugs.** `replica/bugs.md` contains 25 BUG entries
(BUG-001–025) and `grep "Status: OPEN"` returns nothing — every
bug is FIXED, including the three finish-round bugs (BUG-022
NTLM stub, BUG-023 Firefox manifest, BUG-024 watcher never
fired) and the verification-round bug BUG-025 (page-scan probes
never sent a saved site login). The full engine suite is
**330 / 330 pass, 0 failed, twice consecutively** (35 unit +
111 integration + 32 extension + 4 final-verify2 + 16
final-verify + 28 grabber + 14 verifier + 14 options + 7
progress + 15 proxy + 23 sync + 31 video), re-run
independently during this pass with the same counts.
`npm run build` passes; extension JS 44/44; host smoke
13/13 + 14/14 + 14/14 on a freshly rebuilt binary;
`cargo check --tests` 0 warnings. The verdict is therefore
decided by the one unfinished must-have (proxy / Kerberos),
not by bugs.

## Verdict: NOT SHIPPABLE

Per the skill: any must-have missing means not shippable.
Fetchline is at **76.4 / 100** feature parity with **16 of 17
must-haves done** (1 more partial, 0 missing entirely). The one
partial must-have is proxy support, held back only by Kerberos
proxy auth. What is built is solid: the engine, organisation,
queues, media and browser-integration areas all score 100%,
downloads 78.9%, settings 70.0%, and the 330-test suite is
green twice with zero open bugs. A user switching from IDM
today could catch a link (with cookies), grab a video, crawl a
site, sync a queue and configure all 9 Options tabs — but a
user behind a Kerberos-only corporate proxy could not route
through it at all, and the whole desktop shell is still
uncompiled. Not "better than the original" either — no
replica-entrepreneur fixes are in scope yet.

## Top five things to build next (in build order)

1. **Kerberos proxy auth (S09 / F09)** — the only must-have
   not done. SPNEGO/Negotiate via Windows SSPI (or GSSAPI):
   needs a Windows build to implement and prove, since the
   libraries are not present on this VM. Until then a
   Kerberos-only proxy is refused with a clear error.
2. **Registration / 30-day trial (S11)** — the biggest
   remaining user-facing gap (should). Own licence model, not
   IDM's serials; trial countdown, registration dialog.
3. **Antivirus-scan hook (S14)** — run the configured scanner
   on completion and report the result (should; the scanner
   itself stays external).
4. **Quick update check** — weekly check, list new features,
   ask before updating (should).
5. **Download limits: MB-per-hours quota + per-download cap**
   (S12) — finishes the last `partial` should-have; the global
   KB/s cap is already built.

After those, the remaining should/could gaps in order:
drag-and-drop links in / files out, customisable toolbar and
skins, multilingual UI, download history and search, speed
graph history per download. The site grabber's login handling
(send saved site logins on the crawl, currently reported as
login-required) and persisting pending completion actions are
smaller follow-ups worth doing alongside.

## Corrections made to features.csv during this run

- **Proxy support row, `clone` value: `yes` → `partial`.**
  Verifier revert (see "What changed" above): Kerberos proxy
  auth is unbuilt, and recon F09 names it in scope, so the
  row's own scope does not allow a `yes`. The note now records
  the revert and its reason. Score effect: 78.5 → 76.4,
  must-haves 17/17 → 16/17 (verified by re-running `parity.py`
  after the edit).
- **Options dialog row, notes field:** appended a verification
  note — the sync re-probe does call the credential-aware
  `probe_url` (verified by reading `manager.rs`, as the
  parallel rework claimed), but the page-scan probes did NOT
  send site logins (BUG-025, fixed in this pass with 4 kept
  tests); the site grabber crawl still does not send site
  logins (documented, out of scope for the fix). Note-only
  change; the `clone` value stays `yes`.
- Every other row was checked against `build-log.md`,
  `bugs.md`, `test-plan.md` and the app code
  (`app/src/components/*.tsx`, `app/extension/*`,
  `app/engine/src/*`) and matches what is built. No other row
  needed a change. Licence (S11) verified still `no`: no
  licence/registration code exists in the app or the Tauri
  shell.
