# Build log — Fetchline vertical slice

Date: 2026-11-14. Scope: vertical slice only (S01, S02, S03 + F01/F02/F03
core loop). Screens are listed per the recon map; anything not listed
here is not built.

## S01 — Main window: partial (2026-11-14)
- Done: header + theme toggle (light/dark), toolbar (Add / Pause / Resume /
  Cancel / Remove), categories sidebar with counts, download table with
  live progress/speed/ETA, status bar, empty state, error banner.
- Missing: sortable/resizable columns, drag-and-drop between categories,
  custom category creation, Options/Scheduler buttons (those screens do
  not exist yet), tray icon, context menu.
- Harder than expected: keeping the table in sync — live progress events
  plus a 1 s list refresh are both needed, because an event can be missed
  when the dialog is open.

## S02 — Add a new download dialog: done (2026-11-14)
- Done: URL validation, file-name default from probe / Content-Disposition,
  save-folder field, category dropdown (auto-pick from file type),
  "Start download" and "Save for later", Escape/backdrop close, focus on
  the URL field, inline error for bad URLs and probe failures.
- Missing: native folder-picker button (needs the Tauri dialog plugin;
  the field is a plain text input for now), duplicate-file prompt
  (the engine silently appends " (2)" instead).
- Harder than expected: nothing — the probe-first design made the dialog
  straightforward.

## S03 — Download progress dialog: partial (2026-11-14)
- Done: live speed/ETA/downloaded-of-total, per-segment bars, status badge,
  Pause / Resume / Cancel, error message display.
- Missing: speed graph (the recon's 60 s chart — numbers are shown instead),
  "Open folder" on completion, retry countdown display.
- Harder than expected: segment bars — a dynamic split creates segments
  after the dialog opened, so the bar list must be driven by live events,
  not by the snapshot taken when the dialog opened.

## Engine (not a screen, but the slice depends on it)
- Done: HEAD probe + ranged-GET fallback, 4-way segmented download,
  dynamic split of the largest segment, `.part` file with pre-allocation
  and atomic rename, 2 s SQLite checkpoints, pause/resume from persisted
  progress, ETag/size re-validation on resume (restart from zero on
  change), per-segment exponential backoff then `error` status.
- Verified: `cargo test` in `app/engine` — 6 passed, 0 failed (7.2 s)
  against a local mock HTTP server, byte-exact results. `cargo check`
  warning-free. See `app/README.md`. (Milestone 2 grew this to 17
  integration tests + 4 unit tests, all passing — see S05 above.)
- Harder than expected: the dynamic split. The schema's no-overlap trigger
  forces a strict order (shorten the old segment in the DB first, then
  insert the new one), and the running task must re-read its end bound on
  every chunk or it writes past the split point.
- **Real bug found and fixed during testing:** the run loop deadlocked.
  `try_split`, `checkpoint` and `emit` locked a segment's `std::Mutex`
  and then called `segment_len()`, which locked the same mutex again
  (std Mutex is not re-entrant). The first full-download test hung for
  7 minutes before this was found with temporary tracing. Fixed by
  passing the already-locked state (`segment_len_of(&st)`); all 6 tests
  pass since. A second bug was in the test mock, which matched the
  `Range:` header case-sensitively while hyper sends it lowercase —
  fixed, and the engine now also fails loudly if a server answers a
  partial range request with 200 instead of 206.
- Note: files under 1 MiB download as a single segment by design
  (`MIN_SIZE_TO_SEGMENT`); the 512 KiB test file therefore exercises
  the single-segment path, and multi-segment behaviour is exercised by
  the same code with 4 initial segments on larger files.

## S04 — Categories: done (2026-11-14)
- Done: builtin categories wired to real data with counts; create a
  custom category (name, folder, file extensions, comma-separated);
  auto-assign from file extension (custom categories take part in the
  lookup); move a download to another category by dragging its table
  row onto a sidebar category, or from the "Move to category…" menu in
  the toolbar; delete a custom category, blocked while downloads
  reference it (the engine refuses with "still has N download(s) —
  move them to another category first" and the UI shows that error;
  builtin categories cannot be deleted at all).
- Schema change (justified): `downloads` gained `queue_position
  INTEGER NOT NULL DEFAULT 0` + index `idx_downloads_queue_position
  (queue_id, queue_position)`. `created_at` has 1-second resolution
  and cannot represent a reorder, so queue order needs an explicit
  column. `Store::migrate` adds it to pre-existing databases.
  Schema re-validated after the change (python sqlite load: 9 tables,
  main queue + 5 categories seeded).
- Missing: nothing in S04/F10 scope.

## S05 — Queues and scheduler: done (2026-11-14)
- Done (queues): main queue (seed `queue-main`) + custom queues;
  "Save for later" in S02 now actually files the download in the
  main queue; list files in a queue with up/down reorder; Start Now
  drains in order and each finished file leaves the queue; stop
  pauses the current file (resumable) and ends the run after it
  settles; double-click a sidebar queue to open the scheduler.
- Done (scheduler): per-queue one-time schedule (start/stop time,
  start on startup) driven by a 30 s tick in the Tauri shell plus
  `start_startup_queues()` at launch; a one-time queue runs once per
  session and is never restarted by later ticks; a stop time ends the
  run after the current file pauses. On-completion action: none /
  open file / run program / shutdown / hibernate. Shutdown and
  hibernate show a 60 s countdown dialog with Run now / Cancel and
  never run silently; open file / run program execute as soon as the
  queue finishes. The engine refuses shutdown/hibernate off Windows
  ("only available on Windows") instead of running anything.
- **Deferred (recorded, not built): periodic synchronisation.**
  The scheduler form shows the repeat option disabled as "coming in
  a later version", and `update_queue` rejects `schedule_type =
  "periodic"` with "Periodic synchronisation is not built yet". This
  is the L build; do not treat the disabled radio as a bug.
- Commands added to `src-tauri/src/lib.rs` (architecture.md names
  where they exist): `create_queue`, `update_queue`, `start_queue`,
  `add_to_queue`, `list_queues`, `get_queue`, `get_queue_files`,
  `reorder_queue`, `stop_queue`, `create_category`,
  `move_to_category`, `delete_category`, `execute_queue_action`,
  `dismiss_queue_action`, `get_pending_action`,
  `remove_from_queue`. `add_download` gained a `queue` parameter.
  Architecture.md names the category list `get_categories`, which
  already existed — no duplicate `list_categories` was added.
  The shell also forwards `queue-action-pending` events and runs the
  30 s scheduler tick.
- Verified: `cargo test` in `app/engine` — **17 passed, 0 failed**
  (6 existing + 11 new: queue drain order, queue stop + restart,
  scheduler start/stop/one-time-no-restart, reorder, category
  auto-assign, category delete blocked then allowed, builtin delete
  rejected, periodic rejected, run-program action + dismiss,
  shutdown errors off Windows) plus 4 timeutil unit tests.
  `npm run build` passes (tsc + vite). `cargo check` warning-free.
- Harder than expected / real bugs found and fixed during testing:
  1. **Queue restart stalled after a stop.** The runner's wait loop
     broke on the stop flag before the in-flight download had
     finished pausing, so a restart collided with the
     still-registered run ("already running") and the queue died
     with its files stuck. Fixed: on stop the runner pauses the
     file *and waits* for a terminal status before ending the run.
  2. **Double resume.** `resume_download` returns as soon as the
     run task is spawned, but the status flips to "downloading"
     asynchronously; the wait loop polled the stale "paused" status,
     ended the run, and the outer loop resumed the file a second
     time — whose probe then failed on a stale pooled connection
     from the aborted pre-pause segments ("error sending request").
     Fixed: the wait loop ignores the pre-start status until it has
     seen "downloading". Found with temporary tracing (since
     removed); the test mock was also extracted to
     `engine/tests/common/mock.rs` so both test files share it.
  3. The pause/resume timing test is sensitive to parallel load:
     it failed once in the full 17-test suite (pause landing before
     any checkpoint) and passed in isolation. No code change; the
     test's 600 ms pause window is tight when 17 mock servers share
     1.5 CPUs.
  4. **Flaky probe failure in the test mock.** A test that adds
     several downloads to one mock server failed intermittently
     with "Connection reset by peer" on the probe. Cause: the mock
     serves one request per connection and then closes the socket
     without saying so, so reqwest pooled the dead connection and
     reused it. Fixed in the mock (not the engine): every response
     now sends `Connection: close`. Three consecutive full runs
     after the fix: 17/17 + 4/4 each time.
- Partial / untested on Windows: the Tauri shell still cannot be
  compiled on this Linux VM (missing `gdk-3.0`; apt fix not retried,
  per the milestone brief) — the 15 new commands are written
  following the existing pattern but have not been compiled; the
  drag-and-drop and the 60 s countdown dialog are UI-only until the
  first Windows build. Pending completion actions live only in
  engine memory: if the app is closed before the countdown is
  answered, the action is lost (the UI re-checks pending actions on
  launch, but only within the same process lifetime). Shutdown /
  hibernate run real OS commands (`shutdown /s /t 0`, `shutdown /h`)
  and have never been executed on Windows here.

## S07 — Download All: done (2026-11-14)
- Done: fetch a page (GET, 8 MiB cap, non-success status is an error),
  extract links from `<a href>`, `<link href>`, `<img src>`, `<video src>`,
  `<audio src>`, `<source src>`; relative links resolved against the page
  URL, fragments dropped, `javascript:`/`mailto:`/`tel:`/`data:` skipped,
  de-duplicated in first-seen order, the page's own URL excluded. Each
  link is probed in parallel for size; probe failures are listed with
  their error and excluded from the selection; links already in the
  download list are flagged and start unchecked (the user can still pick
  them). Checklist grouped by file type (Video / Audio / Image /
  Document / Archive / Program / Other) with a type filter, select all /
  select none / invert, and a live count of selected links plus their
  total size (with "+ unknown" when a size is missing). "Start now" or
  "Add to queue" (main queue by default, or a chosen queue); every link
  is added through the single-add path, so category auto-assign and the
  duplicate-path guard (BUG-002 fix) apply.
- Engine: new `engine/src/links.rs` (extraction + classification, pure
  functions with unit tests) and manager methods `fetch_page_links`,
  `add_batch_downloads`, `download_all_links`; `Store::url_exists`.
  Tauri commands (architecture.md names): `fetch_page_links`,
  `add_batch`, `download_all_links`, wired into `src/lib/api.ts`.
  No schema change.
- Verified: `cargo test` — **61 integration + 6 unit, 0 failed**
  (51 existing + 10 new: extraction of all tag types + relative
  resolution, kind classification, probe sizes/kinds/dead-link error,
  already-downloaded flag, bad page 404, batch line validation with
  line numbers, batch dedup, duplicate-path guard reuse across
  batches, batch add-to-queue + category auto-assign, download-all
  adds only the selected links, unknown queue/category rejected up
  front with nothing added). `npm run build` passes (tsc + vite).
  `cargo check --tests` warning-free.
- Missing: nothing in S07/F07 scope. The page fetch uses the engine's
  reqwest client with no extra headers — a site that blocks non-browser
  clients will answer with its error, which the dialog shows.
- Harder than expected: Rust raw strings. The extraction unit test
  used `r#"..."#` for its HTML, but `"#frag` inside the HTML ends a
  raw string early and the compiler reports it as "unknown prefix"
  errors — two rounds of that before the test was rewritten as a plain
  escaped string.

## S10 — Batch download: done (2026-11-14)
- Done: "Batch" toolbar button opens a dialog with one link per line
  and a live link count. Submit calls `add_batch`, which validates
  each line in order: blank lines ignored; bad URLs and probe failures
  reported with their 1-based line number and skipped; a link repeated
  inside the batch is flagged as a duplicate of its first line. One bad
  line never fails the batch. "Start now" starts every created download;
  "Add to queue" files them in the chosen queue in batch order. The
  dialog stays open with the result ("Added N, M lines could not be
  added" + the per-line errors) when anything was skipped, and closes
  when everything was added. No filename pattern option — the brief
  said to keep it simple, and the file name comes from the probe /
  URL as in a single add.
- Missing: nothing in S10/F08 scope. Large lists are added one probe
  at a time (sequential, like single adds); a 100-link batch takes as
  long as 100 probes — acceptable for a desktop paste, noted here.
- Harder than expected: nothing — reusing `add_download` for every
  line meant the batch inherited the probe, category and duplicate
  guards for free, and the only new logic was line validation and
  the per-line error report.

## S09 — Options dialog (core tabs): partial (2026-11-14)
- Done (General): default download folder, start-on-startup flag (stored;
  see the caveat below), browser-integration flag, weekly update check,
  theme (applies live and persists — the header toggle now writes the
  setting), language (English only for now).
- Done (File Types): every category's extension list is editable; the
  change feeds straight into the existing auto-assign lookup.
- Done (Connection): connections per download (1–16, default 8 — now
  actually changes the engine's segment count), connection timeout
  (1–300 s, applied as the reqwest connect timeout on client rebuild),
  retries per part (0–10), speed limit (KB/s, 0 = unlimited).
- Done (Save To): every category's folder is editable; the folder
  resolution in `add_download` is now: explicit folder → category
  folder → settings default folder → OS Downloads folder.
- Done (Downloads): default action (start now / save for later — the
  Add dialog's primary button follows it), duplicate handling (keep
  both / replace), ask-before-remove (a confirm dialog in the main
  window), antivirus command (stored; the scan hook itself is a
  later should-have, not this milestone).
- Done (Proxy/Socks tab): enable/disable, HTTP / SOCKS5 / FTP type,
  host, port, username/password, use-for (all / HTTP only / HTTPS
  only), bypass list, Test button that probes a URL through the
  draft proxy and reports the status. The engine really routes
  through the configured proxy: reqwest `Proxy` on the rebuilt
  client, Basic auth, `NoProxy` bypass. SOCKS5 needed reqwest's
  `socks` feature (added to `engine/Cargo.toml`).
- Settings live in the existing `settings` table; six new columns
  were added to `schema.sql` plus `Store::migrate` ALTERs for older
  databases (verified by a migration test that opens a pre-milestone
  database file). Proxy rows gained `enabled` + `bypass` columns the
  same way. No other schema change.
- Password storage: the password is protected with Windows DPAPI
  (`CryptProtectData`, user scope, direct FFI to crypt32, Windows
  only) and only the "has password" flag ever crosses the Tauri
  bridge. On this Linux VM there is no DPAPI, so the value is stored
  base64-obfuscated (`b64:` prefix) — that is NOT encryption, and the
  DPAPI path has never run on Windows here. `secret.rs` documents
  this; the stored value carries a `dpapi:` / `b64:` prefix so a
  Windows build can tell them apart.
- Commands added to `src-tauri/src/lib.rs` (architecture.md names
  where they exist): `get_settings`, `update_settings`,
  `set_speed_limit`, `get_speed_limit`, `get_proxy_config`,
  `save_proxy_config`, `test_proxy`, plus `update_category` (new
  name — no architecture.md name exists for it). The shell applies
  the stored settings once at launch and after every save.
- Verified: `cargo test` — **81 integration + 12 unit, 0 failed**
  (61 existing + 20 new: settings round-trip + validation + migration,
  segment count from settings, category folder/extension edits,
  default folder, duplicate overwrite, proxy validation, password
  round-trip + hidden-from-UI, HTTP proxy + auth, disabled proxy,
  bypass, SOCKS5 + auth + wrong password, test-proxy, FTP stored
  but not applied, speed cap / unlimited / mid-download change).
  `cargo check --tests` warning-free. `npm run build` passes.
- Missing / deferred: Site logins tab (separate feature row, not
  built), Dial-Up and Sounds tabs (deferred — legacy/polish, per
  architecture.md), multilingual UI (language is stored, UI stays
  English). Start-on-startup is a stored flag only: registering the
  app in Windows autostart was not implemented.
- Harder than expected / real bug found and fixed during testing:
  **the speed limiter could stall a download forever.** The token
  bucket holds at most one second of tokens, so a chunk larger than
  the rate (a 64 KiB chunk at a 16 KiB/s cap) could never be afforded
  and `acquire` waited in a loop that never ended — the capped test
  download sat at 57,197 bytes for over a minute. Fixed in
  `limiter.rs`: a large acquire is spent in rate-sized pieces.
  A limiter unit test pins this (`acquire_larger_than_rate_completes`).

## S12 — Speed limiter: partial (2026-11-14)
- Done: global KB/s cap (0 = unlimited) enforced by a shared token
  bucket (`engine/src/limiter.rs`) across all active downloads —
  every segment task waits for tokens before writing each chunk.
  The cap is live-changeable (a running download picks the new rate
  up on its next chunk) and the current cap shows in the status bar
  as a button that opens Settings. `set_speed_limit` /
  `get_speed_limit` follow the architecture.md command names.
- Verified by test, not just by reading: a 256 KiB download at
  32 KiB/s measurably takes ~8 s (asserted ≥ 5 s, < 25 s); unlimited
  finishes in under 5 s; lifting the cap mid-download finishes the
  rest in seconds instead of ~30 s.
- Missing / not built: the MB-per-hours quota (recorded as not built
  — `set_download_limit` was not added), and no per-download cap
  (the downloads table has no per-download limit column; the global
  cap is the whole feature as built).

## Bug-fix pass — BUG-008–011 (2026-11-14, after replica-test round 2)

Failing-test-first; the two round-2 DOC tests were converted into
real regression tests, not deleted. Full suite after the fixes: 106
integration + 12 unit, 0 failed, 2 consecutive runs. `npm run build`
passes. BUG-005/006/007 untouched.

- BUG-008 (absurd speed limit): rejected, not clamped.
  `Store::update_settings` rejects any cap above
  `MAX_SPEED_LIMIT_KBPS = 10_000_000` KB/s (10 GB/s — beyond any real
  link, and the 10,000,000 KB/s sane-huge value still works) with a
  clear "too large" error; `apply_settings` uses `saturating_mul` as
  defence in depth, and `SettingsDialog` enforces the same bound.
  Chosen over clamping so a typo (e.g. an extra digit) is visible
  instead of silently becoming a different cap.
- BUG-009 (proxy Test said "OK" for 407): `test_proxy` now errors on
  a 407 — "Proxy authentication failed (407) — check the proxy
  username and password" — and on any other non-2xx status. The
  dialog's existing "Failed — …" path shows it as a failure.
- BUG-010 (Test reported success without using the proxy):
  `test_proxy` now fails before sending when the proxy is disabled,
  the test URL is not http(s), the proxy is FTP (no FTP transport),
  or the proxy's `use_for` scope does not cover the test URL's
  scheme. A test can no longer report success for a proxy it never
  used.
- BUG-011 (dead settings wired in): `file_types_auto` is now the
  allow-list for category auto-assign — empty means every extension
  is auto-assigned (the old behaviour); a non-empty list restricts
  auto-assign to the listed extensions and anything else falls back
  to Documents. `excluded_sites` is a host list (exact host or any
  subdomain): `add_download` refuses before any probe,
  `fetch_page_links` refuses an excluded page and flags excluded
  links with an error instead of probing them, and batch lines to
  excluded sites are reported with their line number and skipped.
  Both lists are normalised on save (extensions lowercased, no dots,
  de-duplicated; hosts lowercased, no scheme/path). The Settings
  dialog edits both: "Excluded sites" in the General tab,
  "Auto-assign file types" in the File Types tab. Limitation: there
  is still no browser extension, so exclusion only applies to
  downloads Fetchline itself starts — it cannot stop a browser
  download directly.

## Bug-fix pass — BUG-005–007 fixed (2026-11-14)

Failing-test-first; no open bugs remain in `replica/bugs.md`.
- BUG-005 (malformed schedule time): `Store::update_queue` now
  validates start/stop with the existing `parse_iso_to_unix` (UTC
  ISO-8601, fractional seconds accepted) and rejects a malformed
  time with a clear error — nothing is stored. The parser also
  rejects impossible dates (30 February, 31 April, 29 February in
  a non-leap year), which previously rolled silently into the next
  month. Legacy rows written before the fix are reported by
  `scheduler_tick_at` instead of silently skipped: the tick still
  processes every other queue first, then returns an error naming
  the bad queues ("Some queues have an invalid schedule and were
  skipped: … Re-save their schedules."). Re-saving such a queue
  from the dialog validates and clears the bad times. Blank strings
  clear the field, as the `update_queue` doc already promised.
- BUG-006 (stop before start): same validation rejects a stop time
  that is not after the start time — including stop equal to start
  and stop with no start — with exactly the UI's message: "The stop
  time must be after the start time." Engine and UI now agree.
- BUG-007 (sidebar delete-category button mouse-only): both
  hover-only buttons in `CategorySidebar` (delete-category and the
  queue schedule button, same pattern) no longer use `hidden …
  group-hover:block`. They use `opacity-0` with
  `group-hover:opacity-100 group-focus-within:opacity-100
  focus-visible:opacity-100`: `opacity-0` keeps the button in the
  keyboard tab order, and it appears on hover, on focus within its
  row, and on keyboard focus. Verified by source check plus the
  built CSS containing both emitted rules; a real keyboard
  walk-through still needs the desktop binary.
- Full sweep after the fixes: **109 integration + 13 unit, 0
  failed, 2 consecutive full runs** (was 106 + 12; the two DOC
  tests were converted, not duplicated). `cargo check --tests`
  warning-free. `npm run build` passes; Vite preview serves the
  app (index 200, `Fetchline` title, JS asset 200). Regression
  areas all covered by the passing suite: add/pause/resume,
  categories, queues/scheduler/actions, Download All + batch,
  settings, proxy, limiter. `/tmp` cleaned of test leftovers
  before and after the runs.

## S13 — Browser integration (extension): partial (2026-11-14)
- Done: Manifest V3 extension in `app/extension/` — `manifest.json`
  (permissions `activeTab`, `contextMenus`, `downloads`, `storage`,
  `nativeMessaging` only; no host permissions, no `cookies`, no
  content script), `background.js` service worker (context-menu
  "Download with Fetchline" on links and image/video/audio elements;
  download watcher that cancels a browser download and hands the
  URL over when catching is on, the type is in the catch list and
  the site is not opted out), popup (connection status, catch
  toggle default ON, per-site opt-out, send-current-tab, desktop
  check), fresh violet download-arrow icons (16/32/48/128 PNG +
  SVG), `README.md` with load + host-install + verification steps.
- Done (desktop side): standalone native host `app/host/`
  (`fetchline-host`, own crate depending only on the engine by
  path — a `[[bin]]` in `src-tauri` was not used because that
  crate cannot build on this VM at all). It speaks the
  native-messaging protocol (4-byte LE length + JSON; framing
  helpers live in exactly one place, `engine/src/native_host.rs`,
  shared with the tests). It opens the SAME SQLite file as the
  desktop app (`--db-path` / `FETCHLINE_DB_PATH` override; else
  the first existing Tauri app-data candidate) and enqueues into a
  new `pending_links` table (migration in `Store::migrate` + table
  in `replica/schema.sql`, so in-memory test DBs get it too).
  `Manager::drain_pending_links` picks pending rows up through
  the REAL `add_download` path (excluded-sites refusal,
  `file_types_auto` auto-assign, duplicate-path guard all apply)
  and marks each row accepted (with the download id) or rejected
  (with the reason). The Tauri shell drains on startup and on the
  same 30 s tick as the scheduler, and exposes a
  `drain_pending_links` command. No stub anywhere: the host binary
  was smoke-tested end to end (ping, get_settings, two enqueues,
  video_found refusal, download_all enqueue — rows visible in the
  DB afterwards).
- Done (truth + opt-out): the host reports
  `settings.browser_integration` and `excluded_sites`; the popup
  shows them, and when the desktop flag is off the catch toggle is
  disabled and nothing is caught — desktop setting wins. Per-site
  opt-out exists in both places (desktop list in Settings,
  extension-local list in the popup); either one skipping a site
  is enough. Exclusion is enforced three times: extension,
  host boundary (never enqueued), drain (re-checked, and
  `add_download` checks again before any probe).
- Implemented messages: `download_link` and `download_all`
  (with a documented deviation — the extension sends `links: []`
  and the host expands the page via `fetch_page_links`, because
  there is no content script to list the page's links), plus
  `ping` / `get_settings` (not in architecture.md). `video_found`
  is refused with a clear error — F05 / S08 is not built.
- Verified: `cargo test` in `app/engine` — **109 integration + 13
  unit + 19 new extension tests, 0 failed** (framing round-trip /
  oversized / truncated / clean EOF, host enqueue + excluded +
  integration-off boundaries, drain via real add_download,
  drain-time exclusion, drain-time integration-off, duplicate URL
  getting a different save path, dead-link rejection with the rest
  still draining, page expansion, manifest validation).
  `cargo check --tests` warning-free. `npm run build` passes;
  `npm run test:extension` (Python manifest validator) passes;
  `node --check` passes on `background.js` / `popup.js`.
- Phase 2 verification (2026-11-14, same day): the full suite was
  re-run twice (109 + 13 + 21 extension, 0 failed) and the real
  host binary was driven with native-messaging frames against a
  local HTTP server only (27 smoke checks: ping, get_settings,
  enqueue paths, exclusion, integration-off, malformed /
  oversized / truncated frames, locked DB, pre-S13 migration,
  drain of the host-produced SQLite file — 7 accepted with
  distinct save paths, 1 dead link rejected with 404, double
  drain a no-op). It found two bugs, both fixed failing-test-first
  in the verification round (see `replica/bugs.md` BUG-012/013):
  (a) `download_all` with no links bypassed the
  browser-integration off switch — `enqueue_page_links` never
  checked the flag, so links were queued while catching was off
  and drained once it was turned back on; fixed in
  `manager.rs`, and a `download_all` that enqueues nothing now
  returns an error instead of `ok: true` (`native_host.rs`);
  (b) a malformed JSON frame killed the host with no reply —
  the host loop now answers a framed error and continues for a
  bad-JSON payload, and still exits promptly on a framing
  failure (`app/host/src/main.rs`). Regression tests added:
  `enqueue_page_links_refuses_when_browser_integration_is_off`,
  `host_download_all_all_skipped_is_an_error` (extension tests
  now 21). No other verification finding: pre-S13 migration
  adds `pending_links` without losing data, a locked DB fails
  cleanly after the ~5 s busy timeout (exit 1), and a missing
  DB path is created with its parent directories.
- Missing / partial: the video panel (F05) is not built; drained
  links sit **queued, not started** (starting them is the desktop
  app's job — an unattended host must not start downloads the user
  has not seen); no cookies are ever sent, so links that need a
  login may fail at the probe; the extension cannot launch the
  desktop app (the popup only checks the connection). The Tauri
  shell changes (drain call + command) follow the existing pattern
  but are **uncompiled** — `src-tauri` still cannot build on this
  Linux VM (missing `gdk-3.0`), same as every earlier milestone.
  **Correction (2026-11-15, Milestone 2):** the video panel is
  built (S08, see below) and drained links now START by default
  (`browser_start_drained`, on) — see the "S13 finish" entry.
  The queued-only behaviour survives as that setting turned off.
- Harder than expected: (1) the brief said the Tauri DB path is
  `app_data_dir()/fetchline.db`, but `src-tauri/src/lib.rs`
  actually uses `app_data_dir()/fetchline/app.db` — the host
  therefore tries the Tauri app-data candidates in order and takes
  the first existing file, with `--db-path` as an explicit
  override; the exact Windows app-data folder still needs
  confirming on the first Windows build. (2) The popup's
  "send this page" needs the active tab's URL, which
  `chrome.tabs.query` will not return without a permission — the
  minimal fix was `activeTab` (granted only on the popup click),
  not the broad `tabs` permission. (3) The shared test mock
  produces dead-code warnings in the new test file (it also serves
  proxy tests this file does not use) — silenced with
  `#![allow(dead_code)]` on the test file, not by editing the mock.
- Design decision (recorded): the host only enqueues; it never
  downloads. The queue is the contract between the browser and
  the desktop app, and the drain is the only path into the
  download list — there is no second add path to drift from the
  first. `pending_links` rows are never deleted by the drain, so
  the history of what the browser sent stays visible.

## S13 — Browser integration: finish (Milestone 2, 2026-11-15)
- Done (drain starts downloads): `drain_pending_links` now STARTS
  each accepted download through the normal `start_download`
  (speed limiter, proxy, segments all apply) instead of leaving it
  queued. New setting `settings.browser_start_drained` (default
  ON — start immediately); turning it off keeps the old
  queued-only behaviour. New column in `replica/schema.sql` +
  `Store::migrate` ALTER for older databases (default 1, verified
  by a kept migration test on a hand-built pre-M2 settings table).
  If the start itself fails, the download stays queued and the
  failure is recorded in `DrainSummary.start_failed` (new field),
  not as a rejection. The drain still goes through the REAL
  `add_download` path (excluded sites, `file_types_auto`,
  BUG-002 duplicate guard all apply). Settings UI: a "Start
  browser-caught downloads at once" checkbox in Options > General
  (disabled while catching is off); the stale "(not built yet)"
  hint next to the integration toggle is fixed. The Tauri
  `drain_pending_links` command needs no change (same signature,
  `DrainSummary` gains the new field) but is still uncompiled.
  This also covers drained videos (S08, same drain path).
- Done (Download All context entry — already existed, now
  verified): the "Download all links on this page with Fetchline"
  page-context menu item was already in `background.js` from S13
  (it sends `download_all {page_url, links: []}` and the host
  expands the page via the S07 `fetch_page_links` — no second
  fetcher was written). No new permission was needed
  (`contextMenus` already covers it; the host does the fetching).
  Verified end to end through the REAL `fetchline-host` binary
  against a local HTTP page only: new smoke test
  `app/host/download-all-smoke.py`, 14/14 (3 good links enqueued
  as `download_all` rows, duplicate de-duplicated, dead link
  skipped at expansion, excluded page refused, integration-off
  refused — the BUG-012 path — malformed JSON answered with a
  framed error, host stays alive). One wrong test expectation,
  not a product bug: explicit `download_all` links are enqueued
  without probing, so a dead link in an explicit list is queued
  and rejected at drain time, not at enqueue time.
- Done (popup status): the popup shows the pending count ("N
  links waiting in Fetchline") and the last hand-off ("Last sent
  …" / "Last send failed …"). The host reports `pending_count`
  (new `HostReply` field) in every `get_settings` / queued reply,
  including the host binary's hand-built `download_all` reply;
  the background worker records each hand-off outcome in
  `chrome.storage.local` (outcome + time only, never a URL) and
  the popup's "Send this page" path records its own too. New
  Node tests with a fake `chrome` API (no browser needed):
  `extension/tests/popup.test.js` (6 tests) and
  `extension/tests/background.test.js` (4 tests), wired as
  `npm run test:extension-popup` /
  `test:extension-background`. `popup.html` gains the two status
  lines; the formatters are pure functions on `globalThis`.
- Done (Windows checklist): `app/extension/README.md` now has a
  "Windows verification checklist" — registry key, manifest
  `path` / `allowed_origins` with the real extension ID,
  app-data path confirmation (`%APPDATA%\com.fetchline.app\
  fetchline\app.db` first, then `%APPDATA%\fetchline\app.db`),
  first live handoff, Download All, both off switches, and
  removing dev state. It is a checklist only: NOTHING in it has
  run, and the row stays `partial` for exactly that reason.
- Verified: full engine suite **222/222, 0 failed, 2 consecutive
  runs** (21 unit + 111 integration + 25 extension + 15 grabber +
  14 verifier + 15 sync + 21 video; was 218 — +4 new extension
  tests, the drain test rewritten to assert completion
  byte-exact). `cargo check --tests` warning-free. `npm run
  build` passes; `npm run test:extension` passes; `node --check`
  passes on all four extension scripts; extension JS tests 21/21
  (11 video-detect + 6 popup + 4 background). Host smoke:
  download-all 14/14, video 11/11 re-run green. No new bugs
  found (see `replica/bugs.md` Round 8).
- Missing / partial (unchanged, honest): the Windows handshake
  is unproven (no .exe yet — checklist above is what proves it);
  no extension code has executed in a real browser on this VM
  (popup, menu, watcher verified by code review + `node --check`
  + the fake-`chrome` Node tests only); no cookies, so
  login-only links may fail at the probe; the Tauri shell changes
  (one doc comment on `drain_pending_links`, no new command)
  follow the existing pattern but are uncompiled (`gdk-3.0`,
  same as every earlier milestone). The row stays `partial`.
- Harder than expected: (1) the popup/background tests needed a
  `vm` context with a fake `chrome` API, and two test bugs ate
  the time, not product bugs — the fake `sendNativeMessage`
  took 2 arguments where the real API takes 3 (host, message,
  callback), and `assert/strict`'s `deepEqual` is strict, so a
  message built inside the `vm` realm (different Object
  prototype) fails it even with identical structure — both fixed
  by comparing JSON strings. (2) The S13 test
  `drain_picks_up_enqueued_link_via_add_download` asserted
  status `queued`, and the S08 test asserted the same for video —
  both had to be rewritten for the new start-by-default
  behaviour (the video test is renamed
  `video_found_end_to_end_drains_into_a_real_download`).

## S13 — Browser integration: finish (2026-11-15, PARTIAL 1 of 4)

- Done (extension): cookie forwarding for login-only links.
  `manifest.json` gained the `cookies` permission (justified
  in the manifest, the validator and the README) and
  `browser_specific_settings.gecko.id:
  "fetchline@fetchline.app"`. `background.js` gained
  `cookiesFor(url)` — it reads cookies ONLY for the exact URL
  being handed over (`chrome.cookies.getAll({url})`) and
  attaches name/value pairs to `download_link` (message.url)
  and `video_found` (variants[0].url); `download_all` carries
  no cookies (the host fetches the page itself). The watcher
  fix (BUG-024): `extensionOfFilename()` strips directories
  and takes the suffix, so a download that already has a
  filename is no longer silently skipped.
- Done (host + engine): `native_host.rs` gained `CookiePair`
  and `cookies_to_header()` (validate + join; name: no
  whitespace/control/`;`,`,`,`=`; value: no `;`,`,`,control;
  ≤ 32 pairs, ≤ 8192 header bytes) plus
  `cookies_to_json()`/`cookies_from_json()` (corrupt → empty,
  never fails a drain). `DownloadLink`/`VideoFound` carry
  `cookies: Vec<CookiePair>`; `enqueue_one` validates and
  stores the JSON on the pending row; `mark_pending_link`
  sets `cookies = NULL` when the drain decides the row.
  `store.rs` gained the `cookies TEXT` column (migration
  placed after the S08 rebuild block so a rebuild can't drop
  it; `replica/schema.sql` updated to match). `manager.rs`
  gained `DownloadManager.cookies: HashMap<download_id,
  header>` (in memory only): `add_download_with_cookies`
  probes via `probe_with_cookies` (cookies win over a stored
  site login; FTP falls back to a plain probe), the drain
  rebuilds the header from the row and calls it, `run_inner`
  passes the header to every segment GET for that host only,
  and the entry is removed on complete/error/cancel, kept on
  pause, dropped on restart (a resumed download then fails
  with a clear 401 — tested). `probe_with_cookies` is
  deliberately the plain HTTP(S) path: it does NOT go through
  proxy/NTLM machinery, so a cookie-gated link behind an NTLM
  proxy reports the proxy's 407, and the raw NTLM segment
  retry does not carry cookies — both documented in
  `bugs.md`. A `download_all` page expansion carries no
  cookies; a login-gated page's links must be right-clicked
  individually.
- Done (Windows handshake): verified by inspection against
  Chrome's native-messaging docs and Firefox/MDN — registry
  `HKCU|HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\<name>`
  (Edge substitutes `Microsoft\Edge`, Firefox `Mozilla`),
  default value = manifest path; `allowed_origins` entries
  exactly `chrome-extension://<32 lowercase a–p chars>/`,
  trailing slash mandatory, no wildcards; extension-ID format
  confirmed via Chrome's example id
  `knldjmfmopnpolahpmmgbagdohdnhkik`; `db_candidates()` in
  `app/host/src/main.rs` verified by inspection
  (`%APPDATA%\com.fetchline.app\fetchline\app.db` first, then
  `%APPDATA%\fetchline\app.db`). The README Windows checklist
  (7 items) is annotated per item as verified-by-inspection
  vs needs-Windows; nothing in it has run on Windows.
- Done (tests): 7 new engine tests in
  `tests/extension_tests.rs` behind a local
  `CookieGatedServer` fixture (own TCP server in the test
  file, not the shared mock): header validation, host stores
  cookies on the row + reply never carries the value, host
  refuses invalid cookies, gated drain completes byte-exact
  and the row's cookies are wiped, gated drain without
  cookies rejected with a clear 401, cookies never leak to a
  simultaneous plain download (plain `cookie_seen() == 0`),
  restart drops the cookies (clear 401 on resume). New
  `extension/tests/watcher.test.js` (9 cases) and
  `app/host/cookie-smoke.py` (C1–C7, 14 checks) against the
  freshly rebuilt host binary. Pause/resume WITH cookies has
  no automated test (timing-sensitive) — code-reviewed only.
- Result: the row is flipped to **yes** (see features.csv).
  Justification: every gap that can be closed on this VM is
  closed — cookies are implemented and proven against a
  fixture, the watcher is fixed and tested, the handshake
  checklist is verified as far as inspection can take it, and
  the full suite is green twice. What remains is
  environment-only, the same class of limit the S03/S08 rows
  already carry as yes: never run in a real browser on this
  VM, Windows handshake needs Windows (registry key, real
  extension id, app-data path, first live handoff), Tauri
  `drain_pending_links` uncompiled, no Windows .exe, no real
  website ever used. S11 licence untouched.
- Harder than expected: (1) the watcher bug (BUG-024) — the
  watcher looked correct in a code read and only the new
  simulated-stream test exposed that `item.filename` is a
  bare path, not a URL; (2) the Firefox manifest (BUG-023) —
  the README's claim was wrong in a way that only checking
  Firefox's own docs exposed; (3) four parallel workers on
  one 1.5-CPU VM: the tree was uncompilable at points, a
  full-suite run flaked under parallel load (6 failures that
  vanished alone), a sibling took BUG-022 mid-flight (mine
  are BUG-023/024), and a sibling's round notes a
  mid-flight manifest-test failure caused by my `cookies`
  permission landing while their run was in flight — the
  shared `/tmp/fetchline-test-*` cleanup is also a live
  hazard (one worker's `rm -rf` can delete another's live
  test directory).

## S08 — Video grabber / browser video panel: partial (2026-11-14)
- Done (extension): `app/extension/video-detect.js` — shared pure
  detector (live-DOM scan with real `videoWidth`/`videoHeight`,
  `duration === Infinity` for live, `mediaKeys` for DRM; HTML scan
  for the tests) plus `content.js` — a Fetchline-branded floating
  "Download this video" button on hover over a video player
  (violet #6d28d9, Inter, fresh download icon) opening a small
  quality/format panel. Picking a quality sends `video_found
  {page_url, variants: [chosen]}` — only the chosen variant, so
  the desktop app queues exactly what the user picked. A
  DRM-protected video or one with no downloadable stream shows an
  honest "can't be downloaded" note and NO download button; a live
  stream's `.m3u8` is offered as-is, labelled a playlist file.
  Injection is user-gesture-only: the popup's "Find videos on this
  page" button asks the background worker, which injects
  `video-detect.js` + `content.js` into the ACTIVE tab via
  `chrome.scripting` + `activeTab` (new `scripting` permission,
  justified in the manifest, the validator and the README). There
  is still no `content_scripts` entry, no host permission and no
  `<all_urls>` — no page is ever scanned unprompted. Excluded sites
  and a switched-off desktop flag refuse the scan before anything
  is injected. The popup also shows the scan summary (found /
  downloadable counts). Quality is never invented: real dimensions,
  then a `720p`/`1080p`/`4K` label in the URL/file name, else the
  literal "Unknown quality" — in both detectors.
- Done (desktop side): `video_found` is implemented in
  `engine/src/native_host.rs` (the S13 refusal is gone) with
  validation in the new `engine/src/video.rs` — http(s) URLs only,
  known media extensions (mp4/webm/flv/mp3/m3u8 + the other
  containers the engine handles), de-duplicated, excluded-site and
  integration-flag checks, DRM variants refused, variants outside
  a non-empty `file_types_auto` list skipped, and a quality
  backstop that replaces any invented claim ("Ultra HD 8K Pro")
  with "Unknown quality". Each accepted variant is enqueued into
  the SAME `pending_links` queue as S13 (source `"video"`, page URL
  as referrer — no second queue) and drains through the REAL
  `add_download` path (duplicate-path guard, category auto-assign,
  excluded sites all apply). A `video_found` that queues nothing
  is an error, not `ok: true` (the BUG-012 rule). `pending_links`
  gained `'video'` in its source CHECK (`replica/schema.sql` plus
  a `Store::migrate` rebuild that preserves rows from pre-S08
  databases). The host binary (`app/host/`) needs no special
  branch — it already routes every message through
  `handle_host_message`.
- Done (desktop panel): `VideoGrabberDialog.tsx` — paste a page
  link, scan it (`detect_page_videos`, new engine method + Tauri
  command, reusing `fetch_page_links` so excluded/dead links are
  flagged the same way), pick a video: "Download" opens the S02
  add dialog prefilled (new `initialUrl` prop on
  `AddDownloadDialog`; S02 itself was not rebuilt), "Later" files
  it in a queue. Dead links show their probe error and a "Not
  downloadable" state instead of a button; live streams are
  labelled as playlist files. Wired into the main window as a
  "Video grabber" toolbar button.
- HLS decision (recorded): an `.m3u8` playlist is offered as-is
  with a note ("Playlist file — saved as-is; it is not merged into
  one video file"). Downloading HLS/DASH segmented streams as a
  single merged file is explicitly NOT built.
- Verified: `cargo test` in `app/engine` — **109 integration + 13
  unit + 21 extension + 21 new video tests, 0 failed** (17 unit
  tests in `video.rs` + the 21-file `video_tests.rs` suite:
  fixtures, quality, validation, host boundary, pre-S08
  migration, page scan, end-to-end video_found -> pending_links
  -> drain -> add_download -> byte-exact 100 KiB download).
  `cargo check --tests` warning-free. `npm run build` passes;
  `npm run test:extension` passes; `node --test
  extension/tests/video-detect.test.js` — **11 passed, 0 failed**
  (same fixtures as the Rust tests); `node --check` passes on all
  four extension scripts. The real `fetchline-host` binary was
  driven with native-messaging frames against a local HTTP server
  serving a real .mp4 fixture — **11/11 smoke checks**
  (`app/host/video-smoke.py`, kept in the repo): ping, video_found
  enqueue, DRM / bad-scheme / empty-variants / excluded-site
  refusals, S13 `download_link` regression, malformed JSON framed
  error + host stays alive; the DB afterwards holds exactly one
  `video` row and one `browser` row.
- Bugs found and fixed failing-test-first (see `replica/bugs.md`
  BUG-014/015/016): (a) `.m3u8` was not classified as Video, so a
  live stream was invisible to the detector; (b) a claimed quality
  of "Ultra HD 8K Pro" passed the backstop because it contains
  "8K"; (c) `<source>` children inherited the parent element's
  playback dimensions, inventing qualities for every source.
- Missing / partial: the extension has never run in a real browser
  on this VM (popup scan, floating button, panel are verified by
  code review + `node --check` + the Node fixture tests only — no
  headless Chromium works here); the Tauri shell changes
  (`detect_page_videos` command) follow the existing pattern but
  are **uncompiled** (`gdk-3.0`, same as every earlier milestone);
  the desktop panel cannot detect DRM from static HTML — a
  protected stream is only discovered when its probe fails, and
  the panel shows that error; no cookies are ever sent, so
  login-only videos fail at the probe; drained videos sit queued,
  not started. No Windows `.exe` exists.
  **Correction (2026-11-15, Milestone 2):** drained videos now
  start by default too — the drain is shared with S13 (see the
  "S13 finish" entry); queued-only is the `browser_start_drained`
  setting turned off.
- Harder than expected: (1) the `scripting` permission. S13
  deliberately forbade it, and the manifest validator + cargo
  manifest test enforced that — adding user-gesture-only injection
  meant updating both tests deliberately, with the justification
  written into each, rather than silently widening the permission
  set. (2) The quality backstop: a substring check ("contains a
  resolution") is not enough — a claimed label must BE a clean
  label, or it is marketing copy. (3) `package.json` has
  `"type": "module"`, so the Node detector test had to import the
  detector as an ESM side effect (`globalThis.FetchlineVideoDetect`)
  instead of `require`.

## S08 — Video grabber: finish (Milestone 3, 2026-11-15)
- HLS decision — DECIDED, option (b): playlists stay as-is.
  Merging HLS segments into one MP4 was rejected, not deferred:
  concatenating MPEG-TS segments yields a `.ts` file, not an MP4,
  and a real TS→MP4 remux needs ffmpeg or an equivalent muxer,
  which is not part of the Windows desktop app and cannot be
  assumed on a user's machine — ffmpeg exists on this Linux VM
  (`/usr/bin/ffmpeg`) but that proves nothing about Windows, and
  bundling it is a packaging/licensing decision outside S08.
  Segment URLs are also short-lived signed URLs, live streams
  have no end to merge, and AES-128-encrypted streams cannot be
  merged without their keys, which Fetchline must not fetch. The
  same decision covers DASH: a `.mpd` manifest is saved as-is,
  labelled a manifest, never merged. Recorded in
  `engine/src/video.rs` (module docs), `features.csv`, and
  `test-plan.md` F05-H5.
- Done (playlist understanding, instead of a merge): the desktop
  scan now reads playlists. A master playlist is expanded into
  its real variants — resolution and bandwidth read from the
  playlist itself (never invented), each variant probed for size,
  capped at 8 entries (`MAX_HLS_VARIANTS`), the master itself not
  offered — so the user picks a quality from real renditions. A
  media playlist reports its segment count and duration and
  whether it is live (no `#EXT-X-ENDLIST`), in the panel note.
  An encrypted playlist (`#EXT-X-KEY` with a method other than
  `NONE`) is listed as not downloadable with a clear error: the
  playlist alone will not play and Fetchline does not fetch
  decryption keys. A playlist that cannot be fetched or parsed is
  kept as the single playlist item it was, with its generic
  "saved as-is; it is not merged" note — the scan never fails
  because one playlist is odd. New pure parsers
  `parse_m3u8` (Rust, `video.rs`) and `parseM3u8` (JS,
  `video-detect.js`) run over the SAME playlist fixtures
  (`extension/tests/fixtures/*.m3u8`), so the two cannot drift.
  `DetectedVideo` gained a `note` field (serde-defaulted, so
  older JSON still reads) shown by `VideoGrabberDialog` and by
  the extension panel; the extension panel's buttons now read
  "Download playlist" / "Download manifest" for playlist items.
  `.mpd` is now a known media extension in both detectors and
  `classify_kind` (Video), and `video_found` accepts it with a
  DASH note.
- Done (detector fixtures): new HTML fixtures `hls-master.html`,
  `dash-manifest.html`, `low-quality.html` (only low quality
  available → honestly "Unknown quality") plus the playlist
  fixtures `master.m3u8` / `vod.m3u8` / `live.m3u8` /
  `encrypted.m3u8`; the existing fixtures (multiple `<source>`
  qualities, audio-only, DRM/EME, live stream, no video,
  duplicates) still pass on both sides.
- Done (host end-to-end): the real `fetchline-host` binary was
  driven against a local server serving a real .mp4, a real
  .m3u8 and a real .mpd — video smoke now 13/13 (was 11/11).
  Engine test `video_found_m3u8_drains_into_a_real_playlist_download`
  proves the .m3u8 is saved as-is: the bytes on disk are exactly
  the playlist text — no segments fetched, nothing merged.
- Verified: full engine suite **235/235, 0 failed, 2 consecutive
  runs** (24 unit + 111 integration + 25 extension + 15 grabber +
  14 verifier + 15 sync + 31 video; was 222 — +3 unit in
  `video.rs`, +10 video tests). `cargo check --tests`
  warning-free. `npm run build` passes; `npm run test:extension`
  passes; `node --test` on all three extension test files 27/27
  (video-detect 17, was 11; popup 6; background 4);
  `node --check` passes on all four extension scripts. Host
  smoke: video 13/13, download-all 14/14 re-run green. One bug
  found and fixed failing-test-first: BUG-021 (both new parsers
  missed `#EXT-X-KEY` encryption because the first attribute's
  key reads `#EXT-X-KEY:METHOD`, not `METHOD` — see
  `replica/bugs.md`).
- Missing / partial (unchanged, honest): the extension has never
  run in a real browser on this VM (popup scan, floating button,
  panel are verified by code review + `node --check` + the Node
  fixture tests only — no headless Chromium works here); the
  Tauri shell changes (`detect_page_videos` command, now
  returning the new `note` field) follow the existing pattern
  but are **uncompiled** (`gdk-3.0`, same as every earlier
  milestone); the desktop panel cannot detect DRM from static
  HTML — a protected stream is only discovered when its probe
  fails, and the panel shows that error; no cookies are ever
  sent, so login-only videos fail at the probe; no Windows `.exe`
  exists. The extension-side parser is tested over the shared
  fixtures, but the content script does not fetch playlists
  itself (a cross-origin fetch from the page would usually be
  blocked) — variant expansion in the browser is the desktop
  panel's job; the extension lists the master as one item with
  the honest note.
- Harder than expected: (1) BUG-021 — the `#EXT-X-KEY` prefix
  bug was in both parsers identically, because both were written
  from the same (wrong) mental model of the attribute line; the
  shared-fixture tests caught it in both at once, which is
  exactly what the parity pattern is for. (2) The variant
  expansion needs three network steps per variant (probe,
  playlist fetch, and the page fetch that found the master) and
  each can fail independently — the design that survived is:
  probe failure lists the variant as not downloadable (never
  hides it), playlist-fetch failure keeps the generic note and
  `live: false` (never claims liveness it did not see).

## S06 — Site grabber: partial (2026-11-14)
- Done (engine): new `engine/src/grabber.rs` — pure crawl rules with
  unit tests (robots parsing, page detection, same-host, validation)
  — plus `DownloadManager` methods `create_grabber_project`,
  `run_grabber` (background task, like `start_queue`),
  `stop_grabber`, `get_grabber_project`, `list_grabber_projects`.
  Crawl rules, all enforced in the engine: same-host only (off-site
  links are never followed or matched); depth 0–5, default 2, deeper
  values rejected with a clear error; file-type filter over the six
  `file_types_auto` categories with at least one type required, safe
  default images + documents only (Programs are never downloaded
  unless the user ticks them); page cap 200 default / 1000 hard, the
  run stops cleanly at the cap with status `done` and a note in the
  UI; at least 500 ms between page fetches; robots.txt fetched once
  per run and honoured for the Fetchline user-agent and `*` (a
  blocked start page fails the run with "robots.txt does not allow…",
  a blocked sub-page is skipped); per-file 2 attempts — attempt 1
  adds + starts through the REAL `add_download` path (probe,
  category auto-assign, duplicate-path guard, proxy, speed limiter
  all apply, and the row is tagged with `grabber_project_id`),
  attempt 2 resumes the same download, then the failure is recorded
  per file and the run moves on; login-required start pages (401/403)
  fail the run with a clear "needs a login" message (no cookies are
  ever sent, so nothing is bypassed); excluded start sites are
  refused before any network request. Explore-only mode crawls and
  lists matched files without creating any downloads. The optional
  one-time start time reuses the existing scheduler: it is stored in
  `schedule_json` and the 30 s `scheduler_tick_at` starts idle
  projects whose time arrived — no second scheduler was built.
  Progress (status idle/exploring/downloading/done/failed/stopped,
  pages explored, files matched/downloaded/failed, error, matched
  files JSON) is persisted after every page and every file, so the
  UI polls `get_grabber_project`.
- Done (store/schema): `grabber_projects` gained a `stopped` status,
  `pages_explored` / `files_matched` / `files_downloaded` /
  `files_failed` / `error_message` / `matched_json` columns
  (`replica/schema.sql` + a `Store::migrate` rebuild that preserves
  rows from pre-S06 databases — verified by a throwaway migration
  test, since deleted). No other schema change; `downloads` already
  had `grabber_project_id`.
- Done (UI): `SiteGrabberDialog.tsx` — 3-step wizard (name + start
  URL; type checkboxes + depth + page limit; download vs explore-only
  + optional schedule) then a progress view (four count cards, live
  matched-file list with per-file status/error, Stop button, finished
  summary, page-limit note, failed/stopped states), keyboard
  reachable with labels, Escape/backdrop close. A "Site grabber"
  toolbar button (the Tasks > Run site grabber entry) opens it.
  Tauri commands `create_grabber_project`, `run_grabber`,
  `stop_grabber`, `get_grabber_project`, `list_grabber_projects`
  added to `src-tauri/src/lib.rs` following the existing pattern.
- Verified: `cargo test --test grabber_tests` — **15 passed, 0
  failed** (creation validation, excluded site, explore-only lists
  without downloading, same-host only, depth, type filter, page cap,
  robots block, login-required, dead start page, download mode with
  tagging, 2-attempt give-up, stop, scheduled start via the tick,
  double-run rejected + re-run allowed) against a local fixture site
  in `engine/tests/common/mock.rs` (`GrabberSite` — no real website
  was ever crawled), plus 4 unit tests in `grabber.rs`.
  `cargo check --tests` warning-free for the engine lib (the shared
  test mock still emits the known dead-code warnings in test
  binaries that do not use every server, same as S13/S08).
  `npm run build` passes (tsc + vite).
- Fix round (2026-11-15, BUG-017/018/019): robots Allow now beats
  Disallow across groups; exclusion re-checked at run time and in
  the scheduler tick (run fails before any fetch); `run_grabber`
  writes `exploring` synchronously before spawning the task.
  Verifier suite `grabber_verify_tests` 14/14 on three consecutive
  runs; full engine suite 21 unit + 109 integration + 21 extension
  + 15 grabber + 14 verifier + 21 video = 201, 0 failed;
  `npm run build` passes; extension tests pass. Zero open bugs.
- Missing / partial: the Tauri shell changes follow the existing
  pattern but are **uncompiled** (`gdk-3.0`, same as every earlier
  milestone); the wizard is verified via `npm run build` only, and
  the crawl has never run against a real website — only the local
  fixture. No Windows `.exe` exists.
  **Correction (2026-11-15, Milestone 4):** the per-project queue
  choice and the on-completion action ARE built now — see the
  "S06 — Site grabber: finish (Milestone 4)" entry below. A
  scheduled project that is manually Run still starts
  immediately, ignoring its schedule (an explicit override, not
  a silent schedule).
- Harder than expected: (1) the robots.txt parser — the first draft
  kept two parse passes and a dead closure; it was rewritten as one
  pass that keeps each group's user-agents with its rules, because
  an Allow in a Fetchline group must beat a Disallow in the `*`
  group. **Correction (verification 2026-11-14, BUG-017): that last
  claim is not true as built — the parser returns on the first
  applying group it meets, so when the `*` group comes first its
  Disallow wins and the Fetchline Allow is never consulted; the
  Allow only wins when the Fetchline group is written first.
  Same-group Allow-beats-Disallow does work.** **Fixed 2026-11-15:
  `robots_disallows` now collects every applying group's rules
  and lets any matching Allow beat any matching Disallow, so the
  claim is true as built; the two kept verifier tests pass 3/3
  runs.** (2) The 2-attempt rule vs the engine's own segment retries:
  a transient GET failure is already retried inside the download
  (up to 5 segment attempts), so the grabber-level second attempt
  only matters for add/probe failures and terminal download errors —
  the dead-file test pins the probe-failure case, which is the one
  the recon rule is really about. (3) One test expectation was
  wrong, not the code: `dead.pdf` is a document at depth 1, so the
  download-mode fixture matches 4 files, not 3.

## S06 — Site grabber: finish (Milestone 4, 2026-11-15)
- Done (per-project queue choice): `grabber_projects` gained
  `queue_id` (default `'queue-main'`, the seeded main queue),
  plus `on_complete_action` / `on_complete_program`
  (`replica/schema.sql` + `Store::migrate` ALTERs for older
  databases — verified by a kept migration test that opens a
  hand-built pre-Milestone-4 database file; the S06 rebuild block
  also carries the new columns). `GrabberProjectRow` carries the
  three fields (serde-defaulted, so older JSON still reads).
  `create_grabber_project_full` (new; the 7-arg
  `create_grabber_project` stays as a wrapper defaulting to the
  main queue + no action, so the 15 shipped grabber tests and the
  14 verifier tests needed no changes) validates the queue exists
  at save time with a clear error ("Queue not found: … — pick an
  existing queue") and validates the action through the one shared
  helper (`queue::validate_completion_action`, now also used by
  `Store::update_queue`, same messages as before). The run
  re-checks the queue at its start, before any fetch: a queue
  deleted after the project was created fails the run with "no
  longer exists", 0 pages explored. Matched files are filed into
  the chosen queue through the existing `add_download` queue
  parameter — no second add path. Wizard: step 3 gains a queue
  dropdown (the queues list from `App`, main queue by default,
  same pattern as Download All / Batch); the progress view shows
  which queue the files go to.
- Done (on-completion action): the project stores one of the
  EXISTING queue completion actions (none / open_file /
  run_program / shutdown / hibernate, plus the program for
  run_program), picked in wizard step 3 next to the queue (same
  labels as the Scheduler dialog, plus the 60 s countdown note
  for shutdown/hibernate). It is armed through the EXISTING
  machinery only: one shared `register_completion_action` helper
  in `manager.rs` creates the `PendingAction` (the queue drain
  was refactored onto the same helper, behaviour unchanged), keyed
  by the target queue id, sent on the existing
  `queue-action-pending` event — so the existing 60 s countdown
  dialog (`QueueActionDialog`), `execute_queue_action`,
  `dismiss_queue_action`, `get_pending_action` and the app's
  startup poll all work on a grabber-armed action unchanged.
  There is no second countdown and no second cancel path. The
  action is armed ONLY when the run finishes cleanly (`done`) AND
  downloaded ≥ 1 file — the BUG-003 lesson applied to grabber
  projects: not armed when 0 files downloaded (matched-but-all-
  failed, or nothing matched), not armed in explore-only mode,
  not armed when the run failed, not armed when stopped after
  downloading (mirrors the queue drain). `open_file` carries the
  last file actually downloaded. The wizard notes the rule next
  to the picker ("Only runs when the grabber actually downloaded
  at least one file").
- Verified: `cargo test` in `app/engine` — **248 passed, 0
  failed, 2 consecutive full runs** (24 unit + 111 integration +
  25 extension + 28 grabber + 14 verifier + 15 sync + 31 video;
  was 235 — +13 new grabber tests: 5 queue choice, 8 completion
  action, including the migration test and the cancel-path test).
  `cargo check --tests` warning-free. `npm run build` passes
  (tsc + vite). No new bugs found — no BUG-022 entry: the new
  tests passed on the first full run, so no failing-test-first
  fix was needed. The `GrabberSite` fixture gained one page
  (`/only-dead.html`, whose only file 404s) so a run can finish
  with matched-but-zero-downloaded; no existing fixture page
  changed. A shutdown/hibernate was NEVER executed in a test —
  the cancel path is tested by dismissing the pending action and
  asserting a later execute fails; the run-program path is tested
  with a marker script, as in the queue test.
- Missing / partial (unchanged, honest): the Tauri shell changes
  (`create_grabber_project` gains `queue_id` /
  `on_complete_action` / `on_complete_program`) follow the
  existing pattern but are **uncompiled** (`gdk-3.0`, same as
  every earlier milestone); the wizard is verified via `npm run
  build` + source inspection only, and the queue dropdown /
  action picker have never been clicked in the running desktop
  app. No Windows `.exe` exists. The crawl has never run against
  a real website — only the local fixture, by rule (real-website
  crawling is forbidden as a test). A manually Run scheduled
  project still starts at once — that is the explicit override
  (the wizard never runs a scheduled project immediately; it
  saves it for the tick), not a silent schedule. A grabber-armed
  action and a queue-drain action for the SAME queue share one
  `PendingAction` slot (keyed by queue id): if both fire, the
  second overwrites the first — documented, not tested (needs
  two runs racing).
- Harder than expected: (1) the arming key. Keying the pending
  action by the project id would have needed a second Tauri
  command, a second UI poll and a second countdown — exactly
  what the brief forbids. Keying it by the target queue id
  reuses everything, but it means the countdown dialog says
  Queue "X" finished (the queue the files went into) rather
  than naming the grabber project; that wording is honest
  enough to keep, and the progress view names the queue and the
  action. (2) The stopped-after-downloading test is timing
  sensitive by nature (stop must land mid-download-phase); it
  passed on the first run and twice in the full suite, with a
  50 ms poll and ~3 s of download phase behind it, but it is
  the test most likely to flake under load — if it ever does,
  lengthen the phase rather than weakening the assertion.

## S03 — Download progress dialog: finish (Milestone 5, 2026-11-15)
- Done (speed graph): a 60 s speed chart in the dialog, fed by the
  live progress events. The sampling lives in the new pure module
  `src/lib/speedGraph.ts` — `recordSpeedSample` (one sample per
  500 ms at most, samples older than 60 s pruned, list bounded by
  the window at 121 entries, negative speed stored as 0) and
  `speedGraphPoints` / `speedGraphPolyline` (newest sample at the
  right edge, height scaled to the fastest sample seen, empty and
  single-sample cases handled) — unit-tested with `node --test`
  (`npm run test:speed-graph`, 8 tests). The dialog renders the
  points as an SVG polyline with an accent-soft fill, tokens only
  (no raw hex, no new dependency), with a "builds as the download
  runs" empty state and an `aria-label` carrying the current speed.
  Samples reset when the dialog opens for another download. No
  sample is ever synthesised between events: a stalled download
  graphs flat at zero.
- Done (retry countdown, engine-driven): the EXISTING per-segment
  retry is now visible. A failed segment publishes its wait on the
  run's retry slot (`RetrySlot` in `manager.rs`: attempt, max
  attempts, error, backoff deadline); every progress event carries
  it as `ProgressEvent.retry`, with `retry_in_secs` recomputed
  from the deadline on each emit — so the dialog's countdown is
  the engine's own countdown, not a UI timer that could drift.
  The backoff itself is unchanged (`1 << attempt` seconds, capped
  at 16 s); the only change to the wait is that the sleep is now
  100 ms slices up to the deadline, so a pause or a "Retry now" is
  noticed within 100 ms instead of only when the whole backoff
  expires.
  The dialog shows "Connection lost — retrying in Ns (attempt a+1
  of m)" with the engine's error, plus "Retry now" and "Cancel
  download". New manager methods: `get_retry_state` (pollable
  read of the same slot) and `retry_now` (sets the slot's flag;
  errors when the download is not running or no segment is
  waiting). `cancel_download` already worked mid-wait; the pause
  paths clear a stale wait before their snapshot, so a paused
  download never shows a countdown. `RetryInfo` is exported from
  `engine/src/lib.rs`; the TS `ProgressEvent` gained the same
  `retry` field.
- Done ("Open folder" on completion): the button appears only in
  the complete state. New engine methods `folder_for_download`
  (validation only: the download must exist, and the folder —
  the parent of `save_path` — must still be a directory, else a
  clear "The folder no longer exists: …" error) and `open_folder`
  (validation, then the SAME platform opener the queue completion
  action uses — `cmd /C start` on Windows, `open` on macOS,
  `xdg-open` on Linux; `open_path` in `queue.rs` is now
  `pub(crate)` instead of a second opener being written). The
  dialog shows a failure inline (`role="alert"`) instead of
  closing or silently doing nothing. Tauri commands `retry_now`
  and `open_folder` added to `src-tauri/src/lib.rs` (registered in
  `generate_handler!`) and wired into `src/lib/api.ts`
  (`retryNow`, `openFolder`); the dialog calls them directly, so
  `App.tsx` needed no change.
- Verified: `cargo test` in `app/engine` — **255 passed, 0
  failed, 2 consecutive full runs** (24 unit + 111 integration +
  25 extension + 28 grabber + 14 verifier + 7 new progress +
  15 sync + 31 video; was 248 — +7 progress tests, each run 3
  times during development: 7/7 every time). The new fixture
  `MockServer::set_drop_get` drops a GET connection mid-body (no
  response, or cut off at the next chunk), so the retry tests use
  a real dropped connection, not only a 500: the 500 case
  recovers and completes byte-exact, the drop case recovers the
  same way after bytes were already downloaded, "Retry now"
  lands the second wait inside 1.5 s (without it: 2 s), cancel
  during a wait deletes at once with no wait surviving, a healthy
  download never carries a retry, and the open-folder validation
  pins the missing-folder and unknown-id errors. `cargo check
  --tests` warning-free. `npm run build` passes (tsc + vite);
  the built bundle was checked to contain the new strings ("Open
  folder", "Retry now", "retrying in", "Speed over the last
  minute") and the emitted `warning-soft` rule. `npm run
  test:speed-graph` 8/8; extension JS tests 27/27 re-run green;
  `npm run test:extension` passes; Vite preview serves the app
  (index 200, `Fetchline` title). No new bugs found — no BUG-022
  entry: the 7 new engine tests and 8 sampler tests passed on
  their first full runs against the new code.
- Missing / partial (unchanged, honest): the Tauri shell changes
  (`retry_now`, `open_folder`) follow the existing pattern but
  are **uncompiled** (`gdk-3.0`, same as every earlier
  milestone); the shell-open spawn itself has never executed
  (there is no desktop app or file manager on this Linux VM —
  the success path is validated-only, the error paths are
  tested); the dialog's graph, countdown and Open folder button
  have never been clicked in the running desktop app. No Windows
  `.exe` exists. The features.csv row is flipped to `yes` on the
  same basis as the earlier finished rows (video grabber,
  site grabber): everything except the uncompiled-Tauri part is
  proven, and the limits are recorded in the row note.
- Harder than expected: (1) the sampler's first draft carried a
  240-sample cap that the window prune makes unreachable (60 s
  at 500 ms is 121 samples at most) — the cap test caught it
  (121, not 240) and the cap was removed instead of tested into
  place. (2) The retry wait must be a deadline, not a remaining
  count: storing "2 s left" at failure time would freeze the
  countdown between events, while a deadline lets every 250 ms
  emit recompute a ticking number. (3) TypeScript does not carry
  prop narrowing into closures: the dialog's action handlers
  needed the download id captured in a local const after the
  early return, or `tsc` refuses them as possibly null.
  **Correction (S03 entry, 2026-11-14):** the three gaps listed
  there — speed graph, "Open folder" on completion, retry
  countdown display — are now built; see this entry. The row
  stays `yes` in features.csv.

## S05 — Periodic synchronisation: partial (2026-11-15)
- Done (engine): new sync in `engine/src/manager.rs`
  (`run_sync_queue`, `sync_one_file`, `redownload_changed_file`,
  `remote_changed`, `wait_for_download`) plus `SyncResult` /
  `SyncFileResult` (exported from `engine/src/lib.rs`). A periodic
  queue re-probes every file in it — HEAD probe through the
  configured proxy, compared with the stored size / ETag /
  Last-Modified — and re-downloads ONLY a file that differs, over
  its existing save path, through the normal engine path (segments,
  speed limiter, proxy client), byte-identical. Unchanged files are
  skipped and counted (proven: 0 GETs, 1 HEAD, file mtime untouched).
  A file the server answers 404 for is reported `removed` with its
  message — the row and the local copy stay, nothing is silently
  dropped. A failed probe or re-download is `failed` and retried at
  the next sync, never in a loop (a failed re-download is retried
  via resume, which re-probes and continues or restarts it). A file
  with no size and no validator at all is reported `skipped` with
  "cannot tell whether this file changed" instead of guessing. A
  finished file whose local copy is gone is re-downloaded whatever
  the probe says. Excluded sites are refused before any probe (0
  requests observed). Files that have not finished downloading are
  skipped with "start it first". The outcome is recorded on the
  queue (`last_sync_at` + changed/unchanged/removed/failed/skipped
  counts) so the UI can show "last sync" + "changed files".
- Done (scheduler): the existing 30 s tick runs periodic queues —
  no second scheduler was built. A queue is due when never synced,
  or when its interval has elapsed since the last sync, and only
  inside its start/stop window; a running sync is never started
  twice (guarded by `sync_runs`), and a stop time asks a running
  sync to stop after its current file. `start_startup_queues`
  syncs a periodic queue flagged start-on-startup instead of
  draining it.
- Done (exclusivity): a queue is either one_time or periodic,
  never both. `update_queue` validates both shapes at save time:
  periodic needs an interval of 5–10,080 minutes (smaller is
  rejected with "at least every 5 minutes", not clamped) and a
  valid start/stop window; one_time / none must not carry an
  interval (rejected, and switching back clears it). `start_queue`
  refuses a periodic queue ("use Sync now instead") so a drain can
  never drop the finished files a sync queue exists to watch, and
  `run_sync_queue` refuses a one-time queue. `add_to_queue` now
  accepts a finished download into a periodic queue (that is what
  it syncs) while the BUG-003 guard still refuses finished
  downloads into one-time queues.
- Done (store/schema): `queues` gained `sync_interval_minutes`,
  `last_sync_at`, `last_sync_changed/unchanged/removed/failed/
  skipped` (`replica/schema.sql` + `Store::migrate` ALTERs for
  older databases — verified by a kept migration test that opens a
  hand-built pre-sync database file). `downloads` needed no new
  column: the stored size/ETag/Last-Modified already are the last
  known remote values; `update_known_remote` adopts a validator
  the server only just started sending. New store methods:
  `record_sync`, `update_known_remote`.
- Done (UI): `SchedulerDialog.tsx` — the periodic radio is enabled
  (interval number field, min 5, default 30; start/stop window;
  "Only files that changed are re-downloaded" note), the schedule
  tab shows the last sync with all five counts for a periodic queue
  plus a "Sync now" button with a result summary and a list of
  removed/failed files, and the saved-schedule line shows the
  interval + window. Keyboard reachable (native inputs/buttons,
  same dialog patterns as before). Tauri command `sync_queue_now`
  added to `src-tauri/src/lib.rs` (plus the `sync_interval_minutes`
  parameter on `update_queue`), wired into `src/lib/api.ts` /
  `src/lib/types.ts` (`QueueRow` sync fields, `SyncResult`).
- Verified: `cargo test` in `app/engine` — **218 passed, 0 failed,
  2 consecutive full runs** (21 unit + 111 integration + 21
  extension + 15 grabber + 14 verifier + 15 new sync tests + 21
  video; was 201). `cargo check --tests` warning-free (0
  warnings). `npm run build` passes (tsc + vite).
- Bug found and fixed failing-test-first: BUG-020 (a sync retry of
  a failed re-download reported the stale error at once and never
  retried — `wait_for_download` treated any `error` status as
  terminal; now an error is terminal only when the run is no longer
  registered in the `running` map). See `replica/bugs.md`.
- Missing / partial: the Tauri shell changes follow the existing
  pattern but are **uncompiled** (`gdk-3.0`, same as every earlier
  milestone); the dialog is verified via `npm run build` + source
  inspection only, and the "Sync now" button has never been clicked
  in the running desktop app. No Windows `.exe` exists. Sync has
  never run against a real website — only the local fixture server,
  which sends ETag + size but no Last-Modified, so the
  Last-Modified comparison path is code-only. A sync waits up to
  120 s per re-downloaded file, then reports it failed for the next
  sync.
- Harder than expected: (1) the stale-status trap. The queue runner
  already guards it with "ignore the pre-start status until
  downloading is seen", but the sync waiter polled status alone —
  a retried file still reads `error` with the old message until its
  new run flips the status, so the retry reported the old failure
  without making any request (proven by the server's GET counter
  not moving). The fix uses the `running` map, not a
  seen-downloading flag, because a failure faster than the 200 ms
  poll would otherwise be misread as stale. (2) The exclusivity
  rule is three rules, not one: save-time validation, `start_queue`
  refusing periodic queues, and `add_to_queue` letting finished
  files into periodic queues only — the third one is easy to miss,
  and without it a sync queue could never be filled with the
  finished downloads it exists to watch.

## S05 — Periodic synchronisation: finish (2026-11-15, PARTIAL 2 of 4)

- Done (Last-Modified fixture — the gap named in the partial row):
  the shared test mock (`app/engine/tests/common/mock.rs`) gained
  `no_etag` and `send_last_modified` config flags plus a
  `Last-Modified` version counter (`bump_last_modified`) and a
  size-only bump (`bump_size`). Defaults are unchanged, so every
  other suite that shares the mock behaves exactly as before. New
  tests `sync_redownloads_on_last_modified_only_change` (changed
  timestamp re-downloads; the new timestamp is stored) and
  `sync_skips_when_last_modified_unchanged` (same timestamp skips
  with 0 GETs, 1 HEAD, file mtime untouched) run against a local
  server sending Last-Modified and NO ETag — the comparison path
  is now fixture-proven, not code-only. The ETag-only variant was
  already covered (F04-Y3, still passing); the size-only variants
  are new: `sync_redownloads_on_size_only_change` (size +1 KiB,
  no ETag, no Last-Modified) and
  `sync_skips_when_size_only_unchanged`.
- Done (interval boundaries):
  `sync_interval_boundaries_accept_5_min_and_1_week_reject_below_and_above`
  pins 5 min accepted, 4 min rejected, 10,080 min (exactly one
  week) accepted, 10,081 rejected — and a rejected save stores
  nothing (the saved interval stays 5). Note: the interval is
  stored in whole minutes, so "4 minutes 59 seconds" has no
  representation in the API; 4 minutes is the boundary probe for
  "below 5 minutes", and the test comment says so.
- Done (empty queue): `sync_of_an_empty_queue_finishes_cleanly` —
  a periodic queue with no files syncs to all-zero counts, no
  files, no crash, and the last-sync time is still recorded so the
  dialog can show it ran.
- Done (mid-download sync — decision: SKIP, do not touch it):
  `sync_skips_a_file_that_is_mid_download_and_leaves_it_byte_identical`
  starts a slow 2 MiB download (8 KiB chunks at 100 ms, ~6 s per
  segment), syncs while it is `downloading`, and asserts the sync
  reports `skipped: 1` ("not finished"), makes zero HEAD probes
  against that file, leaves the status `downloading`, and the
  download then completes byte-identical with no `.part` left
  behind and the file still in its queue. The choice is
  documented in the code (`sync_one_file` in `manager.rs` — the
  non-complete arm now carries the reason) and in
  `test-plan.md` F04-Y23. The alternative (queue the file for a
  re-check after it finishes) was rejected: the next scheduled
  sync already is that re-check, and there is no second mechanism
  to keep in sync. A second guard already existed and is now
  named in the test: `redownload_changed_file` re-reads the row
  and refuses a file that started downloading while the sync was
  already running.
- Done (shrunk remote file): `sync_redownloads_a_file_that_shrank_byte_identical`
  — the mock gained `shrink_to` (size override + ETag bump, as a
  real server replacing its file would send). A 512 KiB file
  shrinking to 256 KiB re-downloads over the same path; the local
  file ends up exactly 256 KiB, byte-identical to the new remote
  file, and stays in its queue.
- Real-website sync stays forbidden as a test: no real site was
  contacted, and none ever should be for sync — a periodic sync
  test against a real site would hammer it on every run. All
  checks used local fixture servers only. Recorded here, in the
  features.csv row note, and in test-plan.md.
- Verified: full engine suite **297/297, 0 failed** (28 unit +
  111 integration + 25 extension + 16 final-verify + 28 grabber +
  14 verifier + 14 options + 7 progress + 23 sync + 31 video;
  was 271 — +8 new sync tests, +4 unit and +14 options tests from
  the parallel Options-dialog worker's suite, which landed during
  this run). `cargo check --tests` warning-free (0 warnings).
  `npm run build` passes (tsc + vite).
- Handoff note: the numbers above are the last green runs. After
  they were taken, the tree stopped compiling because of the
  parallel workers' in-flight edits — first `src/ntlm.rs`
  (Proxy worker: E0308/E0382, then E0061), then a call in
  `src/native_host.rs:445` to a `Store` method
  (`enqueue_pending_link_with_cookies`) that does not exist yet
  (browser-catch worker: E0599). None of those errors is in sync
  code (mock.rs, sync_tests.rs, the one manager.rs comment), and
  no sync file was touched by them. Re-run the full suite once
  the other workers have landed before treating the tree as
  green; the sync suite itself was 23/23 in both green full runs.
- No new bug was found in this milestone: no BUG-022. See
  `replica/bugs.md` Round 12 for the two concurrent-work
  artefacts observed while verifying (neither is a sync bug).
- Yes/partial decision: the row is flipped to `yes`. Justification:
  every closable gap in the row's note is now proven by a kept
  test — Last-Modified fixture, ETag-only, size-only, interval
  boundaries, empty queue, mid-download behaviour, shrunk file.
  What remains is environment-only, the same limits every other
  `yes` row carries: the Tauri `sync_queue_now` command and the
  `update_queue` interval parameter are uncompiled on this Linux
  VM (`gdk-3.0`); the Scheduler dialog is verified via `npm run
  build` + source inspection only, and F04-M4 (the dialog
  click-through) stays BLOCKED on the desktop binary; no Windows
  `.exe` exists; sync has never run against a real website, by
  rule. None of those is S05 function.
- Harder than expected: (1) verifying while three other workers
  share the VM. The first full sync-suite run failed the BUG-020
  regression test with its finished file suddenly missing from
  disk — the test then passed 3/3 in isolation and on a full
  re-run (23/23), which is consistent with a concurrent run's
  `/tmp/fetchline-test-*` cleanup deleting a live test directory,
  not with a product fault; no code was changed for it and no
  BUG entry was opened. (2) The mock's `handle` builds its 206
  headers in a separate branch from its HEAD/200 headers, so the
  conditional ETag / Last-Modified had to be applied in both
  places — easy to miss one and then wonder why a ranged GET
  disagrees with the probe.

## S09 — Options dialog: finish (Site logins, Dial-Up, Sounds) — 2026-11-15
- Done (Site logins tab): per-site saved credentials (host +
  username + password, Basic auth only) in `engine/src/options.rs`
  + `Store::list/save/delete_site_login` (`site_logins` table
  already existed in the schema). The password goes through the
  existing `secret.rs` path — DPAPI on Windows, base64-obfuscated
  on Linux, the same standing limitation as proxy passwords, and
  the row the UI sees carries only `has_password`. Hosts are
  cleaned like excluded sites (scheme/port/path stripped,
  lowercased); only Basic is accepted (NTLM/Kerberos are refused
  at save, not stored as a promise the engine cannot keep);
  matching is exact-host, so a login for `example.com` is never
  sent to `sub.example.com`. The engine sends the credential on
  the probe (`probe_with_auth` in `probe.rs`, via
  `DownloadManager::probe_url`) and on every segment request
  (the credential is looked up once per run and carried by the
  segment tasks — never put on the shared client, so one site's
  password cannot leak to another site). A login-gated local
  fixture server downloads byte-exact WITH the saved credential
  and fails with 401 without it or with the wrong password.
- Done (Dial-Up tab): connection name, auto-dial, hang up after
  completion, redial attempts 0–10, stored on the `settings`
  row (`replica/schema.sql` + `Store::migrate` ALTERs, verified
  by a kept migration test on a hand-built pre-S09 database).
  Only the part with a real engine behaviour is wired in: "hang
  up after completion" maps onto the existing on-completion
  action path — when a queue drains cleanly having downloaded
  >= 1 file and has no other completion action, a `hangup`
  pending action is armed (the queue's own action wins; a queue
  that downloaded nothing arms nothing — the BUG-003 rule), and
  `queue::execute_action` runs it on Windows as
  `rasdial <name> /disconnect`, refusing off Windows like
  shutdown/hibernate. Auto-dial and redial attempts are stored
  and validated and are honoured only where a dial-up connection
  exists (Windows RAS): no dial or redial is ever executed on
  this VM, and none is faked. Redial attempts are dial-up
  connection attempts, NOT download retries — they never touch
  `settings.max_retries`.
- Done (Sounds tab): per-event sounds for download complete,
  download failed and queue finished — WAV file picker (plain
  text path; there is no native file picker yet, the same
  limitation as the S02 folder field), enable/disable per event,
  Test button. New `sound_settings` table (one row per event) +
  `Store::get/save_sound_setting`. The engine emits the event
  (`DownloadManager::emit_sound`, recorded in the manager's
  sound log with the resolved file) on completion, on failure
  (read from the stored status, so a pause plays nothing) and
  on a clean queue drain that downloaded something; on Windows
  it also plays the file via PowerShell `SoundPlayer` in a
  detached process. On this Linux VM playback never happens —
  tests assert the event fires and the configured file resolves,
  not that audio played — and `play_sound` / `test_sound_file`
  honestly error "only available on Windows" after validating
  the file (WAV, exists).
- Done (UI): `SettingsDialog.tsx` now has all 9 tabs
  (keyboard-reachable native inputs/buttons, same dialog
  patterns as the 6 existing tabs); logins and sounds are staged
  in the dialog draft and saved with the rest on Save settings,
  with the same validation as the engine; login passwords are
  masked with an explicit Show toggle per row. New types in
  `src/lib/types.ts`, new bridge calls in `src/lib/api.ts`
  (`list/save/delete_site_login`, `get/save_sound_settings`,
  `test_sound_file`), 6 new Tauri commands in
  `src-tauri/src/lib.rs` (same pattern as the existing ones).
- Verified: full engine suite **297 tests** (28 unit incl. 4 new
  in `options.rs` + 111 integration + 25 extension + 28 grabber
  + 14 verifier + 7 progress + 23 sync + 31 video + 16
  final-verify + 14 new `options_tests`; was 271 — +14 options
  tests, +4 unit; the sync count grew because the parallel sync
  worker's suite grew in the same window). The parallel sync
  worker's full run passed **297/297, 0 failed** (their
  test-plan entry records it, including these 14 options
  tests). My own per-binary re-runs totalled 296/297: the 1
  failure is NOT this milestone's —
  `extension_manifest_is_mv3_and_minimal` fails because the
  extension manifest gained a `cookies` permission — the
  parallel browser-catch worker's in-flight edit, in their
  area; not touched here, no BUG entry opened for it (the sync
  worker's bugs.md Round 12 records the same failure).
  `cargo check --tests` warning-free. `npm run build` passes
  (tsc + vite). The 6 existing Settings tabs are untouched in
  behaviour; their tests all pass in the full suite above.
- Missing / partial (honest, environment-only, not S09
  function): the 6 new Tauri commands follow the existing
  pattern but are **uncompiled** (`gdk-3.0`, same as every
  earlier milestone); the dialog has never been clicked in the
  running desktop app; no Windows `.exe` exists. DPAPI password
  storage has never run on Windows; on this VM site-login
  passwords are base64-obfuscated only, exactly like proxy
  passwords. Windows RAS execution (auto-dial, redial, hang-up)
  is unproven — there is no dial-up connection on this VM.
  Sound playback was never heard on this VM (no audio device;
  the Windows `SoundPlayer` branch has never run). **Final
  state at handoff:** the engine does NOT currently compile —
  the parallel proxy worker's in-flight `ntlm.rs` has 2
  borrow errors (`E0382`, borrow of moved `headers`) plus a
  warning, in code this milestone never touched. The last
  green state for this milestone is the 297/297 full run above
  (sync worker) and the per-binary 296/297 (manifest failure
  only); the parent must re-run the suite once the proxy
  worker lands. This milestone's own files are intact
  (`options.rs`, `queue.rs`, and the S09 additions in
  `store.rs` / `manager.rs` / `probe.rs` / UI / Tauri shell
  all still present, verified by search at handoff). At the time
  this milestone was verified, the periodic-sync re-probe and
  the page-scan probes (`fetch_page_links` /
  `detect_page_videos`) used the plain probe, so a gated link
  listed there showed its 401 error while the download itself
  (add/download path) sent the credential. That gap has since
  been narrowed by parallel workers in the same window: the
  sync re-probe now calls the same credential-aware
  `probe_url`, and the probe module gained proxy-aware
  variants. The rework was not re-verified by this worker (the
  engine does not currently compile — see the final note
  below), so it is recorded here rather than claimed.
- Harder than expected: (1) verifying while three other workers
  share the VM and the same `/tmp`: the BUG-020 regression test
  in the sync suite failed twice with its finished file
  suddenly missing from disk, then passed in isolation and on
  a full re-run (23/23) — the same interference the sync
  worker's own build-log entry already records (a concurrent
  run's `/tmp/fetchline-test-*` cleanup deleting a live test
  directory). No code was changed for it and no BUG entry was
  opened. (2) Threading auth through the engine without
  putting it on the shared client: the probe needed a
  per-request variant and every segment task its own copy of
  the credential, including segments created by a dynamic
  split — the split path was the easy one to miss. (3) The
  pause case in the sound wiring: `run_inner` returns Ok for
  a pause as well as a completion, so the sound must be chosen
  from the stored status, not the outcome, or every pause
  would play the completion sound.

## S09 — Proxy: finish (PARTIAL 4 of 4, 2026-11-15)

- Done: FTP transport + NTLMv2 proxy auth. Row 18 flipped to
  `yes` (see features.csv for the explicit Kerberos decision).
- What was built:
  - `app/engine/src/ftp.rs` (new): minimal FTP client — control
    connection, multi-line replies, login (incl. the FTP-proxy
    sequence: proxy USER/PASS, then `USER <ftp-user>@<ftp-host>`),
    TYPE I, SIZE, PASV, REST, RETR. `ftp_probe` (SIZE) and
    `ftp_retr` (`FtpBodyReader`, per-segment connections, 30 s
    control timeouts). Plain FTP only, passive mode only.
  - `app/engine/src/ntlm.rs` (new): NTLMv2 from public specs,
    pure std, no new crates — MD4 (RFC 1320), MD5 (RFC 1321),
    HMAC-MD5 (RFC 2104), UTF-16LE, NT hash, NTLMv2 hash,
    Type 1 builder, Type 2 parser (incl. target-info timestamp
    AV pair), Type 3 builder with fresh client challenge +
    FILETIME timestamp. Raw-TCP `ntlm_request` / `ntlm_head`:
    the Type 1 request doubles as the real request; on a 407
    with a Type 2, Type 3 + the same request go on the same
    socket (reqwest cannot do NTLM — it is connection-bound).
    `probe_connect_auth`: one unauthenticated CONNECT to learn
    what a proxy demands, used only to explain a failed https
    CONNECT with a clear error.
  - Wiring (`app/engine/src/`): `probe.rs` — `probe_with_proxy`
    dispatches ftp:// to the FTP probe and adds the 407 NTLM
    fallback for HTTP(S) probes; `manager.rs` — `probe_url`,
    `test_proxy`, `run_segment`/`download_segment_once` (new
    `download_segment_ftp` / `download_segment_ntlm`, reusing the
    limiter / pause / retry machinery), `fetch_page_links` and
    HLS variant probes now use the proxy-aware probe, sync
    re-probe uses `probe_url`; `proxy.rs` — `host_bypasses_proxy`
    helper; `store.rs` — `enqueue_pending_link` accepts `ftp`.
    UI: SettingsDialog FTP hint now says FTP proxies are used
    for ftp:// downloads; `src-tauri` `test_proxy` doc updated.
- Behaviour: ftp:// probes and downloads through the configured
  FTP proxy OR directly when none is set; FTP proxy auth and
  the bypass list apply. NTLM is auto-negotiated from a 407
  with `Proxy-Authenticate: NTLM` (no new setting — the proxy
  schema has no auth-type field); `DOMAIN\user`, `user@domain`
  and bare user all accepted; workstation sent as `FETCHLINE`.
  A Kerberos-only proxy (`Negotiate` without NTLM) is refused
  with a clear error, never faked. https:// through an NTLM
  proxy is refused with a clear error (TLS tunnel after the
  handshake is not built). FTP through an HTTP proxy is OUT OF
  SCOPE: an ftp:// download goes direct and the HTTP proxy is
  never contacted.
- Tests: `app/engine/tests/proxy_tests.rs` (new, 15 tests) —
  local FTP fixture server + local NTLM stub proxy, all local,
  no public FTP server. FTP: probe + direct download,
  proxy routing + auth order, bypass list, multi-segment
  byte-exact, pause/resume byte-exact, no-SIZE open-ended,
  login accept/refuse, wrong proxy credentials rejected, FTP
  direct when only an HTTP proxy is set, `test_proxy` ftp URL
  through an FTP proxy + the http/ftp refusal cases. NTLM:
  http download through the stub, wrong password rejected,
  Test button right/wrong password, Kerberos-only refused
  clearly, https test URL refused clearly. Independent check:
  libcurl `--proxy-ntlm` against the same stub — correct
  password: HTTP 200, 512 KiB byte-exact; wrong password:
  HTTP 407. (First curl runs looked like 200 for the wrong
  password because the VM's `no_proxy` includes 127.0.0.1 and
  curl bypassed the proxy entirely; `--noproxy ''` forces it
  through — the earlier 200 was a no_proxy artifact, not a
  stub bug.) NTLMv2 test vectors pinned against an independent
  Python/OpenSSL computation (`/tmp/ntlm_vector.py`, since
  removed with the /tmp scratch).
- Verification: full engine suite **326/326 pass, 0 failed**
  (35 unit + 111 integration + 32 extension + 16 final-verify
  + 28 grabber + 14 verifier + 14 options + 7 progress +
  15 proxy + 23 sync + 31 video); `npm run build` passes.
  BUG-009/010 regression tests pass in the full suite. One bug
  found and fixed in this round: BUG-022 (test-fixture-only —
  the NTLM stub closed the connection on a Basic auth header
  instead of re-challenging; see bugs.md). No engine bug was
  found. Two tests were converted (not deleted) for the
  deliberate behaviour change: `add_download_rejects_non_http_scheme`
  now probes `gopher://` (ftp:// is supported), and the comment
  on `ftp_proxy_is_stored_but_not_applied` now states the FTP
  proxy only routes FTP traffic.
- Harder than expected: (1) NTLM is connection-bound, so the
  whole proxy-auth path had to bypass reqwest with raw TCP —
  including re-sending the request on the same socket after
  Type 3, draining the 407 body first, and handling
  Content-Length / chunked / read-to-close bodies by hand.
  (2) A proxy 407 to a CONNECT tunnel never reaches reqwest as
  a response — it surfaces as a transport error — so https
  failures needed the `probe_connect_auth` preflight just to
  produce a clear error message. (3) The FTP segment model:
  each segment needs its own control + data connections, and
  the 226 check must be skipped when a segment stops early at
  a split end (the server is still sending). (4) Verifying
  while three other workers share the tree and /tmp: the
  baseline full run showed 6 engine_tests failures from other
  workers' in-flight edits; /tmp was NOT cleaned while their
  cargo tests were running (only 44 MB / 512 MB used, no
  pressure), so the before/after `rm -rf /tmp/fetchline-test-*`
  was skipped deliberately and is recorded here rather than
  claimed.
- Missing / limits: Kerberos (SPNEGO/GSSAPI) — not supported,
  needs system libraries not present here; see the decision in
  features.csv row 18. FTP through an HTTP proxy — out of
  scope, goes direct. https:// via NTLM proxy — clear error,
  not built. NTLM signing/sealing — not implemented (auth
  only). No FTPS, passive mode only. The FTP fixture proves
  the proxy login sequence (`user@host`), not a full relay
  proxy. Tauri commands uncompiled on this Linux VM (gdk-3.0);
  no Windows .exe; extension never run in a real browser.
  Test-only dummy credentials were used throughout (`alice` /
  `hunter2` etc.); no password is stored or logged in plain
  text.

## Not built (next milestones, in architecture order)
- S11 licence, MB-per-hours quota, per-download speed cap,
  Windows autostart registration.
- Correction (2026-11-15): this list previously named "S06 site
  grabber" and "periodic synchronisation (deferred to L)" as not
  built. Both are built now — S06 (partial, 2026-11-14) and S05
  periodic sync (partial, 2026-11-15, see above) — so they are
  removed from this list. `parity.md` still scores the pre-sync
  state (58.3/100) until the next replica-diff pass re-scores it.
- Desktop binary: no Windows .exe was produced on this Linux VM.
  `cargo check` in `src-tauri` fails here on the missing `gdk-3.0`
  system library, which apt cannot install on this image (pinned
  `libzstd1` `+hatch1` variant conflicts with `libzstd-dev`).
  First build must happen on Windows — see `app/README.md`.

## Final verification (all four partials) — 2026-11-15

Independent verifier pass (did not build any of the four
partials). Full engine suite re-run twice after the fix below:
**330/330, 0 failed** (35 unit + 111 integration + 32
extension + 4 final-verify2 + 16 final-verify + 28 grabber +
14 verifier + 14 options + 7 progress + 15 proxy + 23 sync +
31 video; was 326 — +4 new final-verify2 tests). `npm run
build` passes; extension JS 44/44 (36 extension tests + 8
speed-graph); `node --check` passes on all four extension
scripts; `cargo check --tests` 0 warnings (3 test-only warnings
in `proxy_tests.rs` found and fixed in this pass — unused
`Read` import, unnecessary `mut`, an `authed` flag the harness
never reads; the flag stays with an allow + comment because a
real proxy keeps connections alive). Host smoke against a
freshly rebuilt `fetchline-host` binary: video 13/13,
download-all 14/14, cookie 14/14. BUG-001–024 re-checked, all
regression tests pass by name (37 named checks); BUG-007
re-checked by source; BUG-013/016 via host smoke / JS tests.

- Bug found and fixed failing-test-first: **BUG-025** — the
  page-scan probes (`fetch_page_links`, `detect_page_videos`,
  HLS variant probes, `fetch_playlist_text`) never sent a saved
  site login, so a login-gated link showed 401 in Download All
  / the video panel even with a saved credential (the manual
  add path already sent it; the sync re-probe already used the
  credential-aware `probe_url`, verified by reading
  `manager.rs`). Fixed by threading `site_auth_for_url`
  (exact-host only) through all four call sites in
  `manager.rs`; 4 kept tests in
  `app/engine/tests/final_verify2_tests.rs` (4/4 pass). The
  site grabber crawl still does not send site logins —
  login-required pages are reported, as documented; separate
  scope decision, not part of the fix.
- Cross-worker damage check: all four workers' code present and
  intact — `CookiePair` / `probe_with_cookies` / cookies map in
  `manager.rs`, `ftp_probe` / `ftp_retr` in `ftp.rs`, NTLM Type
  1/2/3 in `ntlm.rs`, site-login store fns in `store.rs` /
  `options.rs`, Last-Modified handling in the sync path, all 9
  Settings tabs in `SettingsDialog.tsx` (incl. Site logins,
  Dial-Up, Sounds). No worker's files were clobbered by another.
- Manifest check: the `cookies` permission is genuinely used
  (`background.js` `cookiesFor(url)` reads cookies only for the
  exact handed-over URL); the manifest test
  `extension_manifest_is_mv3_and_minimal` passes with it listed
  as allowed, as does the Python validator. One stale comment
  fixed (it cited BUG-022 for the Firefox gecko id; the gecko
  id was BUG-023).
- Licence (S11) verified untouched: no licence/registration
  code exists in the app or the Tauri shell; the row stays `no`.

## Fresh replica-diff — 2026-11-15

`parity.py` re-run on the verified `features.csv`. Score
**76.4 / 100, must-haves 16 of 17** (was 66.7 / 13 of 17 at
the Round-12 baseline; 78.5 / 17 of 17 with the proxy worker's
`yes` still in place). The delta from the proxy worker's
number is the verifier revert: the proxy row was flipped back
`yes` → `partial` because Kerberos proxy auth is unbuilt
functionality, not unproven functionality, and recon F09 names
Kerberos as an in-scope proxy-auth edge — the row's own scope
does not allow a `yes`. `parity.md` rewritten from scratch
(see there for the per-screen structural comparison, the
F01–F10 behaviour diff updated for the four finishes, and the
new top five: Kerberos proxy auth, S11 licence, antivirus
hook, update check, download-limits quota + per-download cap).
Verdict: **NOT SHIPPABLE** — the one partial must-have is
proxy / Kerberos; zero open bugs. No Windows `.exe`; Tauri
shell still uncompiled on Linux (`gdk-3.0`); extension never
run in a real browser; no real website ever crawled/synced.

## Kerberos proxy auth (SPNEGO / Negotiate) — 2026-11-15

The last partial must-have is now built. The verifier's
revert reason (Kerberos unbuilt, recon F09 names it in
scope) no longer applies: `features.csv` row 18 flips
`partial` → `yes`. No `parity.py` re-run in this pass (the
coordinator does the final parity run).

**GSSAPI availability (checked first, as instructed).**
`libgssapi_krb5.so.2` and `libkrb5.so.3` are installed (MIT
Kerberos 1.20.1-6ubuntu2.10), but there are no headers, no
pkg-config entry, no `krb5-config`, no `kinit`/`klist`, no
`/etc/krb5.conf`, and no credential cache. No apt attempt
was made (the pinned `libzstd1` +hatch1 conflict is not to
be touched) — and none was needed.

**Dependency choice (justification).** No new crate. The
engine calls the already-installed MIT Kerberos runtime
directly through FFI (`#[link(name =
":libgssapi_krb5.so.2")]` — the versioned file links with
no `-dev` package). The `libgssapi`/`gssapi` crates were
rejected: they need pkg-config + headers, which are not
installed. The same direct-FFI pattern is used against
`secur32` on Windows (mirroring `secret.rs` DPAPI FFI), so
the Windows path is SSPI: `AcquireCredentialsHandleA`
(`"Negotiate"`) + `InitializeSecurityContextA`. Nothing is
faked and no Kerberos crypto is hand-rolled: the token
exchange is the system stack's, and the engine only encodes
/decodes the SPNEGO framing (DER) around it. The FFI was
proven with scratch probes before the implementation: a
`gss_import_name("HTTP/127.0.0.1")` succeeds and a
`gss_init_sec_context` with SPNEGO fails fast (major
`0x70000`, "No credentials were supplied… SPNEGO cannot
find mechanisms to negotiate") — no hang, no network.

**Auth selection + fallback order (single source of
truth: `proxy_auth_order` in `proxy.rs`, unit-tested).**
`proxy_configs.auth` is `basic | ntlm | kerberos |
negotiate-auto`, default `negotiate-auto` (new column +
migration; old databases without the column migrate to the
default). `negotiate-auto` tries **Kerberos → NTLM →
Basic**, restricted to what the proxy offered and what the
setting allows; `basic`/`ntlm`/`kerberos` try exactly one
scheme and fail with a clear error naming the selected
scheme when the proxy does not offer it. A 407 offering
only `Negotiate` never falls through to Basic. Basic
credentials are sent preemptively only when `auth` is
`basic` (or always for SOCKS5, where username/password is
the only auth); otherwise the engine waits for the 407 and
picks the strongest offered scheme, like a browser.
Kerberos uses the system ticket cache / Windows sign-in,
never the saved proxy password; the saved credentials are
used only for NTLM/Basic fallbacks. Kerberos is plain
`http://` only (https through a Kerberos proxy is refused
with a clear error — no TLS tunnel after the handshake,
same limit as NTLM) and is rejected for SOCKS/FTP proxy
types at validation.

**SPNEGO mechanics.** GSSAPI output is already a complete
`NegTokenInit`, so the engine does not wrap it; the engine
parses the server's `NegTokenResp` to extract
`responseToken` for the next step. Multi-round loop, max 5
rounds, 30 s timeout; GSSAPI `step()` runs via
`tokio::task::spawn_blocking` (it can block on KDC/DNS).
SPN is `HTTP/<proxy host>`. The exchange runs on the probe
path, the download-segment path, the Test button, and the
cookie probe (Basic only for cookies — the raw Kerberos
handshake cannot carry them, so a cookie-gated link behind
a Negotiate-only proxy fails with a clear Kerberos error).
Test seam: `SpnegoInitiator` trait; production callers
always construct the real GSSAPI/SSPI initiator via
`new_kerberos_initiator`; tests pass a scripted initiator
through `kerberos_request_with_initiator`.

**What the tests prove, and what they do not.** The stub
proxy (`tests/kerberos_tests.rs`) validates SPNEGO framing
with its OWN hand-written DER code — it does not share the
engine's encoder/decoder — and the engine's encoder is
pinned independently by unit tests against hand-computed
vectors. The end-to-end success path uses a scripted token
source: it proves framing, the multi-round exchange, and
forwarding — NOT a real domain login. No KDC was ever
contacted. The failure path (no credentials on this VM)
goes through the REAL GSSAPI initiator and the REAL
manager/probe paths: it proves the engine attempts
Kerberos, fails with a clear error naming Kerberos, and
falls back (or refuses) in the documented order.

**Files.** New `app/engine/src/kerberos.rs` (~42 KB:
SPNEGO DER, GSSAPI FFI, SSPI, unit tests) and
`app/engine/tests/kerberos_tests.rs` (14 tests: scripted
handshake success, ranged GET + HEAD, initial-407-token
plumbing, failing-initiator error, real-GSSAPI Negotiate-only
failure via the manager, Kerberos→Basic and Kerberos→NTLM
fallbacks via the manager, explicit-setting no-downgrade
cases, https refusal, auth round-trip/validation/migration,
cookie-gated download behind Basic and behind
Negotiate-only proxies). Edited: `proxy.rs` (auth
selection, fallback order, validation), `ntlm.rs` (shared
body-reader helpers made `pub(crate)`), `lib.rs`
(exports), `store.rs` (migration + `auth` handling),
`replica/schema.sql` (CHECK), `probe.rs` (407 fallback
walker + cookie-probe threading), `manager.rs` (segment
407 walker + `download_segment_kerberos` + Test button
walker), `tests/engine_tests.rs` + `tests/proxy_tests.rs`
(`auth: None` in the helper; one kept test now uses a
demanding proxy because `negotiate-auto` no longer sends
Basic preemptively — same assertions, via the 407 path —
plus a new test pinning preemptive Basic for `auth:
basic`). UI: `ProxyConfigRow.auth` /
`ProxyConfigInput.auth` in `types.ts`, an Authentication
select + hint in the Proxy tab of `SettingsDialog.tsx`
(options filtered to Basic for SOCKS/FTP, type change
forces Basic), `test_proxy` doc comment in
`src-tauri/src/lib.rs`. No new crates, no new Tauri
commands.

**Verification.** Full engine suite **354/354 pass, 0
failed** (44 unit + 112 integration + 32 extension + 4
final-verify2 + 16 final-verify + 28 grabber + 14 verifier
+ 14 options + 14 kerberos + 7 progress + 15 proxy + 23
sync + 31 video; was 330 — +9 unit, +1 integration, +14
kerberos). `npm run build` passes. `cargo check --tests`
0 warnings. `/tmp/fetchline-test-*` cleaned before and
after. BUG-009/010 regression tests pass. **No new bugs
found in this pass (no BUG-026).** One kept-test assertion
bug was found and fixed during the pass (a wrong
`Content-Range` substring in the new ranged-GET test) —
test-only, not engine behaviour, not logged as a bug.

**Missing / limits.** No real KDC contacted (stub only);
SSPI path written, never compiled (Tauri shell still does
not build on Linux — `gdk-3.0` — so still no Windows
`.exe` and new Tauri commands uncompiled); Kerberos
success through the manager's probe+segment paths is
unproven without a KDC (only via the scripted-initiator
seam + the fallback tests); https through a Kerberos proxy
is refused; SOCKS5 GSSAPI is not built; no real website
ever used. Licence (S11), antivirus hook (S14), update
check and download quota untouched, as instructed.

## Update check ("Quick update check", S09 / should-have): done (2026-11-15)

- Done (engine): new `app/engine/src/update.rs` (~19 KB, 6 unit
  tests). `check_for_updates()` fetches a JSON version manifest
  (`{version, changelog|changes|notes, download_url?}`) from
  Fetchline's own update server and reports whether a newer
  version exists. Real semver-ish compare, not string compare:
  `1.10.0 > 1.9.9`, `1.2.3 > 1.2.2`, `1.2 == 1.2.0`, a leading
  `v` tolerated, a pre-release is older than its release
  (`1.0.0-beta < 1.0.0`, `beta.2 < beta.10`), build metadata
  ignored. A malformed manifest (invalid JSON, missing/empty/
  unparseable version, non-http(s) download address, > 1 MiB
  body) is a clear error naming the problem, never a crash and
  never a phantom update. A non-2xx answer names its status;
  a network failure names the failure ("Could not reach the
  update server …") — never a silent "up to date".
- Done (endpoint): the URL is a setting, `settings.update_url`
  (new columns `update_url` + `last_update_check_at` on the
  `settings` table — `replica/schema.sql` + `Store::migrate`
  ALTERs for older databases, verified by a kept migration test
  on a hand-built pre-update database). It defaults to the
  documented placeholder
  `https://updates.fetchline.app/releases/latest.json`, which
  does not exist yet and must be replaced with the real release
  server before shipping. It is Fetchline's own address — never
  IDM's servers, never any third-party feed. Empty / non-http(s)
  addresses are rejected on save with a clear error, nothing
  stored. `update_settings` applies live (a manual check after a
  save uses the new URL with no restart). The fetch uses the
  engine's HTTP client, so the configured proxy and timeout
  apply.
- Done (weekly): the check rides on the EXISTING 30 s scheduler
  tick (`scheduler_tick_at` spawns it; no second scheduler). A
  check is due when `check_updates_weekly` is on (default) and
  the last recorded check is at least a week old, or never
  happened. The fetch runs in a detached task behind an
  in-flight flag, so a slow server cannot stall queue
  scheduling and a later tick never starts a second check. The
  last-check time is recorded after every completed attempt —
  success or failure — so a failed check retries next week, not
  on every tick. When a weekly check finds an update it is sent
  on a new `update_events` broadcast channel; up-to-date and
  failed weekly checks send nothing. Manual Help > Check for
  updates always runs, even when the weekly setting is off.
- Done (ask before updating): the check NEVER installs anything
  and never starts a download — the result carries the version,
  the changelog and the installer address, and the user decides.
  Verified by test: after a newer-version check the download
  list is still empty and exactly one request (the manifest GET)
  was made.
- Done (UI): Help menu in the header (Check for updates +
  Settings), `UpdateDialog.tsx` (current/latest version,
  changelog list, installer address in a readonly field with a
  Copy button, "Remind me later" when an update is available,
  "Close" when up to date, clear error state with "will retry
  next week"), Settings > General gains the update-server
  address field with the last-checked time, and the weekly
  `update-available` event opens the dialog. `api.ts` gains
  `checkForUpdates()` / `onUpdateAvailable()`; `types.ts`
  gains `UpdateCheckResult` and the two new Settings fields.
  The Tauri shell gains the `check_for_updates` command and
  forwards `update-available` events, following the existing
  queue-action pattern.
- Install wiring — what is wired vs what is not: the Tauri
  updater plugin is NOT configured in this build
  (`tauri.conf.json` has no `plugins.updater` section and
  `src-tauri/Cargo.toml` does not depend on
  `tauri-plugin-updater`), and there is no signed release
  pipeline, so there is no in-app install. The result carries
  `updater_configured: false` and the dialog says so plainly
  and offers the installer address to copy instead of an
  Install button that would pretend to work. Wiring the real
  install means: add the updater plugin, publish signed
  releases, and point `update_url` at the server that serves
  them — the check itself needs no change. This is recorded
  in `update.rs`, the dialog, `features.csv` and `test-plan.md`.
- Verified: full engine suite **369/369 pass, 0 failed, 2
  consecutive runs** (50 unit + 112 integration + 32 extension
  + 4 final-verify2 + 16 final-verify + 28 grabber + 14
  verifier + 14 options + 14 kerberos + 9 update + 7 progress
  + 15 proxy + 23 sync + 31 video; was 354 — +6 unit, +9
  update). `npm run build` passes (tsc + vite). `cargo check
  --tests` 0 warnings. `/tmp/fetchline-test-*` cleaned before
  and after. No new bugs found (no BUG-026). One test-only fix
  during the pass: a manifest with no `version` field failed
  with a generic JSON error instead of the explicit "no
  version" error — `version` now has `#[serde(default)]` so
  the explicit error fires; test-only behaviour, not a product
  bug, so no BUG entry.
- Missing / limits (environment-only, not check function):
  the new Tauri command is uncompiled (gdk-3.0, same as every
  milestone); the dialog and Help menu have never been clicked
  in the running desktop app; no Windows `.exe`; no real
  update server was ever contacted (local fixture server only).
  Kerberos proxy code untouched; download quota, licence (S11)
  and antivirus hook (S14) untouched, as instructed.

## S12 — Download quota + per-download cap: done (2026-11-15)

- Done (quota): X MB every Y hours, stored in settings
  (`quota_enabled`, `quota_mb`, `quota_hours`; 1–1,000,000 MB,
  1–720 h — rejected, not clamped, the BUG-008 rule; enabling
  with no size/window is rejected). The live window lives in a
  new `quota_state` table (`window_start_unix`, `bytes_used`,
  singleton id = 1) — NOT in settings, so saving settings never
  resets the window, and a restart does not reset it either
  (proven by a test that drops the whole manager and reopens the
  same database file).
- Window semantics (chosen, documented in `engine/src/quota.rs`,
  `features.csv` and the test plan): the window is **anchored at
  first use** — it starts when the first byte is counted after
  the quota is enabled, after a rollover, or after a manual
  reset — not at a fixed epoch boundary, so the user gets the
  full Y hours whenever they start downloading. **What counts:
  actual downloaded bytes only** — every byte a segment writes
  to a `.part` file, recorded per chunk as it is written, from
  every path that downloads (manual, queue, drain, grabber, sync
  re-download — all go through the same run loop). Probe/HEAD
  bytes never count; a byte re-downloaded after a resume counts
  again, because it was downloaded again.
- When reached: the running download is paused through the
  existing pause path (segments stop at the next chunk boundary)
  and marked `quota_paused` (new `downloads` column), the queue
  stops with the rest still queued, and no new download starts —
  `start_download` refuses with a clear error naming the resume
  time. Only quota-paused downloads are auto-resumed: a download
  the user paused by hand is never resumed by a rollover (pinned
  by a test).
- Rollover is automatic: the 30 s scheduler tick checks the
  window (no second scheduler) and a start re-checks it first,
  so a rollover is picked up within a tick or at once on the next
  start. The manual action is **Reset quota & resume now**
  (`reset_quota`): a fresh window at once plus resuming the
  quota-paused downloads. There is no separate "resume now" that
  keeps the old window — resuming into an exhausted window would
  pause the download again on its next chunk.
- Clock seam (tests): every quota method has an `_at(now)`
  variant taking Unix seconds, mirroring `scheduler_tick_at` —
  tests drive a 1-hour window with `now + 3601` instead of
  waiting an hour.
- Done (per-download cap): a KB/s cap on one download (new
  `downloads.speed_limit_kbps`, NULL/0 = no cap, default),
  enforced by a second token bucket per running download
  alongside the existing global bucket — every chunk waits for
  tokens from both, so the effective rate is the min of the two.
  Live-changeable on a running download (the stored cap is
  re-rated in place, like the global cap). Validation reuses the
  global bound (10,000,000 KB/s, BUG-008 rule).
- Done (sync interplay): sync re-downloads go through the normal
  run path, so their bytes count toward the quota (proven: a
  513 KiB re-download leaves exactly 513 KiB used — the HEAD
  probe counted nothing). A sync re-download paused by the quota
  is reported `failed` naming the quota, fast: `wait_for_download`
  now treats `paused` as terminal for the waiter (before, it
  only returned on complete/error, so a paused re-download would
  have sat out the 120 s timeout). The grabber waiter already
  treated paused as a failure, so it needed no change.
- Done (UI): quota section in Options > Connection (enable,
  MB, hours, live usage + window end + reached state + Reset
  button), per-download cap field in the S02 add dialog (stored
  before the download starts), cap + quota-paused note in the
  S03 progress dialog, quota state in the status bar (usage, or
  "Quota reached — resumes HH:MM"). Types + API wrappers in
  `src/lib/types.ts` / `src/lib/api.ts`; 4 new Tauri commands
  (`set_download_limit`, `get_quota_status`, `reset_quota`,
  `set_download_speed_limit`, the architecture.md name for the
  first) in `src-tauri/src/lib.rs`.
- Schema: settings gains `quota_enabled` / `quota_mb` /
  `quota_hours`; downloads gains `speed_limit_kbps` /
  `quota_paused`; new `quota_state` table (all in
  `replica/schema.sql` plus `Store::migrate` ALTERs/CREATE for
  older databases, with a kept migration test on a hand-built
  pre-S12 database).
- Verified: full engine suite **388/388 pass, 0 failed**
  (52 unit + 112 integration + 32 extension + 4 final-verify2
  + 16 final-verify + 28 grabber + 14 verifier + 14 options
  + 14 kerberos + 9 update + 7 progress + 15 proxy + 19 quota
  + 23 sync + 31 video; was 369 — +2 unit in `quota.rs`, +19
  quota). `npm run build` passes (tsc + vite).
  `cargo check --tests` 0 warnings. `/tmp/fetchline-test-*`
  cleaned before and after. No new bugs found (no BUG-026).
  Three test-only fixes during the pass, not product bugs: the
  migration test's hand-built database needed the
  `grabber_project_id` column a real pre-S12 database has (the
  schema's index on it fails without it); the pause/resume tests
  use a 1.5 MiB file, because a 2 MiB file's ~1 MB remainder does
  not fit in the fresh 1 MB window after a rollover and pauses
  again (correct engine behaviour, wrong test sizing); the
  user-pause test's rollover expectation was wrong — an expired
  window with usage does roll (usage resets) even with nothing
  to resume, the point is the user-paused download stays paused.
- Harder than expected: the quota-pause race. With zero-delay
  64 KiB chunks the four segments could each read their whole
  range before any pause flag landed (TCP coalescing), so the
  file completed instead of pausing. The pause tests use 8 KiB
  chunks with a 5 ms server delay, so the pause lands on chunk
  boundaries with at most one in-flight chunk per segment of
  overshoot (pinned by the test: paused between 1 MB and
  1.5 MB, never before the quota, never the whole file).
- Missing / limits (environment-only, not function): the 4 new
  Tauri commands are uncompiled (gdk-3.0, same as every
  milestone); the quota UI has never been clicked in the running
  desktop app; no Windows `.exe`. A quota enabled mid-download
  applies to the next download, not the running one (like
  `max_connections` — the running download's bytes were never
  counted, so counting them from mid-run would under-count the
  window); the window follows wall-clock Unix time, so a user
  changing the system clock moves the window, as with the
  scheduler. Kerberos proxy code and update-check code
  untouched; licence (S11) and antivirus hook (S14) untouched,
  as instructed.

## S14 — Antivirus hook: done (2026-11-15)

- Done (engine): new `engine/src/antivirus.rs`. After a download
  completes and the `.part` is renamed to its final name, if
  `settings.antivirus_command` is set and `antivirus_enabled` is
  on, the scanner runs on a detached task — completion, the queue
  drain and the UI never wait for it. The command template
  substitutes `{file}` with the finished path (appended as the
  last argument when the template has none); it is split by a
  quote-aware tokenizer and executed directly, never through a
  shell, so a filename cannot inject a second command. Results
  are stored per download (`scan_status` / `scan_detail` /
  `scan_scanned_at` on `downloads`, new columns in
  `replica/schema.sql` + `Store::migrate`): `clean` (exit 0, no
  threat in the output), `infected` (non-zero exit, or exit 0
  with a threat named in the output — the detail is the scanner's
  own first lines, never an invented threat name), `scan_failed`
  (scanner could not start, bad template, file gone),
  `not_configured` (no command, or scanning off — the download
  still completes normally), `timed_out` (configurable 5–600 s,
  default 60 s; the scanner is killed, the app never hangs),
  `scanning` (transient, stored while the detached scan runs).
  Completion progress events carry the result (`not_configured` /
  `scanning` at once; the finished result is re-emitted as a
  completion event when the scan lands), so the dialog updates
  without polling. Nothing is ever auto-deleted or
  auto-quarantined: a threat only disables Open (the engine's
  `open_file` refuses too) and the dialog offers Delete (the
  existing delete path) and Quarantine (new: moves the file into
  a `Quarantine` folder next to it and re-points the row) as
  explicit user choices.
- Done (settings): `settings.antivirus_enabled` (default on) and
  `settings.antivirus_timeout_secs` (5–600, default 60, rejected
  not clamped — the BUG-008 rule) alongside the existing
  `antivirus_command`; `Store::get/update_settings` round-trips
  them, and a pre-S14 database migrates (kept migration test on
  a hand-built old database). The Settings > Downloads tab gains
  the virus-check section: enable checkbox, command field,
  timeout field, Test button (runs the typed command against a
  small throwaway file and reports clean / threat / failed /
  timed out) and a note that Fetchline includes no scanner (the
  user needs one installed, e.g. Windows Defender).
- Done (dialog): the S03 progress dialog gains the S14
  completion section — finished file path, the scan result in
  words for every state, Open (disabled while a threat is found
  or a scan is running), Open folder, Scan again, and Quarantine
  / Delete file buttons only when a threat was found.
- Done (Tauri): 5 new commands following the existing pattern —
  `run_antivirus_scan`, `get_scan_result`,
  `test_antivirus_scanner`, `quarantine_download`, `open_file` —
  wired into `src/lib/api.ts` and the dialog/Settings.
- Verified: `cargo test` in `app/engine` — **410 passed,
  0 failed** (55 unit + 112 integration + 32 extension + 4
  final-verify2 + 16 final-verify + 28 grabber + 14 verifier +
  14 kerberos + 14 options + 7 progress + 15 proxy + 19 quota + 23 sync
  + 9 update + 31 video + 17 new antivirus — 17 in
  `tests/antivirus_tests.rs` plus 3 more unit tests in
  `antivirus.rs` (counted in the 55 unit total); was 390, +20). `cargo check --tests`
  warning-free (0 warnings). `npm run build` passes (tsc +
  vite). One bug found by the full-suite run and fixed
  failing-test-first: BUG-026 (`list_queue_files` missed the new
  scan columns — every queue read failed with
  `Db(InvalidColumnIndex(20))`; regression test kept).
- Harder than expected: (1) the timeout kill. The first runner
  used `tokio::process::Child::wait_with_output`, which consumes
  the child — on timeout there is no child left to kill. The
  rewrite runs the scanner on a blocking thread with two
  pipe-drain threads and a `try_wait` poll; the drain threads
  are deliberately NOT joined on the timeout path, because a
  scanner child (e.g. a `sleep` the scanner spawned) can hold
  the pipes open — joining them made the "timeout" take the
  full 30 s. (2) The queue breakage above: `row_to_download`
  has three SELECT callers, and the third (`list_queue_files`)
  was missed until the full suite ran — the antivirus-only run
  was green, which is why the full suite is the gate, not the
  new file alone.
- Missing / limits (environment-only, not function): the 5 new
  Tauri commands are uncompiled (gdk-3.0, same as every
  milestone); the Settings section and the completion dialog
  have never been clicked in the running desktop app; no
  Windows `.exe`. No real antivirus product was ever run — every
  scan in the tests used a fake scanner script fixture (exit
  codes and output simulated); no malware sample, not even an
  EICAR test file, was fetched, created or used. Windows
  Defender / real AV behaviour on Windows (exit codes, output
  text, timing) remains unproven. Licence (S11) untouched, as
  instructed.

## BUG-027 — First Windows build failure (Unix-only `write_at`): fixed (2026-11-15)

- The first GitHub Actions Windows build (run 43674541575) failed
  at step 9 (`tauri build`): the frontend built fine, but
  `fetchline-engine` failed with 5 errors — `use
  std::os::unix::fs::FileExt` at manager.rs:25 plus four
  `write_at` call sites. The engine-tests step (step 8) exited 1
  too, almost certainly the same compile failure (cargo test
  compiles the same lib).
- Fix: cfg-gated `write_at_pos` helper in
  `app/engine/src/manager.rs` (Unix `write_at`, Windows
  `seek_write`, same semantics); top-level Unix-only import
  removed; all 4 call sites converted; Linux behaviour unchanged.
  Whole-workspace sweep found no other Unix-only compile blocker
  (all other platform branches are cfg-gated; `host` checks
  `%APPDATA%` first). Kerberos SSPI warning at kerberos.rs:696
  fixed by restructuring (input buffer built only when a token
  exists), no `#[allow]`.
- Regression test kept:
  `manager::tests::write_at_pos_writes_out_of_order_byte_exact`
  (out-of-order + overlapping writes read back byte-exact).
- Verified: full suite 411/411 pass (was 410/410, +1), `cargo
  check --tests` 0 warnings, `npm run build` passes, parity
  unchanged 85.4/100. Windows cross-check attempted but blocked
  on this VM (MinGW gcc missing, apt cannot locate the package);
  every `cfg(windows)` block was audited by hand instead, and
  the CI run after upload is the real proof.
- Delivered by pushing the changed files to GitHub directly
  (the connector write access was fixed by the user's full
  disconnect + reconnect) — the push re-triggers the workflow.
  No workflow change was needed (the failure was product code,
  not the workflow). Licence (S11) untouched.

## BUG-028 — Second Windows build failure (CI run 43676557208) — 2026-11-15
- Step 8 "Engine tests" went green on Windows (BUG-027 proven
  fixed), but step 9 failed again at a later stage: a single Rust
  error, `error[E0425]: cannot find type 'Result' in module
  'std::path'` at `app/src-tauri/src/lib.rs:28` — `db_path`
  declared `std::path::Result`, which does not exist.
  `app_data_dir()` returns `Result<PathBuf, tauri::Error>`.
  The typo survived because the Tauri lib never compiles on the
  Linux VM (missing `gdk-3.0`); rustc only reported it once
  BUG-027 was out of the way.
- Fix: `db_path` now returns `Result<std::path::PathBuf,
  tauri::Error>`; behaviour unchanged. Whole Tauri shell
  (`lib.rs` 779 lines + `main.rs`) audited — no other wrong type,
  missing import or bad path (the run-4 log showed this single
  error). Also cleaned the two remaining Windows-only warnings:
  kerberos.rs `in_buf` dead assignment (now built in one
  `input_token.map(...)` assignment; verified with a `rustc -D
  warnings` stub — old pattern reproduces the warning, new one
  is clean) and the unused `FreeContextBuffer` FFI declaration
  (removed — SSPI never allocates a buffer for us).
- Verified: full suite 411/411 pass, `cargo check --tests` 0
  warnings, `npm run build` passes, parity unchanged 85.4/100
  (must-haves 17/17). Windows cross-check still blocked on this
  VM (MinGW gcc missing; Tauri lib uncheckable here) — the CI run
  after the push is the proof. Pushed the changed files only via
  the connector; the push re-triggers the workflow. Licence (S11)
  untouched.

## BUG-029 — First real Windows launch fails silently: startup diagnostics added — 2026-11-15
- The run-#5 MSI installs fine, but on the user's real PC the app
  shows a blank white window for ~1-2 s and closes — no error
  dialog, no WER / Application Error event (Event Viewer shows
  only unrelated CAPI2 errors). `app.db` exists (156 KB, first
  launch), so setup got past the database. No `process::exit`,
  no frontend close, scheduler tick is a no-op on a fresh DB.
  Root cause still UNKNOWN after several rounds of guessing, so
  this round stops guessing and instruments the app (diagnostics
  only, no behaviour change).
- Fix: `app/src-tauri/src/lib.rs` now writes an append-only
  `startup.log` next to the database
  (`%APPDATA%\com.fetchline.app\fetchline\startup.log`) with
  milestones (main entered, webview version or ERROR, setup
  start, db_path ok, store opened, manager built, settings
  applied, spawns done, setup ok, run returned ok, plus a
  once-per-process "frontend reached backend: get_settings ok"
  from the existing `get_settings` command). A panic hook at the
  start of `run()` appends panic payload + location to the same
  file (path from `APPDATA`, temp-dir fallback; hook cannot panic,
  also prints to stderr). A run error is logged before the same
  panic as before. All std-only, best-effort logging.
- Verified: std-only helpers checked with a `rustc -D warnings`
  stub (`/tmp/bug029/` — exact extracted functions compile clean
  and behave as designed); Tauri-side signatures verified
  against vendored tauri 2.12.5 (`webview_version` re-export,
  `Builder::run -> Result<()>`, setup `Result<(), Box<dyn
  Error>>`). Full suite 411/411 pass, `cargo check --tests` 0
  warnings, `npm run build` passes, parity unchanged 85.4/100
  (must-haves 17/17). Tauri lib still uncheckable on this VM —
  the CI run after the push is the proof. Next step is the user
  installing this build, launching once, and sending the
  `startup.log`. Licence (S11) untouched.

## BUG-030 — Diagnostics rewrite broke the Windows build (two E0308 type errors) — 2026-11-15
- CI run #6 (43680534913, commit f51e6199) failed at step 9 with
  two E0308 errors in `app/src-tauri/src/lib.rs`, both from the
  BUG-029 diagnostics rewrite: (1) `DownloadManager::new(store,
  8)` passed a plain `Store` where `Arc<Store>` is required;
  (2) `return Err(e)` in the setup closure passed a `String`
  where `Box<dyn Error>` is required (the old `?` converted it
  automatically). No other compile error in the crate.
- Fix: two lines only — `Ok(DownloadManager::new(Arc::new(store),
  8))` and `return Err(e.into());` (same conversion the old `?`
  used), log line kept. Whole diagnostics diff re-audited for
  other Windows-only compile risks; none found.
- Verified: full suite 411/411 pass, `cargo check --tests` 0
  warnings, `npm run build` passes, parity unchanged 85.4/100
  (must-haves 17/17). Tauri lib still uncheckable on this VM —
  the CI run after the push is the proof. BUG-029 stays OPEN.
  Licence (S11) untouched.

## BUG-029 ROOT CAUSE FOUND + FIXED — `tokio::spawn` in setup panics with no reactor — 2026-11-15
- The user's `startup.log` from the run-#7 instrumented build (launched 07:47:21 EET) ends: `settings applied`, then `PANIC at src\lib.rs:793:13: there is no reactor running, must be called from the context of a Tokio 1.x runtime`. The `.setup` closure runs synchronously on the main thread outside any Tokio runtime, but called `tokio::spawn` six times — the first call panics, the process exits cleanly (code 101, no WER entry) before the event loop starts. WebView2 is FINE (157.0.4341.46 logged). Latent since the first build; not a Windows-only quirk.
- Fix: all six setup `tokio::spawn` calls in `app/src-tauri/src/lib.rs` (lines 793/802/813/824/834/839) are now `tauri::async_runtime::spawn` — Tauri's own runtime handle, the correct way to spawn from setup (signature verified against vendored tauri 2.12.5, `async_runtime.rs:300`). Spawned bodies byte-identical; `tokio::time::interval` inside the tick task unchanged. Audit: no other `tokio::spawn` outside a runtime context in `lib.rs` or `main.rs`. Startup.log diagnostics stay in place.
- Verified: full suite 411/411 pass, `cargo check --tests` 0 warnings, `npm run build` passes, parity unchanged 85.4/100 (must-haves 17/17). Tauri lib still uncheckable on this VM — the CI run after the push is the proof, and the user's next `startup.log` (should end "setup ok" + "frontend reached backend") is the launch proof. BUG-029 FIXED, BUG-030 stays FIXED. Licence (S11) untouched.

## Windows launch smoke test added to CI — 2026-11-15
- New `smoke-test` job in `.github/workflows/build-windows.yml` (runs on
  every push, `needs: build-windows`, `runs-on: windows-latest`,
  `timeout-minutes: 10`). It exists because BUG-029 (blank window, close
  after ~1 s) was only ever caught on the user's PC — a green build alone
  does not prove the app launches. The user asked why this wasn't done
  from the start; fair point.
- Steps: (1) download the `fetchline-windows-installer` artifact;
  (2) PowerShell: locate the MSI recursively under `installer/`, install
  silently (`msiexec /i <msi> /quiet /norestart /log install.log`, step
  fails on non-zero exit), launch `C:\Program Files\Fetchline\
  fetchline.exe` with `Start-Process -PassThru`, wait 30 s, check the
  process is still alive (`Get-Process` + `HasExited`, `MainWindowTitle`
  recorded), print the FULL `%APPDATA%\com.fetchline.app\fetchline\
  startup.log` to the job log and check it contains `setup ok`;
  screenshot the desktop with `CopyFromScreen` (best-effort, try/catch,
  never fails the step); always `Stop-Process` afterwards.
  (3) upload `fetchline-smoke-test` artifact (`startup.log`,
  `smoke-screenshot.png`, `install.log`, `if: always()`,
  `if-no-files-found: warn`).
- PASS criteria (exit 0): process alive after 30 s AND startup.log exists
  AND contains `setup ok`. `frontend reached backend: get_settings ok`
  is recorded but NOT a pass/fail signal — on a headless runner the
  webview may not mount even when the app is fine. FAIL (exit 1) if the
  process died, the log is missing, or `setup ok` is absent.
- Verified on Linux: YAML parses (`yaml.safe_load`), jobs/steps/`needs`/
  trigger re-read and correct; build job's 9 steps untouched; PowerShell
  reviewed line by line (quoting, `$LASTEXITCODE`, `$env:APPDATA` path,
  recursive MSI lookup). No PowerShell exists on this VM — the script is
  reviewed-only; the CI run is the proof. No product code changed, no
  engine tests touched. Licence (S11) untouched.

## Smoke-test v1 run — FAILED at the install step, app never launched (2026-11-15)
- Run 43684788098 (head 8d4d1f78, user's manual workflow upload): build job
  SUCCESS, smoke-test job FAILED in 10 s. Step 3 "Install MSI and launch
  Fetchline" died in 2 s at the msiexec line: `& msiexec.exe` returned with
  `$LASTEXITCODE` BLANK (not 0, not an error code), so the script's
  `if ($LASTEXITCODE -ne 0)` check threw "MSI install failed (exit code )".
  install.log was empty/missing (its tail printed nothing). The app was
  NEVER launched, the 30 s wait never happened, no startup.log was produced.
  So v1 proved NOTHING about the app or the BUG-029 fix — the harness broke
  before the test. Root cause (script bug, not app bug): msiexec is a
  GUI-subsystem app and `&` does not reliably wait for it or set
  `$LASTEXITCODE` in non-interactive pwsh.
- Fix (v2, in `.github/workflows/build-windows.yml`, ready for manual
  upload — the connector cannot push workflow files): install via
  `Start-Process -FilePath 'msiexec.exe' -ArgumentList ... -Wait -PassThru`
  and check `.ExitCode` (null or non-zero = fail, with the install.log tail
  printed). Everything else in the step is unchanged (the startup.log path
  was already correct: `$env:APPDATA\com.fetchline.app\fetchline\startup.log`).

## Real download test + browser capture test (CI, Windows runner) — 2026-11-15

The launch smoke test proved the app opens; these two close the next
gaps: that it actually DOWNLOADS on Windows, and that a browser link
reaches it on Windows. Both were built the same day the smoke test
went green (run 43686477003).

### TEST 1 — real download (`app/engine/tests/real_download_tests.rs`, 2 tests)
- Downloads Fetchline's own LICENSE from a PINNED commit (7255dff9)
  via raw.githubusercontent.com through the REAL DownloadManager
  (probe + add_download + start + segments) and verifies the bytes:
  1,079 bytes, SHA-256
  `c2ef3696eecd8d51c0e615afe9437e5d620abcd9272b9d9f4bcbbd1e4a95f793`.
  The pinned URL was verified byte-identical to the repo copy
  2026-11-15. New dev-dependency `sha2 = "0.10"` (test-only, does
  not ship in the app) + `Cargo.lock` update.
- CI: dedicated step "Real download test (fails the run)" with NO
  continue-on-error (step-8 `cargo test` unchanged, still
  continue-on-error). The step sets `FETCHLINE_REAL_DOWNLOAD_TEST=1`.
- Env gate (honest limit): without the env var both tests print
  SKIPPED and pass. The Linux dev VM sits behind a TLS-intercepting
  egress proxy (its `Hatch Sandbox Egress CA` is in the system
  store, so curl trusts it), but the engine's rustls client uses
  compiled-in webpki roots and rejects it
  (`InvalidCertificate(UnknownIssuer)`) — verified 2026-11-15: both
  tests fail ungated on this VM, skip cleanly gated. The Windows
  runner has direct internet and no intercepting proxy, so they run
  for real there. Never weaken TLS verification to make them pass
  locally. If the repo is renamed, update the pinned URL in the
  test (the hash stays the same — the LICENSE text does not change).

### TEST 2 — browser capture (`scripts/capture-test.py`, CI job `extension-capture-test`)
- New job (needs build-windows, windows-latest, 20 min timeout) in
  three phases, driven by `scripts/capture-test.py`:
  1. host: the real `fetchline-host.exe` (built on the runner from
     `app/host`), driven with real Native Messaging frames (ping,
     get_settings, download_link) against the app's real `app.db`;
     the pending_links row is read back from SQLite (source
     `browser`, status `pending`). Dry-run on this VM against the
     local host binary: PASSED (ping pong, get_settings
     browser_integration=true, download_link queued with
     pending_link_id, row confirmed).
  2. browser: the unpacked extension loads in headless Chrome, its
     ID is discovered (CDP `/json/list`, Preferences fallback), the
     host manifest + HKCU registry key are written exactly as a
     user install would, `popup.html` is opened and its status text
     polled over CDP (websocket-client) until it reports
     "Connected to the Fetchline desktop app".
  3. drain: the installed app drains the queue and downloads the
     pinned LICENSE; the script verifies its SHA-256 in
     `%USERPROFILE%\Downloads` (the correct landing spot: a
     no-extension file falls back to category `cat-documents`,
     whose default folder is empty, so the folder resolution falls
     through to the OS Downloads folder).
- PACKAGING GAP (finding, not a workaround to hide): the MSI ships
  NO `fetchline-host.exe` and creates NO native-messaging registry
  key — `tauri.conf.json` has no bundle config for either, and the
  Tauri app does not depend on the host crate (the repo host
  manifests point at `C:\Program Files\Fetchline\fetchline-host.exe`,
  a path that does not exist after installing the MSI). The job
  builds the host and registers it itself, runner-only. Until the
  installer ships the host + key, a user install cannot capture
  from a browser at all, however green this job is.
- Limits: a human right-click ("Download with Fetchline") or the
  download watcher cannot be driven in headless Chrome (no mouse,
  no real popup window). Those use the same `sendNativeMessage` +
  `handToFetchline` code Phase 2 exercises, and the menu/watcher
  wiring is covered by the Node fake-`chrome` tests. Phase 2 (CDP
  polling via websocket-client, extension-ID discovery) is the
  riskiest part — reviewed-only on this VM (no Chrome here); the
  CI run is the proof.
- Verified on Linux: full engine suite 413/413, 0 failed (411 + 2
  new, the new ones SKIPPED ungated); `cargo check --tests`
  0 warnings; `npm run build` passes; workflow YAML parses and
  re-reads correctly (jobs build-windows + smoke-test +
  extension-capture-test; real-download step has no
  continue-on-error, step-8 still does); `scripts/capture-test.py`
  compiles (`py_compile`) and Phase 1 dry-runs green locally.
  No product behaviour changed; no licence work; no IDM code,
  assets or wording.

## Installer packaging fix — browser capture for real users (S13) — 2026-11-15

Closes the packaging gap found by the capture-test round above: the MSI
shipped NO `fetchline-host.exe` and created NO native-messaging registry
key, so a user install could not capture a single link from a browser
(the CI capture job only worked because it built + registered the host
itself, runner-only). Packaging only — no engine change, no licence work.

- Stable extension id first. The repo host manifest carried the
  placeholder `__FETCHLINE_EXTENSION_ID__` in `allowed_origins`, and
  `manifest.json` had no `key`, so an unpacked load got a random id on
  every machine and no shipped manifest could ever name it. Fix: RSA-2048
  key pair generated in /tmp (private key NOT committed — only the public
  key goes in `manifest.json`); Chrome derives the stable id from it:
  `mioamdgogndabpcjbdekggcnlkkfpmbj`. `app/extension/manifest.json` now
  carries the `key`; `com.fetchline.host.json` names the real id.
  Firefox is unaffected (uses `browser_specific_settings.gecko.id` =
  `fetchline@fetchline.app` in `allowed_extensions` already).
- Validators now pin the identity chain: `validate_manifest.py`
  requires `key` (base64-decodable, DER SEQUENCE, sane length);
  `validate_host_manifest.py` treats the placeholder as a FAILURE and
  cross-checks `allowed_origins[0]` against the id derived from
  `manifest.json`'s key (SHA-256 -> a-p, implemented in the validator).
  `npm run test:extension` passes.
- Ship the host: `tauri.conf.json` gains
  `bundle.externalBin: ["binaries/fetchline-host"]` (Tauri 2.12
  semantics verified against the vendored tauri-utils config docs:
  it looks for `binaries/fetchline-host{-target-triple}{.exe}`).
  The workflow builds the host (`cargo build --release -p fetchline-host`
  in app/host) and copies it to
  `app/src-tauri/binaries/fetchline-host-x86_64-pc-windows-msvc.exe`
  before `tauri build`; the cargo cache now covers `app/host` too.
  `*.exe` in .gitignore already covers the staged binary — no
  .gitignore change needed.
- Ship the manifests: `bundle.resources` maps both
  `com.fetchline.host.json` and `com.fetchline.host.firefox.json`
  into the install dir (next to `fetchline.exe`, for both MSI and
  NSIS — verified in the vendored tauri-bundler WiX/NSIS templates).
  Their `path` ships as the default MSI path
  (`C:\Program Files\Fetchline\fetchline-host.exe`).
- Registry keys (all HKCU, default value = installed manifest path;
  removed on uninstall):
  `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host`,
  `HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host`,
  `HKCU\Software\Mozilla\NativeMessagingHosts\com.fetchline.host`
  (Firefox points at the `.firefox.json` file).
  - NSIS: `bundle.windows.nsis.installerHooks` ->
    `app/src-tauri/nsis-hooks/fetchline-native-host.nsh`
    (POSTINSTALL writes the three keys + runs
    `register-native-host.ps1` to rewrite both manifests' `path` to
    `$INSTDIR\fetchline-host.exe`, UTF-8 no BOM; PREUNINSTALL deletes
    the three keys). NSIS is per-user by default, so the rewrite is
    required for its default `%LOCALAPPDATA%` install dir.
  - MSI: `bundle.windows.wix.fragmentPaths` +
    `componentGroupRefs: ["FetchlineNativeHost"]` ->
    `app/src-tauri/wix/fetchline-native-host.wxs` (three registry-only
    components, `Win64="yes"`, `KeyPath="yes"`, values
    `[INSTALLDIR]com.fetchline.host.json` / `.firefox.json`;
    Windows Installer removes them on uninstall). The MSI manifest
    path stays the shipped default — a user picking a custom MSI
    folder would need to edit the manifest by hand (documented
    caveat in the fragment + README; no fragment mechanism exists to
    rewrite file contents).
  - HKCU caveat (both installers): the keys land in the installing
    user's hive, so a different Windows user on the same machine
    needs their own install.
- Capture test now tests the genuine installed path
  (`scripts/capture-test.py` Phase 2): it reads the installed
  registry key + manifest and verifies they point at the installed
  host with the stable id, requires the discovered extension id to
  equal the id derived from `manifest.json`'s key, then keeps the
  popup check + drain + SHA-256. It writes NO manifest and NO key
  any more. The CI `extension-capture-test` job drops its Rust setup,
  cargo cache and "Build fetchline-host" steps and passes
  `--host-exe 'C:\Program Files\Fetchline\fetchline-host.exe'` —
  it now depends on the MSI from build-windows for the host + key.
  Phase 1 still dry-runs green locally against the real host binary.
- Workflow: `continue-on-error: true` removed from the "Engine
  tests" step (follow-up to BUG-031, fixed in a separate push: the
  suite did not compile on Windows and stayed hidden precisely
  because that step swallowed the failure). The step now fails the
  run visibly. Nothing else about that step changed. Header comments
  updated to match (engine-tests note + real-download note).
- Docs: `app/extension/README.md` Windows checklist rewritten
  (installer now ships host + key; items 1-2 proven by CI, items 4-7
  still needs-Windows — human right-click, Download All, off
  switches); the native-host section now says the installer does it
  all and keeps the manual steps as dev-only; the "No Windows .exe
  yet" honest-limits bullet updated (installers exist, smoke test
  green, capture proven on the runner — a human browser pass is
  still unproven). S13 row note in `replica/features.csv` appended
  (packaging gap closed).
- Verified on Linux: full engine suite 413/413 (see final report for
  the run), `npm run build` passes, `cargo check --tests` 0 warnings,
  `npm run test:extension` passes, `py_compile` on capture-test.py,
  `tauri.conf.json` valid JSON, .wxs well-formed XML, NSIS ExecWait
  expansion simulated (balanced quotes, no unexpanded `$`).
  The Tauri build itself cannot be verified on this Linux VM
  (gdk-3.0) — CI is the proof: the next run must show the host
  staged in the build log, both installers produced, and the
  capture job green against the installed host + keys.

## Progress window — separate OS window per download, auto-open + Settings toggle (S03) — 2026-11-15

- The in-app progress overlay is gone: each download that starts now opens its own
  separate OS window (Tauri `WebviewWindow`, titled "<percent>% <filename>"), one
  window per download. Closing it never cancels the download; double-clicking (or
  Enter on) a table row reopens or focuses its window. Options > General gains
  "Show the download progress window when a download starts" (default on); off
  leaves the table as the only progress view, double-click still opens the window.
- Window content (IDM's dialog, Fetchline's own look — violet tokens, Inter,
  Lucide, no IDM green bar / layout / wording): filename, source URL, file size,
  downloaded amount, transfer rate, time left, resume capability (Yes/No/Unknown),
  progress bar, per-part bars, 60 s speed graph, Pause/Resume, Cancel, "Hide
  details" (collapses to a compact view and shrinks the window), Open / Open
  folder / Scan again / Quarantine / Delete on completion with the S14 scan
  states, retry countdown from the engine's backoff.
- Resume capability is now stored, not guessed: the probe's `accept_ranges` is
  persisted on the download row (`accept_ranges INTEGER`, NULL = unknown for rows
  added before this was stored; `create_download` takes it; all three row
  SELECTs carry it; `row_to_download` maps it; migration + schema.sql).
- Why the window is built the way it is (BUG-044): this app ships NO Tauri
  capability files, so every frontend `plugin:` command is denied by the ACL
  while app commands run ungated (proven from vendored tauri 2.12.5
  `webview/mod.rs` + the installed `@tauri-apps/api` 2.12.4, which routes
  `listen` to `plugin:event|listen` and `WebviewWindow` creation to
  `plugin:webview|create_webview_window`). So the window is created, focused
  and resized from Rust (`open_progress_window` / `set_progress_window_compact`
  commands), its title is updated from Rust in the progress-event forwarder,
  and its live feed is polling: `list_downloads` + a new `get_progress_snapshot`
  command (the engine keeps the latest `ProgressEvent` per download in
  `DownloadManager::last_progress`, stashed in `emit()`). The old overlay's
  "live" events never arrived in production — the 1 s table poll was always
  the real live path — so the main window's dead `onProgress` listener is
  removed (the `onProgress` bridge helper goes with it); the window's 500 ms
  poll is also what feeds its speed graph, so the graph actually builds here.
- Verified on Linux: full engine suite **416/416 pass, 0 failed** (413
  before + 3 new: settings round-trip for `show_progress_window`,
  `accept_ranges` persistence, `last_progress` snapshot matches the
  completion event — all three pass by name in the log), `cargo check
  --tests` 0 warnings, `npm run build` passes. The Tauri shell itself
  cannot be compiled or run on this Linux VM (gdk-3.0) — the new
  `WebviewWindowBuilder` / `set_title` / `set_size` calls were verified
  against vendored tauri 2.12.5 source instead, and the window cannot be
  GUI-tested on the VPS either (session 0, no desktop). CI is the proof:
  the smoke test must still show the app launching, and the window needs
  a human pass on real Windows (auto-open on start, title updates, Hide
  details resize, close-does-not-cancel).

## Sonic Manager rebrand + retheme, BUG-044 fix — 2026-11-15
- Done: display name "Sonic Manager" everywhere visible — `productName`
  + window title in `tauri.conf.json`, extension `name` / `description` /
  `default_title`, README titles, all user-visible "Fetchline" copy in
  the app and extension. Identifiers unchanged (`com.fetchline.app`,
  `com.fetchline.host`); the repo rename is a separate step. No mapping
  note about the internal identifier is written anywhere in the project
  files, per the user's instruction.
- Done: theme restyle from the user's own images (logo, background,
  reference mockup — inspiration only, not shipped). Palette sampled
  with PIL: dark navy #0a0f24, neon red #f73244, neon blue #23a9ea
  (gold #d9b350 is in the background art only, not a UI token).
  Dark theme is the neon look; light theme uses deeper red #c92a2a /
  blue #0b6fc4 with white button text. Tokens live in
  `replica/design/tokens.json` (mirrored in `tokens.css` and
  `app/src/index.css`); the extension popup is restyled dark Sonic.
- Done: new `ProgressRing` component — circular progress ring from
  the reference mockup's download cards (24 px in table rows, 56 px
  with the percent inside in the progress-window header). Primary
  buttons get the glow shadow (`shadow-glow` in both tailwind
  configs).
- Done: BUG-044 fix (approach (a), capabilities file — the standard
  Tauri fix): `app/src-tauri/capabilities/default.json` grants
  `core:event:allow-listen` + `core:event:allow-unlisten` to the
  `main` window only, so the `queue-action-pending` and
  `update-available` listeners in App.tsx work as written. Nothing
  else is granted; progress windows keep polling by design and the
  removed `download-progress` listener stays removed.
- Icons + background (held for a follow-up push — binary files cannot
  go through the connector): `scripts/generate-icons.py` now reads
  `app/src-tauri/icons/logo.jpg` (fails loudly if missing) and makes
  the 1024 app icon from the full logo plus extension icons
  16/32/48/128 — 16/32 use a head crop of the mark's curl, because
  the whole logo at 16 px turns to mud (verified by eye). The
  background JPG becomes the main-window backdrop under a dark
  scrim (`app/src/background.css`, dark theme only); the app root
  divs carry no `bg-bg` class so the body backdrop shows through.
  The logo/background JPGs must reach the repo via the GitHub web UI
  BEFORE that follow-up lands — the workflow runs generate-icons.py
  in both jobs, so landing the adapted script without logo.jpg would
  fail CI at the icon step.
- Verified on Linux: full engine suite **416/416 pass, 0 failed**
  (the extension-manifest test now expects "Sonic Manager");
  `cargo check --tests` 0 warnings (engine + host; the Tauri app
  crate cannot be checked on this VM — gdk-3.0, pre-existing);
  `npm run build` passes; the background build was verified with a
  temporary import (asset emitted, import reverted); contrast
  re-checked on the tokens.json pairs — **24/24 pass** (text 4.5 /
  UI 3.0), light and dark.
- Honest limits: the new look has never been seen on real Windows
  (no Tauri shell build on this VM, no desktop on the VPS); the CI
  smoke-test screenshot is the launch proof and a human pass on the
  user's PC is needed for the final look. Engine error strings and
  the HTTP User-Agent still say "Fetchline" (out of this round's
  scope — needs a follow-up decision).

## Round 41 — workflow path fix (Sonic Manager folder) + theme finish: icons from the user's logo + background backdrop — 2026-11-16

- Workflow path fix (browser route, commit a45e225 on top of
  b809284, 4+/4-): the rename moved the install folder, so the two
  test jobs' hardcoded `C:\Program Files\Fetchline\fetchline.exe`
  (smoke-test line 179, capture-test line 297) and the capture
  command's `--host-exe` / `--app-exe` (line 336, plus the line 333
  comment) now read `C:\Program Files\Sonic Manager\...`. The
  `--db-path` (`com.fetchline.app`), the Chrome path, and the pinned
  raw URL are untouched. A parent diff of the repo file against
  local found exactly one difference — line 315, a comment whose
  leading spaces differ (cosmetic; YAML ignores comment indent).
  Run #10 at this head: build-windows SUCCESS, smoke-test SUCCESS
  (the path fix is proven — the app launched), extension-capture-test
  FAILED at "Run browser capture test" (see BUG-045 in bugs.md).
- Images in the repo (browser route, connector cannot push
  binaries): `app/src-tauri/icons/logo.jpg` (commit fce8f0d, 116 KB)
  and `app/src/assets/background.jpg` (commit d516643, 140 KB),
  both verified HTTP 200 and rendering. Repo head before this
  push: d516643.
- Theme finish (this push): `scripts/generate-icons.py` reads
  `app/src-tauri/icons/logo.jpg` (fails loudly if it is missing)
  and writes the 1024 app icon from the full logo plus extension
  icons 16/32/48/128 — 16/32 use the head crop of the mark's curl,
  48/128 the full logo. `app/src/background.css` puts the
  background JPG under a dark scrim as the body backdrop, dark
  theme only; `app/src/main.tsx` imports it right after
  `./index.css`. The app root divs (App.tsx, ProgressWindow.tsx)
  carry no `bg-bg` class, so the body backdrop shows through.
  BUG-045 (capture test's expected popup string, stale after the
  rebrand) is fixed in the same push — test-only, see bugs.md.
- Verified on Linux: full engine suite **416/416 pass, 0 failed**;
  `cargo check --tests` 0 warnings (engine and host); `npm run
  build` passes and emits the background asset
  (`dist/assets/background-*.jpg`, 143.45 kB); icons generated
  locally at every size (icon.png 1024x1024; icon16/32/48/128);
  `py_compile` OK on the edited capture-test.py. The generated
  PNGs are git-ignored binaries and are not pushed — CI regenerates
  them from logo.jpg.
- Honest limits: the finished look has never been seen on real
  Windows (no Tauri shell build on this VM, no desktop on the VPS);
  the CI smoke-test screenshot from this run's artifact is the
  launch proof and a human pass on the user's PC is needed for the
  final look. Run #8's single flaky FTP pause/resume test
  ("file finished before the pause") did not reproduce in run #9
  and is not a real defect; if it recurs it needs a test-only
  timing fix (wait-for-active before pausing).

## Round 42 — MSI host-manifest path fix (BUG-046) — 2026-11-16

- BUG-046 (see bugs.md): run #11's capture test failed Phase 2 at
  the manifest check — the MSI ships `com.fetchline.host.json` and
  `com.fetchline.host.firefox.json` with the old hardcoded
  `"path": "C:\\Program Files\\Fetchline\\fetchline-host.exe"`, but
  the rename moved the install folder to `C:\Program Files\Sonic
  Manager\`. The registry key was already correct; only the
  manifest FILE's path field was stale. Only NSIS rewrites the
  path at install time, so the MSI was broken for every default
  install since the rename.
- Fix (this push): both manifests' `path` now reads `C:\Program
  Files\Sonic Manager\fetchline-host.exe`. Stale old-path references
  updated in the comments of `register-native-host.ps1`,
  `fetchline-native-host.nsh`, `fetchline-native-host.wxs`, and the
  `capture-test.py` docstring example (cosmetic). No identifier
  changed; workflow file untouched (browser route only).
- Verified on Linux: both manifests parse as JSON (name
  `com.fetchline.host`, type `stdio`, allowed_origins /
  allowed_extensions unchanged); full engine suite **416/416 pass,
  0 failed** (confirmed twice); `cargo check --tests` 0 warnings;
  `npm run build` passes. Repo-wide grep: no
  `C:\Program Files\Fetchline` left in code, only historical
  build-log entries.
- This run also finally exercises the BUG-045 popup-string fix —
  Phase 2 previously died before the popup handshake, so the popup
  check is unproven until this run.
- **Verdict (run #12, 43770074030, head de396268 — 2026-11-16):
  FULLY GREEN.** build-windows SUCCESS, smoke-test SUCCESS,
  extension-capture-test SUCCESS; installer 15,863,208 bytes
  (~15.9 MB). The capture job passing proves BUG-046 and BUG-045
  together — Phase 2's strict popup check ("Connected to the Sonic
  Manager desktop app") was reached and passed. (The report.txt text
  itself was not read from this session — artifact download returns
  401 — but the job's success verdict is the proof, since the
  script fails the job on any phase failure.) Run #12's smoke
  screenshot still shows the LIGHT theme — dark by default is
  Round 43.

## Round 43 — dark theme by default + header logo — 2026-11-16

- User feedback on the run #11 smoke screenshot: it does not look
  like his dark reference picture, the background is missing, and
  the header shows no logo (only the tiny OS window icon). Cause:
  the app opens in LIGHT theme by default and `background.css`
  applies the backdrop only under `[data-theme="dark"]`; the header
  is just the "Sonic Manager" text h1. He approved dark as the
  default theme.
- Change (this push): dark by default — `App.tsx` initial theme
  state `'light'` → `'dark'` and `Settings::default` in `store.rs`
  theme `"light"` → `"dark"` (fresh installs / fresh DBs start
  dark; the stored setting still wins for existing users, and the
  moon/sun toggle still switches to light). The
  settings-defaults test now expects `"dark"`. Header logo:
  `<img>` 32 px next to the title, importing
  `./assets/logo.jpg` (the user's logo, already in the repo;
  copied into the local tree for the build — binary, not pushed
  via the connector). New `app/src/vite-env.d.ts` with
  `/// <reference types="vite/client" />` — required because
  `npm run build` runs `tsc -b` first and TypeScript rejects a
  `.jpg` import without it. The progress window got no logo: its
  header is the download filename, so a mark there would compete
  with it; it applies the stored theme on mount, so it follows
  the dark default automatically.
- Verified on Linux: full engine suite **416/416 pass, 0 failed**;
  `cargo check --tests` 0 warnings; `npm run build` passes and
  emits both assets in dist/assets (background-*.jpg and
  logo-*.jpg); App root carries no `bg-bg` class, so the backdrop
  still shows through in dark mode. The run this push triggers is
  the proof — its smoke screenshot should show the dark theme
  with the background and the header logo.

## Round 44 — main window rebuilt to IDM's layout, in Sonic colors — 2026-11-16
- User instruction (with two reference images): the app should look
  EXACTLY like IDM's design — same layout and icons — only with OUR
  colors from the second picture. References: IDM 6.38 light
  (`~/workspace/user/files/17655_3_ggwt.png`) and the same layout
  recolored in Sonic colors (`~/workspace/user/files/17657_4_wx7s.jpg`).
  Clean-room rule stands: no IDM brand, logo, or icon artwork copied —
  Lucide icons with the same meanings/positions, in the Sonic palette.
  No Registration menu (licence S11 stays set aside) and no "Tell a
  Friend" button. Every existing feature kept, rearranged.
- Menu bar (new `app/src/components/MenuBar.tsx`): logo 28 px at the
  left, then Tasks / File / Downloads / View / Help; status text +
  theme toggle at the right. Tasks: Add URL, Add batch, Download all
  links, Resume, Stop, Stop all, Delete, Delete completed. File: Open
  folder, Remove selected, Settings. Downloads: Video grabber, Site
  grabber, Move to category (all categories), Start Queue / Stop Queue
  / Sync queue now (the selected queue, else the main queue). View:
  Dark theme (checkmark), Progress window. Help: Check for updates,
  Settings. One menu open at a time; Escape / click-outside closes;
  hovering another top entry switches. The old header (logo + title +
  status + theme toggle + Help dropdown) is gone.
- Toolbar (new `app/src/components/Toolbar.tsx`): IDM order, 68 px
  tall, big icons (28 px) + one-line labels in fixed 76 px columns
  with `truncate` — this also fixes the Round 43 nit of the "Add a
  link" label wrapping to two lines. Add URL (Plus, info), Resume
  (Download, info), Stop (Ban, danger), Stop All (Hand, danger),
  Delete (Trash2, muted), Delete Completed (Trash2, danger), Options
  (Settings, info), Scheduler (Clock, accent), Start Queue (Play,
  info), Stop Queue (Square, danger), Grabber (Globe, info + dropdown:
  Video grabber / Site grabber / Download all links / Batch). Stop All
  pauses every downloading download (frontend loop over the existing
  `pauseDownload` — no engine command exists); Delete Completed
  deletes every complete download (loop over `deleteDownload`).
  Disabled states as before (greyed when nothing selected). The
  move-to-category select left the toolbar — it lives in the Downloads
  menu now.
- Categories tree (`CategorySidebar.tsx` rewritten): Categories
  (All downloads + categories + New category), Unfinished (Clock
  icon, count = status != complete) and Finished (CircleCheck icon,
  count = complete), "Grabber projects" section (Globe icon, name,
  `files_downloaded` count, "Running" pill while exploring or
  downloading, hover Play button runs the project), Queues section
  (unchanged). New `SidebarFilter` type `'all' | 'unfinished' |
  'finished' | 'grabber'`; App holds `selectedFilter` +
  `selectedGrabberId` alongside `selectedCategory`/`selectedQueue`,
  and selecting any one clears the others.
- Table (`DownloadTable.tsx` rewritten): columns File Name
  (ProgressRing 24 px + filename), Q (queue name or '—'), Size,
  Status (StatusBadge + percent text for downloading/paused rows),
  Time left, Transfer rate, Last Try, Description. The old Category
  column is gone (the sidebar filter shows it) and the old Progress
  bar column is gone (ring + percent-in-status replace it).
  Description = error_message first, then the quota-paused note,
  then the scan-threat note, else empty. No information lost.
- Engine change (minimal, needed for the Last Try column and the
  grabber filter): `DownloadRow` gained `updated_at: String` +
  `grabber_project_id: Option<String>` (both `#[serde(default)]`)
  in `app/engine/src/store.rs`; the three SELECTs feeding
  `row_to_download` (`list_downloads`, `list_queue_files`,
  `get_download`) now also select `updated_at, grabber_project_id`
  (indices 24, 25 — appended, no renumbering; BUG-026 lesson: the
  three SELECTs must stay in sync). Frontend `types.ts` DownloadRow
  gained the same two fields. "Last Try" semantics: `updated_at`
  is bumped on every status change and on the 2 s checkpoint while
  downloading, so it is the engine's closest record of a last try.
  Kept test added: `list_downloads_carries_updated_at_and_grabber_project`
  in `engine_tests.rs` (creates a real grabber project first — the
  first version tagged a nonexistent project id and failed on a
  FOREIGN KEY violation, which is how the test earned its keep).
- Progress window (`ProgressWindow.tsx` restructured): IDM's three
  tabs — Download status (the current fields: dl grid, quota note,
  speed graph, per-part bars, retry countdown, error, finished-file
  block), Speed Limiter (per-download cap input wired to the
  existing `setDownloadSpeedLimit`, live-changeable; empty/0 = no
  cap; the note says the effective rate is the min of the
  per-download cap and the global cap), Options on completion
  (queue completion-action editor via the existing `updateQueue` —
  none / open file / run program / shutdown / hibernate + program
  input when run_program + Save; the 60 s countdown note; the S14
  scan status + Scan again / Quarantine / Delete actions). The tab
  bar hides in compact mode; the tall progress bar sits below the
  tabs (IDM's order) and stays always visible. Closing the window
  still never cancels the download. Scan rendering is a local
  `ScanStatus` function inside `ProgressWindow.tsx` (not a separate
  file — both tabs live in that file, so a shared file would buy
  nothing), used by both the status tab's finished block and the
  completion tab. Two review catches fixed before the build: the
  queue-fetch effect now depends on the `queue_id` string, not the
  whole row (the 500 ms poll replaces the row object every tick
  and would have refetched + reseeded the completion editor while
  the user types), and the cap handler takes an optional value
  parameter so Remove cap passes '' instead of reading the stale
  input from its closure.
- Contrast re-check (WCAG AA, inline checker in /tmp, not
  committed): **58/58 pass, 0 failed** — the 24 pairs from
  tokens.json plus 34 new toolbar-icon pairs (accent / info /
  danger / success / warning on surface, surface-raised and bg,
  both themes; text-faint on surface for disabled icons;
  text-muted on surface-raised). Lowest margin is the pre-existing
  light-border-input on light-bg at 3.07 (UI threshold 3.0).
- Verified on Linux: full engine suite **417/417 pass, 0 failed**
  (416 + the new test); `cargo check --tests` 0 warnings; `npm run
  build` passes. The run this push triggers is the proof — its
  smoke screenshot should show the new menu bar, toolbar,
  categories tree and table in the dark Sonic theme.

## Round 45 — the user's asset pack: menu bar gone, light theme deleted, pack icons + neon-waves background — 2026-11-16

The user sent `Sonic_Manager_Assets` (ZIP, 2.1 MB) and approved the
plan first (with one change: delete the light theme — dark only).
The pack: 18 SVG line icons (24×24, stroke 1.8, currentColor), the
lightning logo (purple-gradient rounded square + black bolt, cyan
outline), the neon-waves SVG background, colors.json + theme.css
(purple #8B20FF, cyan #00C8FF, pink #F02DCD, bg #0B0F24, surface
#111833, muted #AAB4D0, border #27345C), two reference PNGs, and
the instruction file (UI only, no engine changes, no new features,
use the pack's SVGs/colors/background). The preview image mentions
PNG icons/backgrounds and a font folder, but the zip has SVGs only
— PNGs are generated from the SVG at build time (below), and the
font stays Inter with Segoe UI fallback (the logo SVG declares
"Segoe UI, Arial, sans-serif"; no font file was supplied).

- **Menu bar removed** (`app/src/components/MenuBar.tsx` deleted;
  the reference has none — the native Windows title bar is the
  title bar). Its actions were re-homed, none dropped: Batch +
  Download all were already in the Grabber dropdown; Open folder +
  Move to category moved to a new right-click menu on table rows
  (`DownloadTable.tsx`); Sync queue now is a hover button on queue
  rows in the sidebar; Check for updates is a button in Settings
  (update section). The status text the menu bar showed is covered
  by the footer. Resume/Stop/Stop All/Delete/Delete Completed were
  already toolbar buttons.
- **Light theme deleted** (user's explicit decision): `:root` IS
  the dark palette now; no `[data-theme]` switch anywhere. Removed
  the light tokens from `app/src/index.css`,
  `replica/design/tokens.css` and `replica/design/tokens.json`
  (dark-only, 19 pairs), the theme toggle, and the theme option in
  Settings. `data-theme="dark"` is still set on mount (App +
  ProgressWindow) so the Tailwind dark-mode selector keeps working.
  The engine still stores a `theme` field and accepts 'light'
  (no engine change); the UI never reads it for theming any more
  (`app/src/lib/types.ts` carries a comment saying so). No engine
  test asserts on the light theme, so none changed.
- **New palette** from the pack everywhere (toolbar, sidebar,
  table, dialogs, progress window, extension popup): purple accent
  #8B20FF, cyan info #00C8FF, pink danger #F02DCD; the red accent
  #f73244 is gone. The old JPG background (lightning + gold rings)
  is replaced by the neon-waves SVG (`background.css`, lighter
  scrim 0.55/0.70 so the waves stay visible at the bottom-right as
  in the reference). `background.jpg` stays in the repo unused (a
  binary file — it is no longer referenced).
- **Pack icons** (`app/src/components/PackIcon.tsx`, new): the 18
  SVGs are copied into `app/src/assets/icons/` and imported with
  `?raw` and inlined — as `<img>` their `currentColor` would fall
  back to black and the per-action colours would be lost. Toolbar:
  Add URL is the big glowing purple button, Options the big cyan
  gear in a purple halo (radial gradient + glow shadow), Resume /
  Start Queue / Grabber cyan, Stop / Stop All / Delete Completed /
  Scheduler pink, Delete white. Columns widened to 96 px, so
  "Delete Completed" fits on one line (Round 44 truncated it).
  Sidebar: category-folder / unfinished / finished / grabber /
  queue pack icons; the active row is the reference's filled
  purple row with white text; sidebar widened 232 → 256 px, so
  "Main download queue" fits. Empty state: the pack's glowing
  download icon + link icon on the "Add a link" button. The small
  hover chrome (delete category, run grabber, scheduler, sync) is
  still Lucide — the pack has no equivalents.
- **Logo + app icon**: the header logo is the pack's lightning
  mark as the native title-bar icon, via the generated icon set.
  `scripts/generate-icons.py` now renders the mark from
  `app/src-tauri/icons/logo.svg` (text, in the repo) with Pillow
  only — diagonal gradient as anti-diagonal lines, rounded-square
  mask, bolt polygon with cyan outline — so CI needs no new
  dependency. Caught before building: the first version rotated a
  horizontal gradient, whose corners fill black and showed as dark
  triangles; the anti-diagonal version was verified by eye (purple
  → deep-blue rounded square, dark bolt, cyan outline). Falls back
  to the old `logo.jpg` if `logo.svg` is missing, and fails loudly
  if neither exists. The mark is a bold square, so all four
  extension icons use the whole mark (the old head-crop is gone).
- **Contrast** (WCAG AA, inline checker in /tmp, not committed):
  **19/19 pass, 0 failed** on the new dark-only pairs. One catch
  fixed on the way: the first derived border-input #4a5b8a scored
  2.85 (UI threshold 3.0); it is now #5a6b9a (3.61), the value the
  old dark theme used.
- No engine changes, no new features, no renamed or deleted
  buttons or functions. The table keeps Round 44's IDM columns;
  the progress window keeps its three tabs.
- Verified on Linux: full engine suite **417/417 pass, 0 failed**;
  `cargo check --tests` 0 warnings; `npm run build` passes (the
  background SVG is inlined in the built CSS as a data URI; the
  `?raw` icons are inlined in the JS). The run this push triggers
  is the proof — its smoke screenshot should show the new look:
  no menu bar, purple Add URL button, glowing gear, neon waves.

## Round 46 — match the reference exactly at 1024×768: toolbar fits, queue name fits, white empty-state icon — 2026-11-16

The user confirmed the pack's design-reference image (also
`~/workspace/user/files/17670_1_7tp5.webp`) is the final look. The
run #19 smoke screenshot (1024×768) showed two layout defects
against it, logged as BUG-047 and fixed here. Nothing else changes:
palette, icons, logo, menu-bar removal and all Round 45 work stay
as-is; dark only.

- **Toolbar fits at 1024 px (BUG-047a):** the 96 px fixed columns ×
  11 buttons totalled ~1160 px, so Stop Queue and Grabber were cut
  off behind a horizontal scrollbar. Buttons are now
  `flex-auto` with `min-width: fit-content`: each sizes to its
  content and grows to fill the row, so all 11 buttons fit with
  room to spare and "Delete Completed" stays on one line. The
  container is `overflow-hidden` instead of `overflow-x-auto`, so
  a scrollbar can never appear. Fit checked with real font
  metrics (DejaVu Sans Bold, wider than Inter, as the upper
  bound): content totals ~873 px against 1008 px available.
- **"Main download queue" fits (BUG-047b):** the sidebar went
  256 → 280 px, and the hover actions (delete category, run
  grabber, sync/schedule queue) moved out of the row's flow into
  an absolutely-positioned overlay at the row's right edge — the
  in-flow buttons, invisible or not, still took 24 px each, which
  is what truncated the name. The overlay fades in on
  hover/focus and the count fades out under it; the buttons stay
  keyboard-focusable (opacity-0, never display:none), and the
  Running pill fades with the count. Same treatment for the
  category delete and grabber run buttons, which had the same
  latent truncation for long names.
- **Empty state matches the reference:** the download icon is
  white now (was purple) with a soft white glow, and the backdrop
  scrim lightened 0.55/0.70 → 0.45/0.60 so the neon waves show
  more prominently across the bottom, as in the reference.
- Verified on Linux: full engine suite **417/417 pass, 0 failed**;
  `cargo check --tests` 0 warnings; `npm run build` passes. No
  engine changes, no new features, no renamed or deleted buttons
  or functions. The run this push triggers is the proof — its
  smoke screenshot should show all 11 toolbar buttons with no
  scrollbar, "Main download queue 0 files" in full, and the white
  empty-state icon.

## Round 47 — stronger neon, clearer waves, bigger icons, gradient Add a link, glowing panels — 2026-11-16

The user approved the plan: the current design is good, refine it
further toward the reference (the same design-reference image).
Visual only — no engine changes, no new features, no renamed or
deleted buttons or functions; dark only.

- **Stronger neon glow:** the glow tokens went three-layer (hot
  core + mid halo + wide wash) in `index.css` and
  `replica/design/tokens.json` — purple
  `0 0 12px rgba(139,32,255,.75), 0 0 28px rgba(139,32,255,.45),
  0 0 48px rgba(139,32,255,.22)`, cyan the same shape. Toolbar
  icons now glow constantly via new `.icon-glow-{cyan,pink,
  purple,white}` classes (`drop-shadow`, which follows the SVG
  strokes) and brighten on hover; the Add URL button uses the
  stronger `shadow-glow`.
- **Clearer gradient waves:** the neon-waves SVG strokes are
  brighter and thicker (purple glow 28→34 px at .85 opacity,
  cyan 34→40 px at .80, bottom wave .35→.50) and the backdrop
  scrim lightened again, 0.45/0.60 → 0.30/0.45, so the waves read
  clearly across the bottom.
- **Options gear glows strongly blue + purple:** the halo is
  48 → 56 px with a cyan-centred radial gradient and a
  three-layer cyan + purple box-shadow
  (`0 0 18px cyan .60, 0 0 36px purple .55, 0 0 60px purple .30`);
  the gear itself is 32 → 36 px with the cyan icon glow.
- **Bigger, clearer icons:** toolbar icons 28 → 32 px, labels
  11 → 12 px, toolbar 84 → 92 px tall; sidebar icons 16 → 20 px
  and rows 32 → 36 px. The Round 46 fit still holds: re-checked
  with real font metrics (DejaVu Sans Bold as the upper bound),
  toolbar content totals ~949 px against 1008 px available, and
  the sidebar row totals ~258 px against 280 px — "Delete
  Completed" stays on one line, "Main download queue 0 files"
  stays in full.
- **"Add a link" is a purple→blue gradient:** the empty-state
  button is now `linear-gradient(90deg, #8b20ff, #2563eb)` with
  the stronger glow, 44 px tall; the heading is 20 px bold and
  the description 15 px.
- **Glowing soft borders:** the sidebar and the main area are
  now rounded panels (`rounded-xl`, new `shadow-panel` token:
  a 1 px purple edge at .28 plus purple/cyan washes), inset 8 px
  from the toolbar and footer; the main panel is translucent
  (`bg-surface/40`) so the waves show through behind the table,
  and the active sidebar row glows.
- **Spacing / sizes / fonts:** toolbar 84 → 92 px, sidebar
  section headers letter-spaced, empty-state gaps 12 → 16 px.
  Font stays Inter with Segoe UI fallback (the pack has no font
  files); `tokens.json` layout values updated (sidebar 280,
  toolbar 92).
- **Contrast (WCAG AA):** every changed pair passes — white on
  the gradient ends 5.60 / 5.17, white / muted / faint on the
  translucent main panel 18.36 / 8.87 / 5.43. Known pre-existing
  issue, NOT introduced here: the accent purple #8b20ff as small
  text scores ~2.8 on dark surfaces (dialog tabs, StatusBadge) —
  the two `dark-accent` pairs in tokens.json score 3.38 / 3.12,
  passing the 3.0 UI threshold but not the 4.5 text threshold.
  Fixing it means a lighter text-purple token; left for a
  palette decision, not this round.
- Verified on Linux: full engine suite **417/417 pass, 0 failed**;
  `cargo check --tests` 0 warnings; `npm run build` passes. The
  run this push triggers is the proof — its smoke screenshot
  should show the stronger glow, the clearer waves, the 32 px
  icons, the gradient Add a link button and the glowing panels,
  with all 11 toolbar buttons still fitting at 1024 px.

## Round 48 — all icons bright and glowing, not just Add URL and Options — 2026-11-16
- The user looked at the run #21 screenshot and said the rest of
  the icons look faded ("باقي الأيقونات باهته"): Add URL and the
  Options gear glow strongly, while the other toolbar icons and
  the sidebar icons read as dim next to them. Two causes, both
  fixed here. First, seven of the nine plain toolbar buttons are
  disabled in the empty state, and `disabled:opacity-45` washed
  them to 45% — grey and washed-out by construction. Second, the
  icon glow was only two modest layers (5 px / 14 px), far weaker
  than the Options halo (18 / 36 / 60 px box-shadow plus a radial
  gradient disc) and the Add URL button's three-layer glow.
- **Stronger glow, everywhere:** `.icon-glow-{cyan,pink,purple,
  white}` in `app/src/index.css` are now three layers at the
  Options halo's strength (core 8 px at .85–.90, mid 20 px at
  .50–.55, wide 36 px at .25–.30; hover brightens to 10 / 24 /
  44 px). Every toolbar icon — Resume, Stop, Stop All, Delete,
  Delete Completed, Scheduler, Start Queue, Stop Queue, Grabber —
  uses the same treatment in its own colour, and the Options gear
  gets stronger with it. The matching `icon-glow-*` values are
  recorded in `replica/design/tokens.json` so the two stay in sync.
- **Disabled buttons stay bright:** `ToolbarButton` disabled
  opacity 45% → 75%. At 75% the icons still read as clearly
  dimmer than enabled buttons (and hover gives nothing), while
  no icon reads as grey or washed-out any more. The generic
  `Button.tsx` and the table context-menu item keep 45% — only
  the toolbar was flagged.
- **Thicker, clearer strokes:** all 18 pack SVGs in
  `app/src/assets/icons/` went from stroke-width 1.8 → 2.0
  (verified: 18/18 carry 2.0, 0 remain at 1.8), so the line icons
  read clearly at 32 px. The change is in the repo's icon copies,
  not the user's pack.
- **Sidebar icons glow too:** category folders, Unfinished,
  Finished and queue rows use `icon-glow-purple`, grabber project
  rows `icon-glow-cyan`, so the tree doesn't read dim next to the
  toolbar. Colours are unchanged (the icons keep inheriting the
  row's white text — the active purple row keeps a white icon).
- **Sizes unchanged, fit holds:** toolbar icons stay 32 px,
  sidebar icons 20 px, toolbar 92 px. Re-checked with real font
  metrics (DejaVu Sans Bold as the upper bound, same script as
  Round 47): toolbar content **949.4 px vs 1008 px available**,
  sidebar row 258.1 px vs 280 px — all 11 buttons fit at 1024 px
  with no scrollbar and "Delete Completed" stays on one line.
- **Contrast (WCAG AA):** the changed pairs pass — disabled label
  white at 75% on the toolbar surface 10.15, icon cyan 8.90,
  pink 4.96, purple 3.12 (UI threshold 3.0). The only failures in
  the full check are the two pre-existing `dark-accent` pairs
  (3.38 / 3.12), the small-purple-text issue flagged in Round 47
  — unchanged by this round, still awaiting a palette decision.
- No engine changes, no new features, no renamed/deleted buttons
  or functions. Dark only (light theme stays deleted). Palette,
  background, borders, spacing and all Round 47 work untouched.
- Verified on Linux: full engine suite **417/417 pass, 0 failed**;
  `cargo check --tests` 0 warnings; `npm run build` passes. The
  run this push triggers is the proof — its smoke screenshot
  should show every toolbar and sidebar icon bright and glowing
  at 1024 px, with all 11 toolbar buttons still fitting.

## Round 48 — CI verdict (run #22, 2026-11-16)
- **Run #22 (43788891810, head ab3af71f) FAILED at build-windows,
  step 8 "Engine tests (smoke check)" (exit code 1); smoke-test
  and extension-capture-test were skipped.** The exact failing
  test is NOT established from the API: the job-logs endpoint
  returns 403 ("Must have admin rights to Repository") and the
  check-run annotations only say "Process completed with exit
  code 1". A browser task signed in as xsonicx1998 is needed to
  read the step-8 log and name the failing test.
- This change touched zero engine code (CSS, three TSX files,
  18 SVG icons, tokens.json, this log) and the engine suite
  passed locally 417/417 immediately before the push, so the
  failure cannot be caused by Round 48. Prime suspect,
  unconfirmed: the known flaky `ftp_pause_then_resume_completes_
  byte_exact` timing race, which failed at exactly this step in
  run #8 and never reproduced. No fix loop was started; the
  verdict note rides with the next push.

## Round 48 CI-fix — flaky FTP pause test (BUG-048) — 2026-11-16
- **Diagnosis CONFIRMED by a browser log read:** exactly ONE
  test failed in run #22 — `ftp_pause_then_resume_completes_
  byte_exact` (`app/engine/tests/proxy_tests.rs:716:5`), "file
  finished before the pause". Per-suite totals: 331 passed,
  1 failed (proxy_tests 14 passed, 1 failed; all other suites
  ok). Steps 1–7 passed; smoke + capture were skipped.
- **Fix, test-only (BUG-048, see `replica/bugs.md`):** the test
  replaced its blind 200 ms sleep with wait-for-active
  (`wait_for_status(..., "downloading", 10)`) before pausing,
  and the fixture chunk delay went 20 ms → 50 ms (download now
  ~400 ms, so the pause lands mid-download). No engine source,
  no other test, no UI file changed; the test asserts exactly
  what it asserted before.
- Verified on Linux: proxy_tests 15/15 three consecutive runs;
  full engine suite **417/417 pass, 0 failed**; `cargo check
  --tests` 0 warnings; `npm run build` passes. The run this
  push triggers is the proof — it must go green so the user can
  see the Round 48 look.

## Round 48 CI-fix — CI verdict (run #23, 2026-11-16)
- **Run #23 (43790293212, head 9bca7f53) FAILED again at
  build-windows, step 8 "Engine tests (smoke check)" (exit
  code 1); smoke-test and extension-capture-test were skipped.**
  The fix passed locally (proxy_tests 15/15 three times, full
  suite 417/417), so this is NOT a local-verification miss.
- **The exact failing test is NOT established from the API:**
  the check-run annotations only say "Process completed with
  exit code 1" (plus an unrelated Node 20 deprecation warning)
  and the job-logs endpoint returns 403. A browser task signed
  in as xsonicx1998 is needed to read the step-8 log of job
  133046938946 and name the failing test. Two possibilities:
  the same FTP test failed again despite the fix (the margin
  analysis: pause flag noticed within 250 ms of being set vs
  ~400 ms download — thin on a loaded runner), or a DIFFERENT
  test failed. No fix loop was started; the verdict note rides
  with the next push.

## Round 48 CI-fix #2 — BUG-048 follow-up: wait for real progress, not status — 2026-11-16
- **Diagnosis CONFIRMED by a browser log read:** exactly ONE test failed in run #23 — `ftp_pause_then_resume_completes_byte_exact` (`app/engine/tests/proxy_tests.rs:726:5`), "pause landed before any bytes". Per-suite totals: 331 passed, 1 failed (proxy_tests 14 passed, 1 failed; all other suites ok). Steps 1–7 passed; smoke + capture were skipped. This is the SAME test with the OPPOSITE timing failure to run #22 ("file finished before the pause").
- **Root cause (proven from the engine source):** `set_status("downloading")` is the run task's FIRST step, before any segment is spawned — so the BUG-048 wait-for-status gate lets the pause land before a single byte is written. The pause window is bounded on BOTH sides: bytes > 0 AND bytes < total at pause time.
- **Fix, test-only (BUG-048 follow-up, see `replica/bugs.md`):** the wait-for-status gate is replaced with a wait for REAL PROGRESS — new helper `wait_for_part_bytes` polls the download's `.part` file every 5 ms until it holds a non-zero payload byte, then pauses (10 s timeout fails loudly, never pauses blind). The DB row cannot be the gate: progress is checkpointed to SQLite only every 2 s while the whole download takes ~400 ms, so the row reads 0 throughout. The `.part` file is pre-sized with `set_len` (all zeros) and the fixture payload is `(i % 251)`, so the first non-zero byte proves a segment has written; pausing then (~30–80 ms after spawn) leaves ~1 MiB for the run loop's ≤250 ms notice latency. The 50 ms fixture chunk delay is kept. No engine source, no other test, no UI file changed; the test asserts exactly what it asserted before.
- Verified on Linux: proxy_tests 15/15 five consecutive runs; full engine suite **417/417 pass, 0 failed**; `cargo check --tests` 0 warnings; `npm run build` passes. The run this push triggers is the proof — it must go green so the user can see the Round 48 look.
---
## Round 49 — Options halo shrinks: the gear keeps its glow, the loose circle goes — 2026-11-16
- **User feedback on the run #24 screenshot:** the look is good, but the Options gear's halo is too big — "حوليه دواريه كبيره شويه مش مظبوطه". He said it is NOT our fault: his reference design had it that way. Step 1 of his build sequence (halo → refresh download address → clipboard monitoring → download history if no problems).
- **Fix, UI-only (`app/src/components/Toolbar.tsx`, the Options button's halo span):** the halo disc goes `h-14 w-14` (56 px) → `h-10 w-10` (40 px), and its box-shadow tightens `0 0 18px cyan .60, 0 0 36px purple .55, 0 0 60px purple .30` → `0 0 10px cyan .60, 0 0 20px purple .55, 0 0 32px purple .30`. The gear itself stays 36 px (`h-9 w-9`) with its cyan icon glow. The radial-gradient stops are unchanged (they are relative to the disc, so they scale with it). The halo still reads as a glow hugging the gear — not a hard ring, not gone. Nothing else changes: no palette, no other icon, no size, no layout, no spacing, no engine code.
- **Fit still holds:** re-checked with real font metrics (DejaVu Sans Bold as the upper bound, same method as Rounds 46–48 — this run's script counts all gaps, separators and container padding, so its total reads slightly higher than the earlier 949.4 px): toolbar content **977.9 px vs 1008 px available**, headroom 30.1 px. The Options button is now label-driven (52.6 px label > 40 px disc), so the disc no longer sets the button's width at all — the change saves 3.4 px. All 11 buttons fit at 1024 px with no scrollbar; "Delete Completed" stays on one line. Sidebar untouched (258.1 px vs 280 px, as in Round 48).
- **Contrast:** no text/background pair changed — same colours, same icon, same gradient stops; only the disc diameter and blur radii changed — so the Round 48 contrast results stand unchanged.
- Verified on Linux: full engine suite **417/417 pass, 0 failed** (18 `test result: ok` lines); `cargo check --tests` 0 warnings (explicit grep); `npm run build` passes. The run this push triggers is the proof — it must go green so the user can see the tighter halo.
---
## Round 49 — CI verdict (run #25, 2026-11-16)
- **Run #25 (43795687214, head 8bd2bcd1) FULLY GREEN: build-windows success, smoke-test success, extension-capture-test success.** Installer 15,512,454 bytes (~14.8 MB); smoke artifact 375,151 bytes (slightly smaller than run #24's 381,250 — consistent with the smaller halo); capture artifact 3,566 bytes. Note: the smoke-test job was re-run once (fresh start 03:13:30Z after first completing ~03:11Z) — the re-run also succeeded; who re-ran it is not recorded here. The parent fetches the screenshot from the smoke artifact. This note is local-only and rides with the next push.
---
## Round 50 — BUG-049: the downloads watcher never catches anything — 2026-11-16
- **User report on his own PC (run #25 MSI):** a real mp4 download in Chrome was NOT intercepted (browser downloaded it), right-click → "Download with Fetchline" did NOTHING visible, nothing appeared in the app — popup said "Connected". He closed his browser and asked for the testing to be done without him; the popup's last-handoff line was never provided.
- **Reproduced on the VPS (headless Chrome 157, current extension, freshly built host, fresh app.db, local fixture):** 3 runs, slow + fast fixtures — popup Connected, catch toggle ON, mp4 in the browser's download dir, no pending row, no hand-off. Worker alive, listener registered (`hasListeners()` true), a probe listener fired with `state=in_progress` — the event was delivered, the handler exited silently.
- **Root cause (proven by an instrumented background.js on the VPS copy only, console trace via CDP):** the handler passed every gate (`catch=true desktop=true excl=[]/[]`, `ext=mp4`) and then `chrome.downloads.cancel({ id: item.id })` threw "No matching signature" — `cancel` takes the bare download id, not a query object. The empty `catch {}` swallowed it. The Node fake destructured an object, so the 9-case suite never saw it; the popup handshake never calls cancel, so CI never saw it either.
- **Fix, product (`app/extension/background.js`):** `cancel({ id: item.id })` → `cancel(item.id)`; a cancel failure is now non-fatal for the hand-off (a small file can complete before the async handler runs — the URL still lands in Sonic Manager; only a cancelled download is erased). `erase({ id })` unchanged (erase really takes a query object).
- **Tests:** `watcher.test.js` fake now enforces the real signature (old call fails 4/11 against it, proven on a reverted copy in /tmp) + 2 new BUG-049 tests (bare-id call; cancel failure still hands over, nothing erased) — 11/11. `capture-test.py` gains Phase 4 (watcher): second headless Chrome launch, real click on a SLOW fixture mp4 (512 KiB at ~80 KiB/s), asserts pending row (browser/pending) + no file in the download dir; `--skip-watcher` for local debugging. Also fixed pre-existing Round 40 rebrand leftovers found while verifying: `validate_manifest.py` demanded name "Fetchline" and all 6 `popup.test.js` cases expected "Fetchline" strings (CI never runs them, so CI stayed green) — expectations updated to "Sonic Manager". Extension JS suite now 38/38.
- Verified on the VPS with the fixed background.js: slow fixture INTERCEPTED (pending row, no file, popup "Last sent 4:20:41 AM: Link queued — Fetchline will pick it up") and fast fixture INTERCEPTED the same way. Verified on Linux: full engine suite **417/417 pass, 0 failed**; `cargo check --tests` 0 warnings; `npm run build` passes; extension JS 38/38. The run this push triggers is the proof — its extension-capture-test job must go green with Phase 4 passing.
---
## Round 50 — CI verdict (run #26, 2026-11-16)
- **Run #26 (43804413258, head 266968a7) FULLY GREEN: build-windows success, smoke-test success, extension-capture-test success.** Installer 15,516,734 bytes (~14.8 MB); smoke artifact 375,151 bytes; capture artifact 3,796 bytes (slightly bigger than run #25's 3,566 — the added Phase 4 lines). The capture job's success means Phase 4 passed in CI: a real browser download of the slow fixture mp4 was intercepted (cancelled + erased, pending row landed). The parent fetches the screenshot from the smoke artifact. This note is local-only and rides with the next push.
---
## Round 51 — Refresh download address (step 2 of the user's agreed sequence) — 2026-11-16
- **User order (2026-11-16 ~07:24 EET): "نفذ وبعدين اختبر تمام نفذ إلي بعده وهكذا"** — execute this step, test it, then the next one, and bring more ideas/developments too. This round is step 2 (refresh download address); step 3 is clipboard monitoring, step 4 download history. The step-2 plan was presented 2026-11-16 ~05:18 EET and approved by this order.
- **Feature:** right-click a paused/failed download → "Refresh download address" → dialog (current link, new-link field) → the engine re-probes the new link first: same file → switch URL and resume from the downloaded bytes; different file → mismatch warning, switch only on confirm (restarts from zero); probe failure → error, old URL + progress kept. New engine method `refresh_download_address` + store method `update_download_address` + Tauri command `refresh_download_address(id, url, force)` + `RefreshAddressResult` (Switched/Mismatch) + `RefreshAddressDialog.tsx` + context-menu entry (paused/failed only) + `architecture.md` command table. Reuses the existing probe + resume machinery; site login/proxy/cookies (same host only) and the excluded-sites refusal all apply as in `add_download`.
- **Tests:** 5 new engine tests (same-file resume, probe-failure keeps everything, mismatch changes nothing, confirmed mismatch restarts from zero, running/complete refused). **Caught during verification:** the forced-restart path forgot to recreate segments after the reset, so the restarted download never completed — fixed by adding `create_segments` from the new probe, the same step `resume_download` takes.
- Verified on Linux: full engine suite **422/422 pass, 0 failed** (417 + 5 new); `cargo check --tests` 0 warnings; `npm run build` passes. The run this push triggers is the proof — it must go green. Honest limit: the smoke screenshot shows the main window only; the dialog needs the user's own PC test to be seen in action.
---
## Round 51 — CI verdict (run #27, 2026-11-16)
- **Run #27 (43811435149, head 2d770731) FULLY GREEN: build-windows success, smoke-test success, extension-capture-test success.** Installer 15,561,674 bytes (~14.8 MB); smoke artifact 375,982 bytes; capture artifact 3,799 bytes. The parent fetches the screenshot from the smoke artifact. This note is local-only and rides with the next push.
---
## Round 52 — Clipboard monitoring (step 3 of the user's agreed sequence) — 2026-11-16
- **User approval (2026-11-16 ~08:02 EET):** tapped "تمام، نفّذ مراقبة الحافظة" on the plan presented 2026-11-16 ~07:49 EET — copy a download link → the app offers to download it; never auto-downloads; ignores non-links; dedupes; toggle in Settings to turn it off. Step 4 is download history (its plan is presented after this round).
- **Feature:** shell polls the system clipboard once a second (arboard, new dep in `app/src-tauri/Cargo.toml`) and hands the text to the engine; new `app/engine/src/clipboard.rs` (`CAPTURE_EXTENSIONS` = the same 27 extensions as the extension's `DEFAULT_CATCH_EXTENSIONS`; `extract_download_link` — single link only, http/https/ftp, file extension required) + `DownloadManager::check_clipboard_offer` / `dismiss_clipboard_offer` (toggle gate → extract → hash dedupe → seen dedupe → excluded-site refusal → `url_exists` refusal → offer). New Tauri command `dismiss_clipboard_offer(url)`; offer reaches the UI as a `clipboard-link-found` event; `ClipboardOfferDialog.tsx` (filename, URL, Download / Dismiss); App.tsx never stacks offers; Settings gains a "Watch the clipboard for download links" checkbox (General tab, default on). New `clipboard_monitoring` setting (store + schema.sql + migrate + `get_settings`/`update_settings`). The clipboard is never stored — only a hash of the last seen text plus the session offered/dismissed URL list; an unopenable clipboard disables monitoring for the session with one startup-log line, never a panic.
- **Tests:** 15 new engine tests in `app/engine/tests/clipboard_tests.rs` (detection incl. percent-decoded filename, ftp, uppercase extension, multi-line/over-long refusal; dedupe incl. re-copy after other text and dismiss; exclusion incl. subdomain; toggle off; already-downloaded URL; settings default-on + round-trip).
- Verified on Linux: full engine suite **437/437 pass, 0 failed** (422 + 15 new); `cargo check --tests` 0 warnings; `npm run build` passes. The run this push triggers is the proof — it must go green. Honest limit: the smoke screenshot shows the main window only; the offer dialog needs the user's own PC test to be seen in action.
---
## Round 52 — CI verdict (run #28, 2026-11-16)
- **Run #28 (43815862108, head e3b3b853) FULLY GREEN: build-windows success, smoke-test success, extension-capture-test success.** Installer 15,570,771 bytes (~14.8 MB); smoke artifact 375,182 bytes; capture artifact 3,804 bytes. The parent fetches the screenshot from the smoke artifact. This note is local-only and rides with the next push.
---
## Round 53 — Download history (step 4 of the user's agreed sequence) — 2026-11-16
- **User approval (2026-11-16 ~08:33 EET):** tapped "تمام، نفّذ سجل التحميلات" on the plan presented ~08:32 EET — History section in the sidebar recording every finished download (filename, size, date, URL, folder), surviving removal from the main table; search by name/URL; open folder / re-download from history; clear-history option. Last step of the agreed sequence.
- **Feature (engine half; frontend worker builds the sidebar section in parallel against the same contract):** new `download_history` table (`replica/schema.sql` + `Store::migrate`): `download_id UNIQUE`, deliberately NO foreign key so deleting a download never deletes history. Recording hook inside `Store::mark_complete` only (finished downloads only; row read with the already-locked connection, no deadlock); `record_history` (INSERT OR REPLACE, folder = parent of `save_path`), `list_history(query)` (filename OR url LIKE, `%`/`_` escaped, newest first), `get_history_entry`, `clear_history`; new `HistoryRow`. Manager: `list_history`, `clear_history`, `redownload_from_history` (real `add_download` path, final URL or URL, same filename/folder/category), `open_history_folder` (same `crate::queue::open_path` opener as `open_folder`). Tauri commands `list_download_history`, `clear_download_history`, `redownload_from_history`, `open_history_folder` registered in `generate_handler` (no capability change). `replica/features.csv` row 27 "Download history and search" flipped clone=no → yes.
- **Tests:** 8 new engine tests in `app/engine/tests/history_tests.rs` (real completion recorded with right fields; double recording keeps one row; delete keeps history; search by filename/URL + literal `%`/`_`; clear empties only history; re-download creates a new download that really completes; unknown id errors; open-folder errors when folder is gone).
- Verified on Linux: full engine suite **445/445 pass, 0 failed** (437 + 8 new); `cargo check --tests` 0 warnings. **CI run #29 FULLY GREEN 2026-11-16 ~07:05 EET (run 43819643263, head 0928b120): build-windows ✅, smoke-test ✅, extension-capture-test ✅; installer 15,616,779 bytes; smoke artifact 380,078 bytes.** Honest limit: the new Tauri commands are uncompiled on this VM (gdk-3.0, same as every round); the History sidebar section is the frontend worker's half and has never been seen in the running desktop app.
---
## Round 54 — Real E2E GUI tests (tauri-driver + WebDriverIO) — 2026-11-16
- **User order (2026-11-16 ~16:44 EET):** "try everything" — a comprehensive End-to-End test driving the real program on a real Windows server in GitHub Actions; no new version until it passes and logs + screenshots are inspected. Wine/Xvfb was ruled out first (WebView2 does not run under Wine; apt is broken on this VM).
- **Built (no product behaviour changed):** `scripts/e2e/e2e-test.mjs` (WebdriverIO remote() script, 11 steps, screenshot after every step into test-screenshots/ + results.json/results.md + startup.log + tauri-driver.log); `scripts/e2e/fixture-server.mjs` (20 MiB slow fixture, 32 KiB/s/conn, real Range + HEAD — verified locally with curl: HEAD 200 Content-Length 20971520, Range → 206 exact bytes, 128 KiB in 3.0 s); `scripts/e2e/package.json` + lockfile (webdriverio 10.2.0, own tree, app package.json untouched); `.github/workflows/e2e-windows.yml` (push + dispatch, windows-latest, debug build --no-bundle, tauri-driver --locked, msedgedriver-tool for the exact Edge-matched driver, uploads fetchline-e2e-screenshots always). The suite's steps 02/03/05/10 are designed reproductions of the 4 bugs from the user's run #29 PC test (stale auto-start, missing progress window, Stop no-op, empty Settings), asserted as correct behaviour — a failure is a finding, never a reason to weaken the test.
- Verified on Linux: `node --check` on both .mjs files; `npm run build` passes; full engine suite 445/445. The E2E suite itself cannot run on this VM (no WebView2) — CI is its first real run.
- **Workflow file NOT pushed via connector** (no Workflows permission): `.github/workflows/e2e-windows.yml` is written locally and must be uploaded via the GitHub web UI (xsonicx1998); the run it triggers is then watched and its screenshots artifact inspected. CI verdict: PENDING the browser upload.
- **CI verdict — E2E run #1 FAILED (run 43892347330, head 0ccfb124, 2026-11-16 ~17:09 EET, 6 min):** step 9 "Build debug app (no bundle)" failed after 2m46s; steps 10–13 skipped, so the suite never ran — no step results, no screenshots. Likely cause (harness setup, not a product bug): the e2e workflow never copies fetchline-host.exe into app/src-tauri/binaries/ (build-windows.yml does; tauri.conf.json externalBin requires it). Harness-only fix prepared in the local workflow file (copy lines added, mirroring build-windows.yml); re-upload via web UI needed, then re-run. Job logs unreadable via API (403 + rate-limited); NOT log-proven. No product code changed.
- **CI verdict — E2E run #2 FAILED (run 43894346292, head 9df2324, 2026-11-16 ~17:23 EET, ~8 min):** the host-copy fix WORKED — step 9 "Build debug app" now SUCCEEDED (3m13s), steps 1–12 all passed; step 13 "Run E2E GUI tests" FAILED after ~1 s, so the suite never ran: no per-scenario results, no screenshots, NO artifact (upload found nothing). The script died before main() ran (main() first creates test-screenshots/ + results.json, so a run reaching main() would leave an artifact) — most likely a load-time crash in CI, but NOT proven: both imports resolve fine locally. Job log unreadable via API (403 "Must have admin rights to Repository", confirmed 3×; plus rotating-egress rate limits). Next step: read the run #2 job log via the signed-in browser (xsonicx1998, user-authorised), fix the exact error, then run #3. Do NOT re-run blind. No product code changed; no product finding from run #2.
- **Run #2 cause FOUND, log-proven (parent read the job log via the signed-in browser, 2026-11-16 ~17:57 EET):** step 13 died on `TypeError: Cannot read properties of undefined (reading 'bind')` at `undici/lib/interceptor/decompress.js:78` — `createZstdDecompress.bind(...)` — on Node v20.20.2. Webdriverio's bundled undici needs Node's zlib zstd API, which exists only in Node 22+. Harness-dependency failure, not an app or test failure; the suite still never ran. **Fix (harness-only):** e2e workflow now sets up Node 22 (step renamed, comment records why); no `engines` field in scripts/e2e/package.json; webdriverio/undici versions unchanged; build-windows.yml untouched (still Node 20). Locally proven mechanism: same undici line 78 in the local tree; on this VM's Node v24.22.1 `createZstdDecompress` is a function and `import('webdriverio')` succeeds. No local Node 22 binary exists, so CI run #3 is the verification of the exact Node 22 setup. Workflow file must be re-uploaded via the web UI; run #3 is the proof. No product code changed.
- **CI verdict — E2E run #3 FAILED (run 43902745102, head d3ee9a0, 2026-11-16 ~18:08 EET, ~7m46s):** steps 1–12 ALL passed (incl. "Set up Node.js 22" and "Build debug app" 2m33s); step 13 "Run E2E GUI tests" FAILED after 1m11s (16:07:15 → 16:08:26 UTC). Progress vs run #2: the Node 22 fix WORKED — the undici import crash is gone; the script ran 71 s instead of dying in ~1 s. **An artifact EXISTS this time:** `fetchline-e2e-screenshots`, 1,720 bytes — too small for 11 screenshots, most likely results.json/results.md only. Contents NOT inspected yet (artifact download + job log are parent browser jobs; API logs 403 admin-gated). Per-scenario results UNKNOWN until the artifact is read; NO product finding from run #3 yet. Next: parent reads the log + fetches the artifact via the signed-in browser, views screenshots, reads results.json/results.md; fix the exact error, then run #4. Do NOT re-run blind. No product code changed.

---
## Round 55 — Fix round for the 4 run #29 live defects — 2026-11-16
- **User approval (2026-11-16 ~22:34 EET):** tapped "نفذ الخطة" on the 6-point plan presented 21:48 EET (timeouts + watchdogs, progress checkpointing, Add-dialog fixes, progress-window diagnostics, VPS retest). The content check the plan depended on never ran (RDP session died of inactivity twice; vault fill of the Windows password fails) — he chose execution over the check, and the check folds into the VPS retest. Investigation (Round 55, no code changed) found the likely causes; this round implements them.
- **Defect 1 — frozen progress (engine):** two layered causes. (a) Granularity: the store row was checkpointed only every 2 s while a 10 MB download finishes in ~1.3 s, so the store never learned mid-download progress. `CHECKPOINT_INTERVAL` 2 s → 1 s, and the first progress tick checkpoints at once (pre-aged clock), so a sub-2 s download reports at least once. (b) Hang: the reqwest client had only a connect timeout — a stalled body stream waited forever, leaving status `downloading` for minutes. `build_client` now carries a 60 s total timeout (`CLIENT_TOTAL_TIMEOUT`, `build_client_with_total_timeout` for tests); segments use a separate `build_segment_client` (connect timeout only — a slow download legitimately outlives any total budget, e.g. the 20 MiB fixture at 32 KiB/s), plus a per-chunk stall watchdog (30 s, `SEGMENT_STALL_TIMEOUT`) on `send()` and every `stream.next()` in `download_segment_once`, plus a run watchdog (180 s of zero progress, suppressed during retry backoff) in `run_inner` that aborts the run and fails it with "No data arrived for N seconds — the connection appears to have stalled". `try_split` never splits open-ended segments (mid ≈ 2^63 hazard) and validates the midpoint is in-range before `split_segment`. `Store::reconcile_stuck_downloads` resets rows left `downloading` to `paused` (bytes kept) — called once in the shell's `build_manager` and logged to startup.log — so no zombie row survives a restart.
- **Defect 4 — Resume no-op (engine + UI):** explained by defect 1 — a wedged run leaves status `downloading`, and `resume_download` refused it as "already running". Resume now recovers a stale `downloading` row (no run in the `running` map: reset to paused and resume); a genuinely running download still refuses. And Resume/Pause errors are now visible: the 1 s poll used to clear the error banner on every successful refresh, so an error flashed for under a second and vanished. App.tsx now has a sticky `bannerError` (cleared only by dismiss or a later successful action; the poll never touches it) fed by `run()` and the other user-initiated handlers.
- **Defect 3a/3b — Add dialog silent failure + 1 GB hang (UI + engine):** the new row was hidden by the sidebar category filter (looked exactly like a silent failure) — after a successful add App.tsx now switches to "All downloads" and selects the new row. Escape / backdrop / X / Cancel no longer close the dialog while a submit is in flight. The submit has a 60 s client-side timeout (matching the probe's total timeout) with a clear error, and the submit promise outlives the dialog: a late success adds the row, a late failure lands in the main error banner via a new `onLateError` prop (a failure after some submit already succeeded is not reported — the download exists). The 1 GB hang itself was a probe stall (no total timeout) — fixed by the 60 s total timeout on the probe client.
- **Defect 2 — blank progress window (diagnostics first, no blind render change):** the code path looks correct (window created with `index.html#progress=<id>`, component always renders at least "Loading download…"), so the WebView never painted a frame — root cause unproven. Diagnostics: `open_progress_window` now logs creation / focus / failure + URL to startup.log and enables devtools on the progress window only (verified `WebviewWindowBuilder::devtools` exists in vendored tauri 2.12.5); a static loading shell in `app/index.html` (`#boot-shell`, dark, inline styles) so first paint never depends on JS; `main.tsx` replaces the shell with a plain-DOM error if React fails to mount within 2 s; `ProgressWindow` now distinguishes a poll failure ("Could not load this download" + the error) from "Download removed". The stale BUG-044 comment in lib.rs (claiming no capability files ship) is fixed — `capabilities/default.json` exists since Round 40 and grants `core:event:allow-listen/unlisten` to window `main` only. The shell edit is in `app/index.html` (source); `dist/index.html` is regenerated by the build and carries the shell (verified).
- **Tests:** 8 new engine tests in `app/engine/tests/round55_tests.rs` (total timeout const 60 s + a 1 s client failing a stalling server; stall watchdog failing a wedged download with a "stalled" error; checkpoint showing bytes ~1.4 s into a running download; try_split never splitting an open-ended segment — exactly 1 segment row after completion; reconciliation resetting a zombie row and leaving others alone; resume recovering a stale `downloading` row to completion; resume still refusing a genuinely running download) + a `stall_get` mode on the shared mock (`tests/common/mock.rs`: GET sends headers, then nothing for 2 min). The Add-dialog busy-guard is UI-only — verified by `npm run build` (tsc), no engine test possible.
- Verified on Linux: full engine suite **453/453 pass, 0 failed** (445 + 8 new); `cargo test --release` ~7 min; `npm run build` passes (tsc + vite, dist carries the shell). The run this push triggers is the proof — it must go green. Honest limits: the Tauri shell never compiles on this VM (gdk-3.0), so `lib.rs` (reconciliation call, progress-window logging + devtools, stale comment) is verified by reading only — CI is its first compile; the progress window's root cause is STILL unproven and needs the live VPS test with devtools + startup.log; the run watchdog (180 s backstop) has no dedicated test — the per-chunk watchdog test covers the shared failure path.

## Round 56 — Fix round for the r55 live defects (invoke path wedged after completion, both webviews blank) — 2026-11-17
- **User approval (2026-11-17 ~02:48 EET):** tapped "نفذ خطة الإصلاح" on the §7 plan in `~/workspace/round56-investigation-rev.md`. The r55 VPS test proved the engine completes downloads (DB row complete 23:29:41) but the UI froze at 0% and both webviews went blank white; server evidence overturned the first diagnosis (no store deadlock, no run panic — `mark_complete` ran, the checkpoint wrote), and the wedge mechanism itself was NOT found in code, so the plan leads with observability that does not depend on the runtime. Step-0 live checks mostly failed to run (02:50 EET — the browser agent could not drive the RDP session; main window a blank white rectangle that does not respond to maximize; DevTools never opened; app deliberately NOT restarted, wedged state preserved, PID 5860). This round implements the code-alone parts only (§7.1, §7.2, §7.3, §7.5, §7.4 heartbeat-detection mechanism). The targeted runtime-wedge and webview-blank trigger fixes wait for the post-rebuild retest, where the §7.4 A/B test (plain index.html) is the primary discriminator.
- **§7.1 Observability (engine + shell):** `Store` gained a diagnostics sink (`log_path` + `set_log_path` + `log`, best-effort, `[epoch] message` lines in the startup.log format) — the shell points it at startup.log in `build_manager`, which now also logs "reconciled 0 download(s)" on Ok(0) instead of silence. All 67 `self.conn.lock().unwrap()` sites now go through `Store::lock_conn` (`#[track_caller]`, logs a > 1 s wait with its call site; the helper's own body is the one place that still touches `self.conn` directly). `probe_url` logs duration + outcome; `checkpoint` logs a failed row/segment write instead of swallowing it with `let _ =`. Shell: a plain `std::thread` heartbeat ("heartbeat: startup" + "heartbeat: alive" every 30 s); `main()` restructured to build + `app.run()` + a "clean shutdown" line after it returns (replacing "run returned ok" — a log ending without it means the process died); `list_downloads` sampled entry/exit trace (every 100th call, or any call slower than 1 s) and `add_download` every call, via `AppHandle`-resolved log path; `open_progress_window` logs BEFORE `build()` and logs the window's resolved `url()` after creation.
- **§7.2 Independent run watchdog (engine):** `Running` carries `last_progress_ms: Arc<AtomicU64>` (stamped by the supervisor loop whenever the downloaded total grows, initialised at run start); a separate watchdog task per run reads the CURRENT map entry every 1–10 s (a sixth of the timeout, clamped) and aborts after `run_watchdog_timeout` (default 180 s, `RUN_STALL_TIMEOUT`) of no progress, suppressed during retry backoff. The abort removes the map entry first (unblocks Resume/start), then aborts the handle, clears the retry wait, marks the row error with "No data arrived for Ns seconds — the connection appears to have stalled", emits the snapshot and drops the cookies — mirroring `cancel_download` so a later Resume restarts the download. If the row is already complete/paused/errored only the stale entry is dropped. No sound is played (it would read the store). Timeout is a test knob (`set_run_watchdog_timeout_secs`, clamped >= 1 s). The 30 s per-chunk watchdog and the in-loop 180 s watchdog are untouched.
- **§7.3 Frontend in-flight guard (App.tsx, ProgressWindow.tsx, api.ts):** new `withTimeout` helper (Promise.race with a timer; the hung promise is left pending, its result ignored). The 1 s / 500 ms polls skip a tick while a refresh is in flight and wrap the poll invoke in a 5 s timeout, so a hung invoke REJECTS. After ~10 s with no successful poll the window stops issuing invokes and shows a sticky "Backend not responding — restart the app" banner with Retry connection (resumes polling; the interval was never destroyed, its ticks no-op while stopped) and Reload window; a successful poll clears the dead state. The progress window gets the same guard plus the banner in both its renders (main view and the "Could not load" empty state).
- **§7.4 Heartbeat-detection mechanism (shell + frontend):** `AppState` gained `webview_heartbeats: Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>`; `main.tsx` pings `webview_heartbeat` every 5 s with the window label (`main` / `progress-<id>`, in-flight-guarded, errors swallowed, module level so it runs even if React fails to mount); a runtime-independent monitor thread in `setup` logs "webview <label> heartbeat started / lost (last seen Ns ago) / resumed", once per outage. New `reload_webview_window` command: main reloads in place via `eval("window.location.reload()")`, a progress window is closed and rebuilt via the extracted `build_progress_window` helper (shared with `open_progress_window`). The main window's resolved URL is logged in `setup`, like the progress window's. Devtools enabled on the MAIN window too (`tauri.conf.json` `"devtools": true`) — verified against vendored Tauri 2.12.5 + wry 0.57.2: the builder flag is not cfg-gated and maps to WebView2 `AreDevToolsEnabled`, so F12 works in a release build (the `devtools` cargo feature only gates the `open_devtools()` API method).
- **§7.5 Side bugs + probe timeout:** Add dialog "Save to folder" placeholder is now the configured default folder (new `defaultFolder` prop, fed from `settings.default_download_folder` in App.tsx) or "Leave empty for Downloads" when none is configured — the made-up `C:\Users\you\Downloads` is gone. Speed-cap input is `type="text"` + `inputMode="numeric"` with select-all on focus and non-digits stripped (a number input ignores Ctrl+A, so typing appended → "0512"). Probe total timeout `CLIENT_TOTAL_TIMEOUT` 60 s → 30 s (the dialog's 60 s client-side submit timeout is unchanged); the Round 55 const test is updated to pin 30 s.
- **Tests:** 4 new engine tests in `app/engine/tests/round56_tests.rs` (independent watchdog aborts a stalled run in ~2 s with its message; a slow 4 MiB download at ~50 ms per 8 KiB chunk completes with the watchdog at 2 s; timeout defaults to 180 s and clamps to >= 1 s; probe duration logged with its URL). Full engine suite **457/457 pass, 0 failed** (453 + 4 new; 22 binaries, debug profile, ~3 min; the "0 passed" binary is the env-gated real-download test); `npm run build` passes (tsc + vite). Honest limits: the Tauri shell never compiles on this VM (gdk-3.0), so `lib.rs` is verified by careful reading + `rustfmt` parse only, with every Tauri/wry API used (`WebviewWindow::url`, `eval`, `close`, `set_focus`, `WebviewWindowBuilder::devtools`, `App::run`) checked against vendored Tauri 2.12.5 — the VPS build is its first compile. Deliberately NOT in this round: the targeted runtime-wedge fix and the webview-blank trigger fix (plain-index.html A/B test, fragment removal) — those wait for the step-0 / retest evidence. No pushes, no GitHub/CI, no workflow files touched.

## Round 57 — Fix round for the r56 live defects (7–8% stall, blank webviews) — 2026-11-17
- **User approval (2026-11-17 ~12:19 EET):** tapped "نفذ خطة الجولة 57" on the §9 plan in `~/workspace/round57-investigation.md` (presented 12:03 EET; investigation + live checks ran in parallel — the live checks' DB/disk result decides the §9.5 engine-stall fix, which is NOT in this round). The r56 VPS retest (07:00 EET) failed all 4 checks in the UI and the log showed "webview main heartbeat lost" twice with no watchdog line at all. The investigation corrected two things told to the user before ("ALL 4 FAILED" is a UI verdict, not an engine verdict — DB/disk never checked; "renderer death PROVEN in the log" is overstated — the ping is an invoke, so it also stops on a runtime wedge) and found the reload command used the wrong primitive (`eval("window.location.reload()")` needs live JS + IPC) and the 180 s watchdog is a tokio task on the same runtime it watches.
- **§9.1 Renderer recovery (shell + frontend):** `reload_webview_window` now uses native `WebviewWindow::reload()` (Tauri 2.12.5 → wry `ICoreWebView2::Reload()`) for main, close + rebuild for progress windows, via the shared `reload_webview_now` helper. New `maybe_auto_reload`: the heartbeat monitor thread auto-reloads a window whose pings stop for 30 s — once per outage, max one auto-reload per 5 min per window, every decision logged (lock released before the reload runs, since the reload takes the same lock for its cooldown). `ProcessFailed` registered directly on every webview via wry `WebViewExtWindows::webview()` + `webview2-com 0.39` (`ProcessFailedEventHandler::create` + `add_ProcessFailed`, mirroring wry's own `PermissionRequested` registration — binding VERIFIED from the crate's published docs on docs.rs + vendored wry 0.57.2, not skipped; new Windows-only deps `wry = "0.57"` + `webview2-com = "0.39"` in Cargo.toml, both already in Cargo.lock so no new download on the VPS build, `cargo metadata --offline --locked` passes). On the event: log `webview <label> renderer process failed (<kind>)` + the same cooldown-gated reload. Heartbeat entries registered at window creation (main in `setup`, progress in `build_progress_window`) so a window whose JS never runs is watched from the start. Ping flaw fixed in `main.tsx`: the ping races a 5 s timeout so `pingInFlight` always clears.
- **§9.2 Runtime-wedge hardening (engine + shell + frontend):** clipboard read on `spawn_blocking` behind `Arc<StdMutex<Clipboard>>` with `try_lock` (a stuck RDP clipboard channel skips the tick) + `MissedTickBehavior::Delay` (arboard's own docs warn the Windows clipboard is a global object opened on one thread at a time — parallel operations risk error/deadlock, so one-at-a-time + skip-if-busy is the right shape; arboard asserts `Clipboard: Send + Sync` in its own tests). Supervisor `checkpoint()` store calls on `spawn_blocking` (in-memory snapshot first, then one blocking write). Tauri runtime built with 4 workers via `tauri::async_runtime::set` before the Builder (mitigation only, commented as such; the Runtime is leaked deliberately — `set` keeps only the handle, dropping it would shut the runtime down). `refreshAll` complete: `refreshQueues`, `refreshGrabberProjects` and the new `refreshQueueFiles` all have the 5 s `withTimeout` + a shared in-flight guard. `list_downloads` logs its entry (sampled, same as the exit) — a hung call leaves an entry-without-exit trace.
- **§9.2f Truly-independent watchdog (engine):** `Running.handle` is now `Arc<JoinHandle<()>>`, shared with a new `ThreadWatchEntry` per run in `DownloadManager::thread_watchdogs`. A plain `std::thread` per run watches the same last-progress timestamp (1–10 s cadence, retry-backoff suppression) and on a stall logs FIRST ("thread watchdog aborted … (tokio watchdog did not fire)"), then aborts the run and marks the row error, mirroring the tokio watchdog — never restarts the process. The thread exits when its registry entry is removed (run ended, cancelled, tokio watchdog fired — all three removal sites added) or replaced (newer run — `Arc::ptr_eq`, re-checked immediately before acting). No progress event is emitted: `emit_snapshot` is async and needs the wedged runtime — the DB status is the source of truth.
- **§9.3 Progress-window A/B test (shell + frontend):** the progress window loads plain `index.html` with the id via `WebviewWindowBuilder::initialization_script` (`window.__PROGRESS_ID__`); `main.tsx` reads it, falling back to the `#progress=<id>` fragment. The fragment path stays working.
- **§9.4 Diagnostics (engine):** the in-loop watchdog logs "in-loop watchdog fired …" explicitly before returning its error.
- **Tests:** 2 new engine tests in `app/engine/tests/round57_tests.rs` (thread watchdog aborts a stalled run while the tokio runtime is wedged — 2-worker runtime, both workers blocked by sync sleeps, log shows the THREAD watchdog fired and the tokio one did not; slow 4 MiB download completes with no watchdog line at all). Full engine suite **459/459 pass, 0 failed** (457 + 2 new; debug profile, ~4.5 min); `npm run build` passes (tsc + vite, 5.66 s). Honest limits: the Tauri shell never compiles on this VM (gdk-3.0), so `lib.rs` is verified by careful reading + `rustfmt` parse only, with every Tauri/wry/webview2-com API used (`WebviewWindow::reload`, `with_webview`, `WebviewWindowBuilder::initialization_script`, `wry::WebViewExtWindows::webview`, `ProcessFailedEventHandler::create`, `add_ProcessFailed`, `COREWEBVIEW2_PROCESS_FAILED_KIND` + its free constants, `tauri::async_runtime::set`) checked against vendored Tauri 2.12.5 / wry 0.57.2 / docs.rs — the VPS build is its first compile. Deliberately NOT in this round: the §9.5 engine-stall fix (depends on the live DB/disk check results — do not write it blind) and auto-restart (pending user decision — watchdogs log + abort only). No pushes, no GitHub/CI, no workflow files touched.

## Round 58 — Fix round for the r57 live defects (invoke wedge persists, blank progress window, Start dead / 1 GB add 60 s timeout) — 2026-11-17

- **Investigation (read-only, `~/workspace/round58-investigation.md`):** diagnosis moves from a tokio-runtime wedge to a main-thread wedge in WebView2 window creation — every progress window re-creates the whole environment from scratch on the main thread with an unbounded `wait_with_pump`; if the browser process fails to start a controller, the main thread never returns, no invoke is dispatched/answered, the window can't close. Fits all six r57 retest observations. **Correction:** the investigation's §5.3 explanation of the Start-vs-Save asymmetry (a hung probe invoke disabling Start via `probeBusy`) does NOT apply — `probeBusy` / `getDownloadInfo` do not exist in this codebase; both buttons share the `busy` flag and the same `add_download` invoke. The asymmetry is the intermittent wedge's timing; the §8 log checks (`add_download` entry/exit) decide the 1 GB story.
- **Fix (§9.1–§9.5):** progress windows reuse the main window's WebView2 environment (`WebviewWindowBuilder::with_environment`, verified public API in tauri 2.12.5; env via main window `with_webview` → `PlatformWebview::environment()`, 5 s bounded `recv_timeout`, fallback logged); auto-reload guarded (no rebuild of never-created windows via `created_ok`, cap 2 attempts then `auto-reload abandoned`, rebuild on a detached `std::thread`); `window.url()` getter deleted from `build_progress_window` (builder's URL logged + `build()` elapsed time, >5 s slow warning); main-thread watchdog (`std::thread`, `is_visible()` round-trip, logs `main thread stalled for 10s` once per outage + resume); stale comment fixed (dialog 60 s deliberately longer than the engine's 30 s probe bound). Not in this round: engine-stall fix, auto-restart, probe change, worker increase. `npm run build` passes; `cargo fmt` clean in the edited regions. Tauri shell never compiles on this VM — VPS build is the first compile.
- **User approval (2026-11-17 ~14:18 EET):** tapped "نفذ خطة الجولة 58" on the plan presented 14:17 EET. Live checks (disk cleanup/enlargement, startup.log §8 decisive checks, DB state) were already running in parallel; their results fold into the retest.

- **E0277 fix (2026-11-17 ~15:46 EET, same round):** the first VPS build failed with exactly one error — `NonNull<c_void>` cannot be sent between threads safely (lib.rs:614): `main_webview_environment` sent the environment through `mpsc::Sender<ICoreWebView2Environment>`, but `with_webview` requires a `Send` closure. Fix: a Windows-only newtype `SendableWebView2Environment` with `unsafe impl Send`; the channel now carries the wrapper. Safety (documented in code, no agility claimed): move-only use — the worker never calls a method on it; the only COM call runs on the main thread; clone/drop is only `AddRef`/`Release`. Same transfer Tauri itself performs (`WebviewAttributes` is `unsafe impl Send`, tauri-runtime-2.12.2/src/webview.rs:434). 5 s `recv_timeout` unchanged. `npm run build` passes; `cargo fmt` clean in the edited region. VPS build is the real compile check.

- **Round 58 retest (2026-11-17 16:48 EET, build a26a1027, exe 17,873,920 bytes):** BUILD OK (3m 59s). (a) Progress window ×10 FAIL — still blank white, will not close, red banner after each creation. (b) 20 MB via Start PASS on a healthy instance. (c) Slow Stop/Resume FAIL — froze at exactly 2,097,152 bytes (2 MiB), UI 0%, Resume did not move it. (d) 1 GB PASS by criterion — probe error in ~15 s, no 60 s timeout. Log: progress heartbeat STARTED then LOST (33s); main auto-reload done; progress auto-reload skipped. NO "main thread stalled" or "build took slow" lines — main-thread wedge now weakened. NEW: slow-a.mp4 listed as Finished with only 2 MiB on disk (false Finished). Round 59 investigation proposed, awaiting approval.

- **Round 59 (2026-11-17 ~17:02–18:06 EET):** investigation only, no build. Report `~/workspace/round59-investigation.md`. Live checks (17:44 EET) overturned the diagnosis: full-log grep "created:" = 0, "main thread stalled" = 0, "PANIC" = 0 — main-thread wedge RULED OUT, all four §8 branches ruled out, §9.2–§9.4 suspended. Corrections: slow-a.mp4 never froze at 2 MiB (complete 20,971,520/20,971,520); 1 GB = real VPS DNS failure. §11 follow-up (18:06 EET) narrowed the mechanism to a lost CreateWindow message (by elimination, unproven) and set the revised plan §11.4: instrument first, fix second. Incident: report overwritten while appending §11, rebuilt condensed (532 vs 922 lines); §11 intact.
- **Round 60 implementation (2026-11-17 18:24 EET, logging only, lib.rs):** §11.4 items 1–3 — "heartbeat monitor started" + "main thread watchdog started" lines after spawn; "watchdog tick" every 60 s (tick_count % 12 == 1, logged before the main-window lookup); "progress window {label}: build() entered on thread {thread_id:?}" before `builder.build()`; thread id added to `created:` and `FAILED` lines; "watchdog probe failed: {e}" when `is_visible()` returns Err (10 s recv_timeout unchanged). Engine suite 459/459; npm build passes (20.52 s); cargo fmt clean in edited regions (41 pre-existing hunks elsewhere, untouched). App crate cannot compile on this VM (no gdk-3.0) — VPS build is the real compile check. Push pending → VPS build + instrumented test run (§11.4 items 4–6 as live-check procedure).

- **Round 60 build + instrumented test (2026-11-17 18:26–19:24 EET, task 106d3b65):** BUILD SUCCESS — git pull a26a102..f2d1d4b verified; VS BuildTools v17.14.45, cl.exe 19.44.35230 x64, cargo 1.100.0; host 1m 40s (6,310,912 bytes); Tauri 3m 45s; exe 17,903,104 bytes, 11/17/2026 4:43:57 PM; log C:\fetchline-r55\build-r60.log. Fixture server PID 5980 on 127.0.0.1:55140. New log lines present; full-log greps decide mechanism (b) CONFIRMED (watchdog started ×3, ticks ×35, build() entered ×3, no stall/FAILED/renderer-failed). Blank window reproduced; X + system-menu Close fail; Stop in-flight did not stop (dead invoke channel). Cloudflare 100MB add unconfirmed (session expired). App (PID 7692) left running, 12 downloads. RDP session needs renewal for follow-up.

- **Round 61 implementation (2026-11-17 19:34–19:42 EET):** async fix + UI bugs, approved 19:33 EET. lib.rs: `open_progress_window` (L810) and `reload_webview_window` (L904) → `async fn` (keyword only); setup log line `main thread id: {:?}` (L1648–1653). ProgressWindow.tsx: `import { getCurrentWindow } from '@tauri-apps/api/window'` (L2) + all 5 `window.close()` → `getCurrentWindow().close()` (L418, L433, L531, L605, L964); 0 `window.close()` remain. capabilities/default.json: windows `["main", "progress-*"]`, added `core:window:allow-close`. Rust-call-site check: neither command called from Rust (only `generate_handler!` L2016/L2020 + JS `invoke` via api.ts). Engine suite 459/459 (first run hit DiskFull in quota_tests — /tmp 512M tmpfs was 100% full with leftover /tmp/fetchline-test-* dirs; cleaned per standing rule, full re-run green, /tmp cleaned again). npm build passes (4.71 s). cargo fmt: 41 pre-existing hunks, none in edited regions (new log line rewrapped to rustfmt form). App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile. Push pending → VPS build + retest.

- **Round 61 build + retest (2026-11-17 19:43–21:14 EET, task 4360d730, 1h 30m):** BUILD SUCCESS — git pull to 6358fe54 verified; VS BuildTools; host 1,192,960 bytes 6:10 PM; Tauri exe 18,780,672 bytes 6:16 PM; log C:\fetchline-r55\build-r61.log (14,876 bytes). Fixture server PID 5980 on 127.0.0.1:55140. (a) Progress window PASS (5+ openings, real content, closes via X / in-page Close / glyph; twice needed a repeat click — first click only focused). (b) Fixture 20 MB PASS (live progress to 100%). (c) Stop/Resume PASS (Stop ~16% → Paused 35%, Resume → 100% full bytes). (d) Logs PASS (created: 5, FAILED 0, stalled 0, renderer-failed 0, ticks 108, build() entered 8, main thread id ThreadId(1) vs build() on ThreadId(3/5/4)). (e) Large download FAIL (Cloudflare 404/403 — hosts moved; OVH 75 MB then connection lost + Failed while curl HEAD 200; Tele2 address check failed 3×; failed row has no Resume/Retry in UI). Extra feature pass not run (stopped per instruction). App left RUNNING (PID 6100), 14 downloads (13 finished, 1 failed).

## Round 62 — Settled redesign implementation (progress window + main window + new icon) — 2026-11-17

- **User approval (2026-11-17 ~23:07 EET):** tapped "نفذ التصميم" after the design settled over ~22:13–23:06 EET: progress window per his AI mockup (tabs + parts table + Options tab), main window = the CLEAN first mockup (not the IDM-heavy variant), new logo = the image he sent (Sonic quills + download arrow, blue→pink gradient). Implementation only — no push, no VPS work; the VPS build + live test is the next step.
- **Progress window (ProgressWindow.tsx, rewritten):** frameless custom title bar (TitleBar.tsx, shared) showing live "<percent>% - <filename>"; glowing file icon + filename + Downloading badge + URL + "Receiving data…"; big gradient %; gradient progress bar; stats row (Downloaded / Speed / Time left / Resume); tabs "Download status" + ONE merged "Options" tab (speed limiter + "Close program after download" + "Shut down computer after download"); IDM-style parts TABLE (N. | Downloaded | Info), 5 rows visible + scroll, part count dynamic (engine 1–16). Per-part vocabulary derived from the engine's two segment states (done/active) — documented in code: Done (cyan), "No" (red bold, not running), "Disconnect" (pink, retry backoff), "Send GET..." (amber, running + 0 bytes), "Receiving data..." (purple, running + bytes flowing). REMOVED: 60 s speed graph, Hide details toggle + `set_progress_window_compact` command, completion tab (queue editor), all scan UI (open file/folder, scan again, quarantine, delete) — those live in the main window now. Folder path shown on the status tab. Pause + Cancel footer (Resume on paused/error, Close on complete). Shutdown = 30 s countdown modal with Cancel + "Shut down now"; shutdown wins when both boxes are checked (documented in the tab). Completion fires only on a transition into 'complete' observed while the window is open; options persist in Rust AppState (in-memory) via new commands and are read back on mount. Backend-dead banner, retry countdown, quota note, error message kept; 500 ms poll kept.
- **Frameless windows (lib.rs, tauri.conf.json, capabilities):** progress builder `.decorations(false)`, inner 660×480 (min 560×380); main window `"decorations": false` in tauri.conf.json. TitleBar: drag region is the icon+title area only (buttons sit outside it so they stay clickable), double-click toggles maximize, maximize glyph follows live `isMaximized()` + `onResized`. capabilities/default.json adds `core:window:allow-minimize/maximize/unmaximize/toggle-maximize/is-maximized/start-dragging/set-size/set-focus` for ["main", "progress-*"] (verified against vendored Tauri 2.12.5 permission reference). Round 61 constraints KEPT: `open_progress_window` and `reload_webview_window` stay `async fn`; ProgressWindow uses `getCurrentWindow().close()` only (0 `window.close()`); capabilities still grant ["main", "progress-*"] + allow-close.
- **New Rust commands (lib.rs + generate_handler):** `set/get_download_completion_options` (in-memory `AppState::download_completion` map), `exit_app` (`AppHandle::exit(0)` — verified in vendored tauri 2.12.5 app.rs:581), `shutdown_computer` (Windows `shutdown /s /t 0`, same call as the engine's queue "shutdown" action; error elsewhere), `get_free_disk_space` (Windows `GetDiskFreeSpaceExW` via a small kernel32 extern block, no extra crate; error elsewhere). `set_progress_window_compact` deleted (command + handler + api wrapper). Unused `LogicalSize`/`Size` imports removed.
- **Main window (App.tsx, Toolbar.tsx, DownloadTable.tsx, new MenuBar.tsx + ToolsStrip.tsx):** TitleBar ("Sonic Manager" + new logo) + menu bar with all 6 menus (File, Tasks, View, Downloads, Options, Help; no Registration, no "Tell a Friend"; About dialog is local) + slim toolbar (gradient + Add URL, Resume, Pause, Stop, Delete, search box filtering by filename) + TOOLS strip (Queue, Scheduler, History, Video Grabber, Site Grabber, Refresh Address, Clipboard with ON/OFF toggle flipping the real setting). NO settings gear anywhere (Settings via Options menu). Table: 5 columns (File Name, Size, Status, Transfer rate, Time left) with inline gradient progress bars under downloading/paused rows + status pills; Q/Last Try/Description columns removed (error/quota/threat text survives as the status cell's tooltip). Status bar: "N downloading • total speed • M finished" + quota (when enabled) + drive free space ("C: 21.2 GB free", polled every 30 s from the default download folder). CategorySidebar kept as-is (it already renders All/Unfinished/Finished/Queues + the real categories — the mockup's "Types" section is the builtin categories; hard-coding 4 types would have removed user categories, grabber projects and queue navigation). Toolbar "Stop" pauses the selected download (IDM's "Stop" is a pause; cancel deletes the row, so it is not a Stop) — same engine call as "Pause".
- **Icon (C):** chosen logo copied to `app/src-tauri/icons/logo.png` (new source) + `app/public/icon.png` (title bars, bundled by Vite — verified in dist). `scripts/generate-icons.py` updated to prefer `logo.png`, falling back to the SVG mark then logo.jpg. Ran the script (icon.png 1024 + extension icons) then `npx --yes @tauri-apps/cli@2 icon` (32x32, 128x128, 128x128@2x, icon.ico regenerated; 128x128 verified by eye — quills + arrow, blue→pink). Removed the command's iOS/Android/Store extras (ios/, android/, Square*.png, StoreLogo.png, icon.icns, 64x64.png) — not in the repo's icon set.
- **Tests:** engine suite **459/459 pass, 0 failed** (release profile, /tmp cleaned before/after per standing rule); `npm run build` passes (tsc + vite, 4.10 s; new UI strings verified in the built bundle; dist/icon.png present). `cargo fmt --check`: 40 pre-existing hunks, none in edited regions (one of mine fixed — the joined `use tauri::{...}` import). App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile; resize-from-edges + rounded corners + the frameless title bar behavior are verified on the VPS, not here. No push, no GitHub/CI, no VPS work in this round.

## Round 63 — User feedback fixes on the real Round 62 app (progress window + main window) — 2026-11-18

- **User feedback (2026-11-18 ~00:32 EET, saw the real Round 62 app on the VPS via RDP):** 6 fixes, implemented local only, no push, no VPS work. (1) Progress-window tabs moved to the TOP, directly under the custom title bar (pinned above the scrollable content, like his AI mockup) — they were below the stats row. (2) Footer Cancel button DELETED — Pause is the only action while downloading (Resume on paused/error); the title-bar X still closes the window. (3) On completion the footer shows Open file (primary) + Open folder (secondary) — reuses the existing `open_file`/`open_folder` engine commands (no new Rust code); failures surface as the window's action error. (4) Old neon-waves background REMOVED — `background.css` import deleted from main.tsx, `background.css` + `assets/backgrounds/sonic-neon-waves.svg` deleted; body is flat dark #0B0F24 (`--color-bg` in index.css) in both windows; stale App.tsx comment updated. (The pre-Round-45 `background.jpg` asset is unreferenced and was left in place.) (5) Theme scrollbars in index.css — purple #8B20FF thumb on dark track, cyan on hover, thin; `scrollbar-color`/`scrollbar-width` for Firefox + `::-webkit-scrollbar` for Chromium/Edge; applies everywhere scrollable (main table, parts table, sidebar). (6) History REMOVED from the TOOLS strip (ToolsStrip.tsx `onHistory` prop + button + App.tsx call site) — History stays in the sidebar (and in the menu). Matched against the two agreed-design pictures the user sent (00:38 EET): parts header now reads "Parts (N connections)" with an "X/N completed" counter in accent, Downloaded stat reads "downloaded / size" as in the picture, and the folder path moved into the footer row (left, buttons right). Round 61 constraints kept (async open/reload, getCurrentWindow().close(), 0 window.close()). Engine suite 459/459; npm build passes (4.06 s after the picture-matching edits). No Rust files edited — cargo fmt pre-existing hunks only. App crate cannot compile on this VM — VPS build is the first real compile. Push pending → VPS build + retest (the old RDP test task tests the pre-fix build and must be stopped first).
- **Decisions to flag:** (a) the retry-box "Cancel download" ghost button (aborts a stuck retry loop) was KEPT — it is a different action from the removed footer Cancel; remove it too if the user wants no Cancel anywhere. (b) The parts-table note ("N parts — the count depends on what the server supports, up to 16") was removed to match the agreed picture; the dynamic count now shows only in the "Parts (N connections)" header. (c) The TOOLS strip buttons stay single-line pills (the agreed picture shows two-line labels in the mockup render); no visual change was made to them beyond removing History.

## Round 63b — Cancel restored in the progress window footer — 2026-11-18

- **User decision (2026-11-18 ~01:29 EET):** "بقولك هنرجع cancel تآني" — Cancel is coming BACK. The Round 63 removal is reversed: the footer Cancel button is restored exactly as in Round 62 (commit d0c42dc1) — `variant="danger"`, `onClick={handleCancel}` (the existing `cancelDownload(downloadId)` call, same handler the retry-box ghost button uses), shown while `status === 'downloading' || status === 'paused'`, placed after Pause/Resume and before Close. It does NOT show on completion (Open file / Open folder replace Pause there, per Round 63). Header comment in ProgressWindow.tsx updated to match. Retry-box "Cancel download" ghost button unchanged. No other file touched; all other Round 63 fixes stand (tabs on top, flat dark background, theme scrollbars, no History in the TOOLS strip). Engine suite 459/459; npm build passes. No Rust edited. NOT pushed; VPS build + retest pending.
- **Round 63 build + retest (2026-11-18 00:46–01:35 EET, task c4566d1c, 48 min):** BUILD SUCCESS — git pull aca88eef..92a88a0 verified; icons (generate-icons.py at repo-root scripts/, quoted "@tauri-apps/cli@2" in PowerShell); host 1m 54s (6,310,912 bytes; sidecar renamed correctly after a wrong-name copy); Tauri 4m 10s (frontend 6.38 s); exe 18,332,672 bytes; log C:\fetchline-r55\build-r63.log. Main window PASS (new logo, no History in strip, flat dark, purple scrollbars). Progress window NOT VERIFIED — never opened (double-click/right-click/status-bar/menus). OVH 100Mb.dat stalled at 94% ~12 min, Pause no effect; Add URL Start dead; app unresponsive, X won't close. App left RUNNING hung. Next: Round 64 investigation, then retest on 63b build.
- **Round 63b build + retest (2026-11-18 01:56–02:34 EET, task 8529c233, 37 min):** BUILD SUCCESS — git pull to 3d4c4a9 verified; old hung app killed (Stop-Process -Force), old exe deleted; host 6,310,912 bytes; exe 18,332,072 bytes; log C:\fetchline-r55\build-r63b.log. Main window PASS. Progress window PASS — opened with real content (tabs on top, Pause + Cancel, parts table 4 rows, stats + path). Fixture slow-a.mp4 20.0 MB COMPLETED (Pause 74% → Resume → 100%, 4/4 Done); app responsive throughout, hang did NOT reproduce. User links NOT tested — both testfile.org links 403 Forbidden (Cloudflare, verified via curl) — site-side block. App left RUNNING (16 finished, 0 downloading).

## Round 64 — Steady Speed / Time-left + footer Close removed — 2026-11-18

- **User feedback (2026-11-18 ~02:37 EET):** (1) the progress window's Speed / Time-left must change continuously, not disappear and reappear — the 63b test showed them flickering to '—' between values. Cause: the engine's speed is an instantaneous per-250-ms-tick delta, so a tick with no new bytes reports speed 0 and eta null (checkpoint writes it to the row every 1 s; each progress event carries it). Fix, frontend only: ProgressWindow.tsx holds the last positive speed / ETA for up to 5 s while downloading (refs + effect, cleared off 'downloading'); DownloadTable.tsx does the same per row (Map keyed by id). A sustained idle shows '0 B/s' / '—'. `formatSpeed` gained an optional zero label (default '—'). (2) The in-page footer Close button was removed from the progress window (title-bar X stays; empty-state Close kept; Open file / Open folder on completion kept). Engine suite 459/459; npm build passes (4.30 s). No Rust edited. NOT pushed; VPS build + retest pending.

## Round 65 — System tray + IDM right-click menu — 2026-11-18

- **User requests (2026-11-18 ~02:38–02:44 EET, approved "تمام"):** (1) minimize sends the app to the system tray like IDM, including the download windows; X on the main window also hides to the tray (does NOT exit); exit is right-click tray icon → Exit. (2) IDM-like right-click menu on download rows (from his IDM screenshot: Open, Open with..., Open folder, Redownload, Resume Download, Stop Download, Refresh download address, Remove, Move to queue, Delete from queue, Properties), every item enabled/disabled by the file's state; Refresh Address removed from the top TOOLS strip and available only from this menu.
- **System tray:** `app/src-tauri/Cargo.toml` — `tray-icon` feature enabled. `app/src-tauri/src/lib.rs` — `build_system_tray()` builds the tray from the app's default window icon (the Sonic logo) with a Show / Exit menu (ids `tray-show` / `tray-exit`); left-click release shows + focuses the main window; Exit calls `app.exit(0)` (the existing exit path). Built best-effort in `setup` with log lines — a failed tray never blocks startup. `Builder.on_window_event` intercepts the main window's `CloseRequested` → hide + `prevent_close()` (progress windows unaffected — their X still closes just that window). `open_progress_window`'s already-open branch now calls `window.show()` before `set_focus()`. `app/src-tauri/capabilities/default.json` — added `core:window:allow-hide` + `core:window:allow-show`. `app/src/components/TitleBar.tsx` — new `hideOnClose` prop (default false): `minimize()` hides (both windows) and `close()` hides when `hideOnClose`, else closes; button labels/titles updated ("Minimize to tray", "Hide to tray" / "Close"). `app/src/App.tsx` — main-window `<TitleBar>` now uses `hideOnClose`. Downloads keep running while windows are hidden (engine is independent of the windows).
- **Right-click menu:** `app/src/components/DownloadTable.tsx` — `RowMenu` rebuilt in IDM order with state-based disabled rules (Open: complete only; Open with...: file exists; Open folder: file/folder exists; Redownload: always; Resume: paused/error only; Stop: downloading only; Refresh address: paused/error only; Remove: always; Move to queue: inline click-to-expand section listing existing queues, shown only when queues exist; Delete from queue: only when in a queue; Properties: always). "Open progress window" and "Move to category" kept. Menu is scrollable (`max-h-[80vh]`) and the top clamp changed to `innerHeight − 200`. `app/src/components/DownloadPropertiesDialog.tsx` (new) — read-only dialog in the existing dialog style: filename, URL, status + %, size, downloaded, speed, save path, category, queue, added, completed; Escape/backdrop close. `app/src-tauri/src/lib.rs` — new sync command `open_file_with` (Windows-only `rundll32 shell32.dll,OpenAs_RunDLL <path>`; file-must-exist check via `manager.get_download`). `app/src/lib/api.ts` — `openFileWith(id)` wrapper. `app/src/App.tsx` — `propertiesTarget` state, `handleRedownload` (cancel if downloading, `deleteDownload(id, true)`, re-add same URL/folder/category/queue, select new id), all new handlers wired to `DownloadTable`, dialog rendered.
- **Refresh Address out of the TOOLS strip:** `app/src/components/ToolsStrip.tsx` — `onRefreshAddress` / `canRefreshAddress` props, RefreshCw import and button removed; `app/src/App.tsx` call site updated. Refresh stays available only from the right-click menu (existing wiring unchanged).
- **Verification:** engine suite 459/459 (0 failed, all 22 binaries); npm build passes (tsc + vite, 4.73 s) after one type fix — the menu ref prop must be `React.MutableRefObject<HTMLDivElement | null>` (what `useRef` returns), not `React.RefObject<...>`. `cargo fmt --check`: 40 hunks, all pre-existing style (none in Round 65 regions); the import ordering in lib.rs was fixed to match rustfmt. During that check a real bug was caught and fixed: inserting `build_system_tray` had stranded `#[cfg_attr(mobile, tauri::mobile_entry_point)]` on it instead of `run()` — moved back to `run()`. App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile. NOT pushed; VPS build + retest pending (plus the user's full-app + extension test pass).

## Round 66 — "Download complete" dialog + notification sound — 2026-11-18

- **User request (2026-11-18 ~02:48–02:53 EET, approved "نفذها ممتازه"):** on completion, close the progress window and show an IDM-like "Download complete" dialog in Sonic theme, from his IDM 5.08 screenshot (`workspace/user/files/17855_1_2vhj.jpg`) and the approved mockup (`workspace/download-complete-preview.html` — dark title bar with the real Sonic logo, purple→pink gradient divider, Open button purple→pink gradient like the progress bar, muted grey labels, cyan check like "Done"; inner heading is the FILENAME, not "Download complete" again — title bar only). Plus a built-in notification sound on completion.
- **Dialog:** `app/src/components/DownloadCompleteDialog.tsx` (new) — cyan check, filename heading, "Downloaded X MB (N Bytes)", Address (final URL, else URL), "The file saved as" (full save path), buttons Open (primary gradient via inline style — `Button`'s `bg-accent` class would fight the gradient) / Open with... / Open folder / Close, "Don't show this dialog again" checkbox persisted at once via `update_settings`. Row polled until it appears (progress-window pattern); missing row shows the empty state with Close. Sound plays once on mount (ref guard, failures swallowed).
- **Window:** `app/src-tauri/src/lib.rs` — new async `open_complete_dialog` + `build_complete_dialog_window` (label `complete-<id>`, title "Download complete", frameless, fixed 560x440, min 520x400, not resizable, `window.__COMPLETE_ID__` init script + `#complete=<id>` hash fallback, shared main WebView2 environment on Windows like the progress window, same log lines). Idempotent — an already-open dialog is shown + focused. NO heartbeat registration and NO auto-reload (transient dialog; `main.tsx` sends no heartbeat for it, so the monitor never sees it and `reload_webview_now` is never called for a `complete-*` label). `app/src-tauri/capabilities/default.json` — "complete-*" added to windows. `app/src/main.tsx` — `completeDownloadId()` + render branch + heartbeat skipped for complete windows. `app/src/components/TitleBar.tsx` — new `showMinimize` / `showMaximize` props (default true); the dialog hides both (minimize would hide it to the tray where the tray menu could never bring it back).
- **Trigger:** `app/src/App.tsx` — new effect on `[downloads, settings]` detects a flip into 'complete' (prev 'downloading'/'paused', or first-seen with `completed_at` within 30 s — a tiny file can complete between two 1 s polls), deduped per id, skipped when `show_download_complete_dialog` is off, and skipped when the download's Options say close-after / shutdown-after (those take precedence; the progress window acts on them). Failures are silent — the dialog must never spam the error banner. `app/src/components/ProgressWindow.tsx` — on the same flip with neither option set and the dialog enabled, the window closes itself (`getCurrentWindow().close()`); suppressed (or settings not yet loaded) keeps today's behaviour (window stays, footer Open file / Open folder). Buttons reuse existing commands: Open = `open_file`, Open folder = `open_folder`, Open with... = Round 65 `open_file_with` (rundll32 OpenAs_RunDLL).
- **Setting:** new `show_download_complete_dialog` (default true) — `app/engine/src/store.rs` (struct + Default + migration `ALTER TABLE ... ADD COLUMN ... DEFAULT 1` + `get_settings` SELECT index 30 + `update_settings` `?31`), `replica/schema.sql` (fresh DBs), `app/src/lib/types.ts`, `app/src/components/SettingsDialog.tsx` (checkbox next to "Show the download progress window" — the re-enable path). `app/src/lib/api.ts` — `openCompleteDialog(id)`, `playCompletionSound()`.
- **Sound:** NO binary asset shipped (the connector cannot push binaries). `play_completion_sound` in lib.rs synthesizes the chime at runtime in `builtin_complete_wav()` — two sine tones, A5 880 Hz 0.22 s then D6 1174.66 Hz 0.32 s, 22050 Hz 16-bit mono, 10 ms attack + exponential decay, deterministic bytes — writes it to the app data dir (`complete.wav`, temp dir fallback) and plays it with the same detached PowerShell SoundPlayer the engine uses. Order: (1) a user-configured `download_complete` sound wins — the engine already played it at completion (`emit_sound`, manager.rs:4314), so the command no-ops; (2) built-in chime; (3) Windows default Asterisk sound when the chime cannot be written or played. Windows only, like every other sound.
- **Verification:** engine suite 459/459 (0 failed); npm build passes (tsc + vite, 4.89 s). `cargo fmt --check`: my regions clean (2 hunks fixed — the `already open — focused` log line and the `Err(_)` fallback map); all remaining hunks pre-existing (incl. the same long-line pattern in `open_progress_window`). App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile. NOT pushed; rides with the next push + VPS retest (plus the user's full-app + extension test pass).

## Round 67 — Add URL dialog redesign + extension capture opens the dialog — 2026-11-18

- **User request (2026-11-18 ~03:01–03:09 EET, approved "تمام نفذ"):** redesign IDM's Add URL ("Download File Info") dialog in Sonic identity, from the approved mockup (`workspace/add-url-preview.html`); and the Add URL dialog must appear BOTH from Add URL in the app AND when a download is captured by the browser extension (his choice 03:04 EET "تظهر في الاتنين") — like IDM, the extension capture opens the dialog with the URL pre-filled so he can confirm folder/name/category first, instead of auto-starting.
- **Dialog redesign (`app/src/components/AddDownloadDialog.tsx`):** dark title bar with the real Sonic logo (`/icon.png`, same as TitleBar) + "Add URL" + X, purple→pink gradient divider under it (like the Round 66 dialog). Fields in IDM order: URL; Category dropdown + "+" add-category button (Plus icon; opens `AddCategoryDialog` via new `onAddCategory` prop — App renders it after this dialog in the DOM so it paints above); Save As as ONE editable full path (folder + filename, split on the last separator; a bare name is a rename — the folder stays); "Remember this path for '<category>' category" checkbox (disabled with no category) with the probed file size in cyan on the same row; a read-only folder-path field under it showing the effective folder (typed folder, else global default, else OS Downloads) with a trailing separator; Description field; per-download speed cap kept as a compact row (label left, input right). Buttons in mockup order: Download Later / Start Download / Cancel — the default-action button is the purple→pink gradient primary, slightly wider (inline gradient style + `px-6`, same trick as the Round 66 dialog). Tightened spacing (11 px gaps, 13 px fields, 620 px max width) so the dialog is shorter/squarer. Round 55 submit machinery kept verbatim (detached late-outcome handler, 60 s timeout race, `submitSeq`/`anySucceeded`, Escape/backdrop blocked while busy, `onAdded(id)` / `onLateError`); the speed-cap input keeps the Round 56 numeric handling (text + inputMode, strip non-digits, select-all on focus). New: a debounced (500 ms) probe prefills the filename from the server (only when the user has not typed one) and fills the cyan size; picking a category autofills the folder with that category's default folder (the engine resolves the same way) unless the user already typed a folder; "..." opens the native folder picker (`@tauri-apps/plugin-dialog` 2.8.4 — new dependency, see below).
- **Description:** no description support existed anywhere, so it was added end to end: `description TEXT` in `replica/schema.sql` + migration in `Store::migrate` (PRAGMA table_info check + ALTER TABLE, cookies pattern) + `description: Option<String>` with `#[serde(default)]` on `DownloadRow` + `r.get(26)?` in `row_to_download` + `, description` on all 4 SELECT lists + `Store::set_download_description` (trims, empty clears) + manager passthrough + Tauri command. The dialog writes it right after the add/confirm (non-fatal — a failure never blocks the download).
- **"Remember this path":** persists the folder as the category's `default_folder` via the existing `updateCategory` (only when a category is selected and the folder is non-empty; failures swallowed so the download still starts).
- **Extension captures open the dialog (`app/src/App.tsx`, engine drain):** the native host is UNCHANGED — it still stages captures as `pending_links` rows. What changed is who decides them: `DownloadManager::drain_pending_links` now SKIPS `browser`/`video` rows (they stay pending for the dialog) and drains only `download_all` rows; the exclusion re-check and the browser-integration-off rejection still run first, so an excluded capture is rejected by the drain and never reaches the dialog. The app polls `list_pending_links` with the 1 s refresh (own in-flight guard, silent failures) and queues unseen browser/video rows (`seenCaptureIds` ref); an effect pops one capture at a time into the dialog (`initialUrl` + new `pendingLinkId` + `initialCookies` props) only when no dialog is open — a capture arriving while the dialog is open waits its turn, dialogs never stack. On close without submitting, the row is resolved `rejected` ("Dismissed in the Add URL dialog") — but only after re-listing, so the engine's real error (e.g. a 404 from a failed confirm) is never clobbered; on submit, the dialog's confirm already marked it `accepted`. `captureAddedRef` tells onClose the submit happened.
- **Confirm path (engine + Tauri):** new manager method `confirm_pending_link(id, filename, folder, category_id, queue_id, speed_limit_kbps, start_now)` → `(String, ProbeInfo)`: re-checks browser-integration + excluded-sites (rejects the row on failure), runs the row's cookies through the real `add_download_with_cookies` (probe + segments carry them), applies the speed cap BEFORE starting, starts when asked (a failed start leaves the download queued — the drain's `start_failed` behaviour), marks the row accepted with the download id, or rejected with the reason when the add fails (so the dialog shows the error and stays open). A double confirm is an error ("already decided"), so a link can never be added twice. New manager methods `list_pending_links`, `resolve_pending_link`, `set_download_description`, `probe_url_for_capture` (plain probe, or with the row's cookies — a cookie-gated link probes 401 without them); `probe_url` is now `pub`. New Tauri commands `list_pending_links`, `resolve_pending_link`, `probe_url`, `set_download_description`, `confirm_pending_link` (registered in `invoke_handler`). Frontend `app/src/lib/api.ts` wrappers + `PendingLinkRow` type in `types.ts` (matches the engine struct field for field) + `description?: string | null` on `DownloadRow`.
- **Folder picker plugin:** `@tauri-apps/plugin-dialog@2.8.4` added to `app/package.json` (+ lockfile), `tauri-plugin-dialog = "2"` to `app/src-tauri/Cargo.toml`, `.plugin(tauri_plugin_dialog::init())` in the Builder chain (lib.rs), `"dialog:allow-open"` appended to `capabilities/default.json`. First real compile is the VPS build (the app crate cannot compile on this VM).
- **Unchanged by design:** the clipboard flow (the offer's Download button still adds directly — it never opened this dialog; the task's "clipboard-offer pattern" reference was about surfacing, not the add call), the Download All flow (`download_all` rows still drain + auto-start), and the video panel's prefill (`initialUrl`, now also used by captures). The extension itself is untouched (popup v2 + video button are the next item, approved 03:16 EET).
- **Tests:** engine suite **464/464 (0 failed, 22 binaries)** — 5 new tests, 1 renamed, 8 rewritten for the new drain behaviour: `drain_leaves_browser_capture_for_the_add_url_dialog` (renamed from `drain_picks_up_enqueued_link_via_add_download`: drain leaves the browser row pending, confirm adds + starts it byte-exact), `drain_skips_browser_and_video_rows_but_drains_download_all`, `confirm_pending_link_twice_is_an_error`, `confirm_pending_link_bad_url_is_rejected` (404), `description_round_trip`, `probe_url_for_capture_uses_the_capture_cookies` (401 without, size with); rewritten with `download_all` rows: `drain_leaves_download_queued_when_start_drained_is_off`, `drain_duplicate_url_gets_different_save_path`, `drain_marks_dead_link_rejected_and_keeps_going`, `drain_sends_cookies_only_to_the_download_that_carries_them`; rewritten via confirm: `drain_cookie_gated_link_downloads_when_cookie_present`, `drain_cookie_gated_link_rejected_without_cookie`, `cookies_are_dropped_when_the_app_restarts` (confirm with start_now=false → queued → restart → 401); rewritten via confirm: `video_found_end_to_end_drains_into_a_real_download`, `video_found_m3u8_drains_into_a_real_playlist_download`; rewritten with `download_all`: `final_drain_starts_download_not_just_queued`, `final_drain_duplicate_links_get_different_paths`. Unchanged and still passing: `drain_rejects_excluded_site_at_drain_time`, `drain_rejects_when_browser_integration_turned_off`, `enqueue_page_links_expands_a_page_into_pending_rows`, `final_drain_refuses_excluded_site`. Two compile errors caught by the run: an extra `.flatten()` after `.and_then` in `probe_url_for_capture`, and the dialog plugin's `open` import shadowed by the dialog's `open` prop (renamed to `openFolderPicker`).
- **Verification:** npm build passes (tsc + vite, 4.93 s after the `openFolderPicker` rename). `cargo fmt --check`: the local rustfmt is a newer version that reflows chains and `use` ordering across the whole repo (hundreds of hunks in untouched files, same as prior rounds); my regions are clean except the same version-drift pattern — the long import line was wrapped multi-line and the long test asserts were wrapped to the repo style. App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile, and the first real proof of the dialog + tray + menu + completion dialog + taskbar-less build is the VPS test. NOT pushed; rides with the next push + VPS retest (plus the user's full-app + extension test pass).
- **Decisions to flag:** (a) The gradient primary follows the S09 default-action setting: with the default (`start_now`) it is Start Download, exactly as the mockup; with `save_for_later` it is Download Later. The task brief said "Start Download is the primary" — the setting was kept because it is the only thing the setting controls in this dialog. (b) Edge case: submit → 60 s timeout → user closes the dialog (row resolved rejected) → the late attempt then succeeds: the download appears in "All downloads" (correct), but the pending row stays 'rejected' — the store only decides a row while it is still pending, so the late success cannot flip it back to accepted. (c) The "Remember this path" checkbox is disabled until a category is picked (the path is stored on the category); picking "no category" clears it.

## Round 68 — Download failed dialog + sounds + cap 32 + extension v2 + taskbar progress — 2026-11-18

- **User approvals (2026-11-18 ~03:16–03:45 EET):** (1) IDM-style "Download failed" error dialog (his IDM screenshot `workspace/user/files/-5577617508813472082_5_k98s.jpeg` — he corrected my wrong claim that IDM shows no separate popup; Details text changes per error, approved 03:39–03:40 EET). (2) Completion sound = Windows "tada", picked 03:44 EET ("حلو tada"), ordered BUNDLED 03:45 EET ("متحطه إنت عشان منحتاحش حاجه"); error alert = first custom descending tone (740→494 Hz), approved 03:42 EET; master sounds on/off toggle in Settings, approved 03:43 EET. (3) Segment cap 16 → 32, ordered 03:21 EET ("تمام ارفعه") — he had declined it 2026-11-16, then reversed; IDM default 8 / max 32 verified by web search. (4) Extension popup v2 + floating video button, approved 03:16 EET ("تمام نفذ"). (5) Taskbar progress, approved 03:18 EET ("ماشي نفذه نجرب").
- **Error dialog:** new `DownloadErrorDialog.tsx` (polls the row, yellow warning triangle SVG, message + Details from `error_message`, OK gradient button, plays the error sound once on mount, empty state with Close); lib.rs `error_window_label` + `build_error_dialog_window` (520×360, `window.__ERROR_ID__`, shared WebView2 env on Windows) + async `open_error_dialog` (idempotent focus); `main.tsx` `errorDownloadId()` (`__ERROR_ID__` / `#error=`) render branch, heartbeat skipped for error windows; `App.tsx` error trigger effect (flip from a previously-seen downloading/paused/queued status only, deduped per id, silent failures); `ProgressWindow.tsx` Cancel also shows for 'error' status (Error badge + red error line + Resume already existed). `api.ts` gained `openErrorDialog`, `playErrorSound`, `setTaskbarProgress`. Round 66's completion dialog untouched.
- **Sounds:** `app/src-tauri/assets/tada.wav` copied from `~/workspace/windows-tada.wav` (285,228 bytes); `tauri.conf.json` resources gained `"assets/tada.wav": "tada.wav"` (binary — push via the browser route, the connector cannot push it). lib.rs: `builtin_error_wav()` (740 Hz 0.16 s + 494 Hz 0.32 s) + shared `play_notification_sound()` helper (master switch → user-configured file no-op → bundled asset → synthesized built-in → Windows Asterisk); `play_completion_sound` delegates with `Some("tada.wav")` + chime fallback; new `play_error_sound` command (no asset, `error.wav` cache). Both commands registered.
- **Sounds setting + cap:** engine `store.rs` — `sounds_enabled: bool` (struct + Default true + migration tuple + SELECT index 31 + update_settings `?32` + params); cap validation `1..=16` → `1..=32` with updated error string; `manager.rs` comment "1–16" → "1–32", `emit_sound` checks `sounds_enabled` (fails open on read error). `replica/schema.sql` — `sounds_enabled INTEGER NOT NULL DEFAULT 1`. `types.ts` — `sounds_enabled: boolean`; `SettingsDialog.tsx` — Connections validation/label/input max 16 → 32, Sounds tab master "Play notification sounds" checkbox at the top.
- **Extension:** `popup.html`/`popup.js`/`popup.css` rewritten for v2 (Connected pill with nested dot/text spans, Developer card, Facebook link, note, version footer; no toggles/buttons/handoff code). `content.js` restyled to Sonic identity (dark panel #0B0F24, purple→pink gradient button, compact top-right "Download" button, quality rows with Full HD/HD tags and sizes only when known, "Sonic Manager" naming) + `qualityTag()` helper. `background.js`: new "Find videos on this page" context-menu item + `injectVideoDetector()` helper shared with the kept `scan_page_for_videos` message handler. `tests/popup.test.js` rewritten for v2 (3 tests: Connected / host unreachable / host error). `README.md` updated (popup v2, menu trigger, install steps 2–8).
- **Taskbar:** lib.rs `set_taskbar_progress` command (progress 0–100 or None to clear; status normal/paused/error; `tauri::window::ProgressBarState` + native window title for the tooltip), registered. `App.tsx` taskbar effect: aggregate percent over downloading rows with known size; single active = its own percent; all-paused = paused state; nothing active clears the bar + restores "Sonic Manager" title; invokes only when rounded percent/title changes.
- **Verification:** engine suite **464/464 (0 failed, 22 binaries)** — first full run failed only `settings_reject_invalid_values` (its final "nothing was stored" assertion expected base 8, but the valid 32-save legitimately leaves the row at 32 — fixed the assertion, single test passes, full re-run green). npm build passes (tsc + vite, 20.99 s). Extension Node tests: popup 3/3, background 4/4, video 17/17, watcher 11/11, manifest validators OK. `cargo fmt --check`: my regions clean (fixed `play_error_sound` signature + `sounds_enabled` tuple); remaining hunks are pre-existing newer-rustfmt drift. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; rides with the next push + VPS build + the user's full-app + extension test pass.

- **Round 69 — six Round 68 retest failures (2026-11-18, local only, NOT pushed):** investigate-first round, user approved ("نفذ").
- **Extension (a):** lib.rs `register_native_host_best_effort` (Windows only, called from `setup` after "settings applied"; best-effort, failures logged, never fatal): finds `fetchline-host.exe` next to the app exe or under `src-tauri/binaries` (dev build), canonicalizes the path, writes Chrome + Firefox manifests (UTF-8, no BOM) into the app data dir (`fetchline/native-host/`), and points the Chrome / Edge / Firefox `HKCU\...\NativeMessagingHosts\com.fetchline.host` keys at them via std-only `reg.exe` (`query` first — idempotent; `CREATE_NO_WINDOW` so no console flashes). No new dependency (`serde_json` already present). `background.js`: watcher now pauses → hands over → cancels/erases ONLY on confirmed receipt → resumes the browser download on any hand-off failure; `sendToHost` (background + popup) bounded at 15 s. Menu handler's empty catch now records injection failures to `lastVideoScan`. `popup.js`/`popup.html`/`popup.css`: the pill's failure now shows Chrome's real `lastError.message` in a detail line (guarded — a missing element never breaks the pill), plus a scan line showing the last video-scan outcome. `tests/popup.test.js` (fake gains `statusDetail`/`scanLine` + `chrome.storage.local`; +1 scan-line test, asserts the detail text) and `tests/watcher.test.js` (fake gains `pause`/`resume` + `hostError`; +1 resume-on-failed-handoff test; first test asserts pause-then-cancel order). Repo host manifests left as-is (install-time templates; the installer script rewrites them).
- **Clipboard / queued rows (b):** `App.tsx` clipboard offer chains `startDownload(id)` after `addDownload`; `Toolbar.tsx` + `DownloadTable.tsx` menu gate Resume to paused/error/queued; `App.tsx` `handleRedownload` starts the new row after adding.
- **Error dialog + stale captures (d):** `DownloadErrorDialog.tsx` gains a message variant (`message`/`title`/`onClose` props; no polling; same layout + error sound); `TitleBar.tsx` gains optional `onClose` (X dismisses an overlay instead of closing the window); `AddDownloadDialog.tsx` gains `onSubmitError` (fired in the submit catch with the message + typed filename); `App.tsx` holds `addError` state and renders the dialog as a `z-[70]` overlay above the Add dialog. Engine: `Store::expire_stale_pending_links` (rejects browser/video rows older than 24 h) + call at the top of `DownloadManager::drain_pending_links`.
- **Right-click menu (e):** `DownloadTable.tsx` — the queue section is always rendered; "Delete from queue" is always shown, disabled when `d.queue_id === null` (with a title); header comment updated.
- **Settings (f):** `manager.rs` — `initial_ranges` now uses the connections setting as the segment count (`max_connections.min(size / MIN_SEGMENT_BYTES)`, new `MIN_SEGMENT_BYTES = 256 KiB` floor); `INITIAL_SEGMENTS` removed. (5a) and (5c) are documented non-bugs in `bugs.md` — no code change.
- **Verification:** engine suite **466/466 (0 failed, 22 binaries)** — was 464, +2 new tests (`max_connections_setting_raises_segment_count` in `engine_tests.rs`, `stale_browser_capture_expires_in_drain` in `extension_tests.rs`). npm build passes (tsc + vite, 4.87 s). Extension Node tests: video 17/17, popup 4/4, background 4/4, watcher 12/12, manifest validators OK. `cargo fmt --check`: my regions clean (lib.rs, manager.rs, store.rs); remaining hunks are pre-existing newer-rustfmt drift. App crate cannot compile on this VM — VPS build is the first real compile of the new Rust. NOT pushed; rides with the next push + VPS build + full RDP retest of all six fixes.

## Round 70 — User's own-PC failures + Round 69 retest failures (HTTP client, host, console flash, UI gaps) — 2026-11-18

- **Round 69 retest verdict (VPS, live Windows):** BUILD PASS (exe 19,406,848 bytes). Extension still FAIL ("Not connected / Error when communicating with the native messaging host"; auto-capture swallows the download; video button never reappears). Clipboard fresh offer PASSES; old stuck "Waiting" row Resume still FAILS (row stays "Waiting"; 15.1 MB uptodown link, 404). OVH 100Mb.dat: probe OK (100.0 MB), Start to 38%, then FAILS "Network error: error sending request for uri (https://proof.ovh.net/files/100Mb.dat)", row Failed, Resume on Failed does nothing; progress window showed 11 connections with the setting at 8. 404 error dialog PASS. Delete from queue PASS. Segments PASS (8 parts on 20 MB fixtures). Regression mostly PASS. NEW: main-list TRANSFER RATE sometimes "0 B/s"/"—" while the progress window shows live speed; list went 22 → 5 → 6 → 7 rows (17 "missing"); URL 1's error dialog hard to dismiss (only Escape worked); URL 2's Start Download did nothing (silent; startup.log shows the probe failed).
- **Downet — the HEAD→GET probe fallback (item d) is REFUTED, kept only as zero-risk defence:** VPS curl HEAD on both downet URLs = 200 OK (Content-Length ~948 MB / ~924 MB, Accept-Ranges: bytes); ranged GET 0-1023 = 1024 bytes. The app's probe fails in ~177–190 ms with "error sending request" — it never sends the request. Root cause is the HTTP client, not the probe method. The fallback (already written, fires only when HEAD dies at transport level) stays; it cannot fix downet.
- **HTTP client — root cause PROVEN (2026-11-18, `openssl s_client` from this VM):** `app/engine/Cargo.toml` used `reqwest` with `rustls-tls` (rustls 0.23.46, bundled Mozilla roots). downet serves an INCOMPLETE chain — exactly 1 certificate (leaf only, CN `*.downet.net`, issuer `SSL2BUY EMEA RSA Domain Validation Secure Server CA`, Sectigo intermediate, TLS 1.2, ECDHE-RSA-AES256-GCM-SHA384, "verify error:num=20/21: unable to get local issuer certificate"); the leaf carries an AIA extension pointing at the Sectigo intermediate. rustls-webpki does NOT fetch missing intermediates via AIA; Chrome and Windows SChannel DO — that is why Chrome and VPS curl work and the app fails fast. Direct curl from this VM fails the same way, confirming it is not app-specific. OVH's chain is COMPLETE (3 certs), so the OVH mid-download failure is NOT explained by this fix and needs VPS observation. Fix: `reqwest` feature `rustls-tls` → `native-tls` (one line, `app/engine/Cargo.toml`) — SChannel on Windows (AIA fetching + Windows store, same behaviour as Chrome/Edge), OpenSSL on Linux. `build_client`/`build_segment_client` (`app/engine/src/proxy.rs:345–386`) are plain builders (User-Agent `Fetchline/0.1`, connect + 30 s timeouts, no TLS tuning) and needed no change; no rustls-specific code anywhere in engine/host src; the Tauri shell and host crates have no reqwest/rustls dependency. Verification: engine suite **468/468 (0 failed)** with native-tls (first run aborted at 370 passed when the 512 MB /tmp tmpfs filled with leftover `fetchline-test-*` dirs — quota_tests failed with SQLite DiskFull, an environment artifact, not a code failure; after `rm -rf /tmp/fetchline-test-*` the full re-run is green). `Cargo.lock` updated by the test run.
- **Host no-window + lazy DB + host.log (a)(b) — done in the first half of the round:** `app/host/src/main.rs` — `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]` (GUI subsystem, no console on Chrome spawn), `log_line` + `ensure_store` lazy open (Ping answered without the DB), framed error replies instead of `exit(1)`, `host.log` next to `startup.log` (never stdout). New spawn test `host_answers_ping_when_database_is_unusable` (app.db as a directory → ping still pongs, download_link gets a framed error naming the DB failure, host.log written, clean exit 0). NOTE (user evidence 2026-11-18 ~14:47 EET): he DELETED the extension and the cmd window STILL appears — the host is NOT the cause of the flashing he still sees; the PowerShell sound spawns are the prime suspect. The host fix stays (a console host would flash whenever the extension IS installed) but is no longer presented as the flashing cause.
- **PowerShell CREATE_NO_WINDOW (steer 1) — done:** `CREATE_NO_WINDOW` added to both PowerShell sound spawns in lib.rs (`play_wav_with_sound_player`, `play_default_system_sound` — module-level const hoisted, local one removed), `shutdown_computer`, `open_file_with` (rundll32); engine `manager.rs` `play_wav_file`; engine `queue.rs` rasdial, shutdown ×2, `cmd /C start` (`run_program` and the user-configured antivirus scanner deliberately left alone). Full `Command::new` sweep of lib.rs + engine: every console-spawning call reachable from a failed download now carries the flag; the failure path fires TWO unflagged PowerShell consoles per failed download (engine `emit_sound("download_failed")` → `play_wav_file`, plus the error dialog mount → `play_error_sound` → `play_wav_with_sound_player`), matching "appears, disappears, appears again" exactly.
- **Registration (c) — done:** same-dir `fetchline-host-x86_64-pc-windows-msvc.exe` candidate added to `register_native_host_best_effort` (lib.rs), so the shipped ZIP works with the host named EITHER `fetchline-host.exe` OR `fetchline-host-x86_64-pc-windows-msvc.exe` next to the app exe (verified by code inspection of the candidate list).
- **Probe fallback (d) — done, kept as defence only:** `probe.rs` — transport-level HEAD failure falls back to ranged GET (bytes=0-0) in `probe_with_proxy` + `probe_with_cookies`, like the existing 405/501/403 fallback; +2 tests (`probe_falls_back_when_head_drops_connection`, `probe_with_cookies_falls_back_when_head_drops_connection`, new `head_reset` mock mode).
- **(1) Old "Waiting" row Resume — INVESTIGATED, NOT an engine bug, no code change:** `resume_download` (`manager.rs:4185`) re-probes; on probe failure the `map_err` sets the row to `error` with the real message BEFORE returning the error, and every other outcome (probe OK → `start_download` → `run`, whose every Err also sets `error`) moves the row out of `queued`. No code anywhere sets a row back to `queued` after creation, so a Resume click on the 404 uptodown row MUST flip it to `error` with "Server replied 404". The UI path is also correct (toolbar + menu both call `resumeDownload` → Tauri `resume_download`; `run()` shows the error banner; the 1 s poll refreshes the list). The observed "stays Waiting" cannot be produced by the engine — retest must re-check with the sidebar filter visible (see (4)) and confirm whether the row flips to error after the click.
- **(2) OVH 11 connections with the setting at 8 — EXPLAINED, label fixed:** `try_split` (`manager.rs:4987`) splits the largest remaining segment whenever a running task frees up (`tasks.len() < max_connections()`), inserting a new segment row via `store.split_segment` — the part count grows past the setting (8 initial + 3 splits = 11) while CONCURRENT connections never exceed 8. The progress window's "Parts (N connections)" label conflated the two. Fix: `ProgressWindow.tsx` now reads "Parts (N parts)". The setting is honoured; no engine change.
- **(3) Main-list speed "0 B/s"/"—" while the progress window shows live speed — FIXED (engine):** the supervisor tick computes an instantaneous 250 ms speed, but the 1 s checkpoint persisted that single sample to the DB — a tick with no bytes wrote 0, and bursty flow could keep the DB at 0 for >5 s (expiring the Round 64 hold) while the progress window's 250 ms event snapshots stayed live. Fix (`manager.rs` `run_inner`): the checkpoint now carries the average over the checkpoint interval (`checkpoint_total`/`checkpoint_at` baselines; first interval seeded a full second back), so the DB speed is positive whenever any bytes arrived in the last second and the ETA derives from it too. The live emit keeps the instantaneous value. No new test (checkpoint speed needs a live download); the full suite must stay green and the retest must confirm the main list holds live speed during a bursty download.
- **(4) 17 missing rows — INVESTIGATED, most probable cause is a list filter, NOT data loss:** `list_downloads` returns every row with `status != 'deleted'`; the ONLY writers of `deleted` are user Remove (`lib.rs:1408`) and user Cancel (`manager.rs:4443`) — no automatic mass deletion exists in code. The frontend `visible` memo filters by selected queue / `unfinished` / `finished` / grabber / category / search text; 22 rows (mostly complete from earlier rounds) → an "Unfinished" filter click shows only the ~5 non-complete rows, and newly added downloads raise it to 6, 7 — exactly the observed pattern. Retest must confirm by checking the sidebar filter state and, decisively, `SELECT COUNT(*) FROM downloads` on the VPS app.db (if the DB still holds 22 rows, nothing was lost).
- **(5a) Error dialog hard to dismiss — FIXED (frontend):** the per-dialog close paths were already correct in code (OK → `closeDialog` → `onClose`/`getCurrentWindow().close()`; X → TitleBar `close()`; `Button` forwards `onClick` via `{...rest}`) — the observed failure is most plausibly the stacked dialogs (the error overlay `z-[70]` opens OVER the still-open Add dialog, which also shows the same error inline, so dismissing the overlay leaves an error-showing dialog behind) plus no keyboard support: the dialog never took focus (Enter had nowhere to land) and had no Escape handler of its own (the Escape that "worked" belonged to the Add dialog underneath, which does not clear `addError`). Fix (`DownloadErrorDialog.tsx`): `autoFocus` on the OK button (React 18 — `ref` does not forward through the plain `Button` component, `autoFocus` spreads onto the native button) + a window Escape handler calling `closeDialog`, for both variants.
- **(5b) Silent Start Download failure on URL 2 — PARTLY FIXED (frontend), rest is the TLS bug:** the submit path is correct in code (button enabled whenever not busy; failure → `setError` + `onSubmitError` → overlay), and the engine-side cause of the probe failure is the rustls chain issue above. But the debounced probe failed SILENTLY (`catch { setFileSize(null) }`) — no size, no filename, no signal at all before Start, which is exactly the "no probe result, no size" observation. Fix (`AddDownloadDialog.tsx`): new `probeError` state, shown as muted text next to the size ("Could not check this link — Start will try again", full reason in the `title` tooltip); advisory only, not the pink submit error. If Start still appears to do nothing after the TLS fix, the retest must check whether the click landed (RDP scaling has missed clicks before) or the button was busy from an earlier in-flight submit (60 s timeout bound).
- **Verification:** engine suite **468/468 (0 failed)** with native-tls + checkpoint change (final re-run after all edits — see below); host tests unit 2/2 + spawn 2/2; npm build passes (4.78 s); extension Node tests video 17/17, popup 4/4, background 4/4, watcher 12/12, manifest validators OK. `cargo fmt --check`: my regions clean; remaining diffs are pre-existing drift. App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile of the new Rust (native-tls switch is a Cargo feature line only; the checkpoint change is plain arithmetic in `run_inner`). NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): both downet URLs + OVH 100Mb.dat downloading in the app (the TLS proof); extension popup Connected on a real PC layout; no console flash on host spawn or on a failed download (PowerShell fix); old "Waiting" row Resume flips it to error with the 404; main-list speed stays live during a bursty download; row count vs app.db (filter vs deletion); error dialog dismisses with OK / Enter / X / Escape; a dead link shows the muted probe note in the Add dialog before Start.

## Round 71 — Second-AI audit #2: extension/host, OVH 38% stall, white flash, minor regressions — 2026-11-18

- **Basis (user-approved "اه" 2026-11-18 ~18:43 EET):** second-AI reply #2 (received ~18:41 EET) on the Round 70 retest verdict + the log evidence read ~17:56 EET, accepted with 2 corrections. **Correction A:** the second AI's "Chrome launches hosts via cmd.exe" mechanism is UNCERTAIN (Chrome most likely launches directly, not via cmd.exe) — but the ACTIONS are still right: strip `\\?\` and revert the host to console, because Chrome spawns hosts hidden (CREATE_NO_WINDOW) anyway, so a console host does NOT flash; the flash the user saw came from the app's own PowerShell spawns, already fixed with CREATE_NO_WINDOW in Round 70. **Correction B:** the stale-host idea is already REFUTED by timestamps (Round 70 host exe 4,889,088 bytes, 11/18 1:36:24 PM, newer than the Round 70 commit; both host copies identical) — the rebuild-every-time safeguard below is kept as procedure, not as the cause. Diagnostics that PROVED the starting facts: host answers a framed ping with pong + exit 0 when launched by hand (while the app was running, no DB-lock exit); host.log appeared ONLY after that manual run (Chrome never launched the host — failure is at LAUNCH, not I/O); OVH parallel curl test: 5/8 succeeded (206), 3 rate-limited (429), 0 timed out, connects fast (0.135–0.311 s) — OVH rate-limits parallel range requests from the VPS IP.
- **A1 — host back to console:** `app/host/src/main.rs` — `#![windows_subsystem = "windows"]` REMOVED, with a comment explaining why the host MUST stay console: a GUI binary detaches from the stdio pipes, so Chrome can spawn it and never talk to it — the Round 70 "Error when communicating with the native messaging host" failure. Verified by grep: the host never spawns fetchline.exe via `Command` (no such call in `app/host/src/`), so there was nothing to flag there.
- **A2 — `\\?\` stripped at registration:** `app/src-tauri/src/lib.rs` — new `fn clean_windows_path` strips a leading `\\?\` (and `\\?\UNC\` → `\\`) before the path is written to the manifest JSON and the registry; the resolved host path is now logged (`native host: host exe resolved to...`); manifest paths also pass through the cleaner. The two `Command::new("reg")` calls already had `.creation_flags(CREATE_NO_WINDOW)` (lines 2615, 2628) — verified, no change needed.
- **A3 — build-procedure safeguard (PROCEDURE, not code):** EVERY build/retest must rebuild the host crate FIRST — `cargo build --release -p fetchline-host` (or `--manifest-path app/host/Cargo.toml`) — copy it as the sidecar `app/src-tauri/binaries/fetchline-host-x86_64-pc-windows-msvc.exe`, and print the host exe LastWriteTime into the build log. The Tauri build does NOT rebuild the host (separate Cargo workspace), so a stale host is otherwise silent. Do NOT change the CI workflow files — the connector cannot write `.github/workflows`; if CI must do this, it goes via the browser route (GitHub web UI).
- **A4 — extension ping-first + video button:** `app/extension/background.js` — new `pingHost()` (3 s bound, silent failure); the download watcher now pings FIRST — if the ping fails it returns early and the browser download proceeds natively, never paused, never swallowed. Pause → hand over → cancel-only-on-receipt → resume-on-failure flow unchanged. `app/extension/content.js` — the floating video button now shows immediately for the first video with variants (fixes "mouse already over the video when injected via context menu, so no mouseenter ever fires") + document-level `mouseover` delegation (mouseenter doesn't bubble) so videos added after injection also show it. NO host-health gating on the button (there never was any); `sendVariant` still re-attempts native messaging only on click. Verified: content.js has NO link-click interception / `preventDefault` anywhere, so that part of the plan had nothing to change.
- **B1 — connect timeout 8 s:** `app/engine/src/proxy.rs` — new `MAX_CONNECT_TIMEOUT = 8 s`; both `build_client` and `build_segment_client` now cap connect timeout at `min(setting, 8 s)` so dropped SYNs fail fast instead of hanging for the 21 s Windows TCP SYN timeout. The user setting still governs shorter values.
- **B2 — per-segment retry + non-fatal degradation (the big change):** `app/engine/src/manager.rs` — `SegOutcome::{Done, Deferred(EngineError)}`; `SegState` gains `running`, `done_at_start`, `started_at`, `deferred_until`, `deferrals` + a `SegState::new()` constructor (all 3 construction sites use it); `RunningGuard` clears `running` on Drop, including on panic. `run_segment` is now a wrapper (marks the segment running/done_at_start/started_at, clears deferred_until; guard clears running on exit) + `run_segment_inner` (retry loop). New params: `store: Arc<Store>` (per-attempt logging), `solo: bool`. `is_deferrable(e, received_bytes)`: only when the attempt received NO bytes AND (reqwest `is_connect()`/`is_timeout()` OR status 429/503 OR a send-phase stall — the new `SEND_STALL_MSG_PREFIX = "No response headers for "` const, used by the new `send_stall_error()` for the `.send()` timeout; a BODY stall — headers arrived, then silence — keeps the old `stall_error()` and is NOT deferrable, see the verification note below). Deferrable failures get a fast retry loop first: 3 retries at 500 ms / 1.5 s / 3 s (via new `sleep_backoff` helper, 100 ms slices, pause/"Retry now" aware, no retry-slot write), bounded by `max_attempts` (`attempt < max_attempts.min(4)`). After the attempts: non-solo → mark deferred (backoff 2/4/8 s by deferral count), return `Ok(Deferred(e))`; solo → return the error (fatal — nobody left to pick it up). Non-deferrable failures keep the old retry loop (max_attempts, 2/4/8/16 s backoffs). `log_segment_attempt` logs every failed attempt: segment index, byte range, attempt number, elapsed ms, full error chain. `app/engine/src/error.rs` — `EngineError` now implements `source()` (Http/Db/Io/Json expose their inner error); new `error_chain_text(e)` walks the full chain ("Network error:... <- client error (Connect) <-...") so "error sending request" stops hiding the cause. Supervisor (`run_inner`): new `effective_cap` starting at the connections setting; on `Deferred`, ratchets down to the number of workers that actually received bytes (min 1) and logs it. Loop-top: `tasks.is_empty()` no longer breaks blindly — breaks only when no segment is waiting on a deferral (see the verification note: a `done >= len` check would spin forever on open-ended segments); otherwise respawns a deferred segment (solo) or sleeps a tick (avoids entering `select!` with an empty JoinSet, whose `join_next` returns None at once). Tick branch calls `try_split` every 250 ms while a slot is free (deferred-respawn + stealing between completions).
- **B3 — segment stealing:** `try_split` now takes `cap: usize` and a `JoinSet<Result<SegOutcome, EngineError>>`: FIRST respawns a deferred segment whose backoff expired (solo if it's the only task); THEN stealing — prefers splitting a segment stuck in Connecting (running, done == done_at_start, started > 5 s ago, remaining > 256 KiB) over the largest remaining segment. Verified safe: the stream loop re-reads the shared `end` per chunk and stops at the split point, so a stuck task's late connection cannot overlap the new half. Split-off halves also run solo when alone.
- **B5 — mock + test:** `app/engine/tests/common/mock.rs` — `MockConfig.max_concurrent_gets: Option<usize>`; `MockServer` gains `inflight_gets` + `rate_limited` counters and a `rate_limited()` accessor; `handle()` refuses the (max+1)th concurrent GET with 429 (HEAD never counted; `InflightGet` guard keeps the count exact across all early returns). New test `app/engine/tests/round71_tests.rs`: `download_completes_when_server_limits_concurrency_to_three` — 2 MiB file, 8 segments of 256 KiB, 2 s/chunk (~6 s per segment, longer than the 0.5+1.5+3.0 = 5 s retry schedule so refused segments MUST defer), limit 3; asserts completion, byte-exact file, `rate_limited() > 0`, and the engine log (via `Store::set_log_path`) contains "deferred". PASSES standalone in ~23 s.
- **C1 — background colour #0B0F24 everywhere:** `app/src-tauri/tauri.conf.json` — main window gains `"backgroundColor": "#0B0F24"` and `"visible": false` (file kept single-line minified; the first edit pretty-printed it and was reverted to compact form). `app/src-tauri/src/lib.rs` — `.background_color(tauri::window::Color(11, 15, 36, 255))` + `.visible(false)` on all three WebviewWindowBuilders (progress, complete, error). `app/index.html` — `<style>html, body, #root { background-color: #0B0F24;}</style>` in `<head>` (CSP already allows `style-src 'unsafe-inline'`). `app/src/components/DownloadTable.tsx` — outer table container div gains `bg-[#0B0F24]`.
- **C2 — auto-reload spam stopped:** `maybe_auto_reload` now returns `Ok(false)` and removes the heartbeat entry when `app.get_webview_window(label)` is None (closed window) — stops the dozens of "webview progress-<guid> auto-reload FAILED: No progress window open" lines in startup.log. Main window unaffected (its X hides to tray, never closes).
- **C3 — show-when-ready:** `app/src-tauri/src/lib.rs` — new `spawn_show_fallback` (detached thread, shows the window after 3 s unconditionally — `show()` on a visible window is a no-op) called in each builder's `Ok(window)` arm and in `setup()` for the main window; new `window_ready` Tauri command (Tauri injects the calling `Window`; `window.show()`), registered in `invoke_handler`. `app/src/lib/api.ts` — new `windowReady()` wrapper; `app/src/main.tsx` — new `WindowReady` component firing `windowReady()` in `useLayoutEffect` after the root's first commit, rendered inside StrictMode next to the window component (serves main/progress/complete/error alike; failures swallowed — the 3 s fallback guarantees appearance). Verified against vendored tauri-2.12.5: `WebviewWindowBuilder::visible(bool)` (webview_window.rs:934), `background_color(Color)` (1248), `tauri::window::Color(pub u8, pub u8, pub u8, u8)` re-exported from tauri-utils, `Window`/`WebviewWindow::show()` — all exist.
- **D1 — speed 1:4 ratio — VERIFIED, NOT a ×4 bug:** the checkpoint formula divides bytes by actual elapsed seconds (correct in steady state); the 1:4 ratio could NOT be reproduced from code (retest numbers were ranges from different moments; list values matched the fixture server's 32 KiB/s × 8 = 256 KiB/s ceiling while the progress window showed burst-inflated instantaneous readings). The ONE real defect: `checkpoint_at` was pre-aged by CHECKPOINT_INTERVAL, so the first checkpoint's denominator (~1.25 s) covered only ~0.25 s of bytes (~5× understatement, one sample). Fixed: `checkpoint_at = Instant::now()`. NO blind ×4.
- **D2 — red banner X:** `app/src/App.tsx` — new `dismissedBannerRef` + `setPollError` helper — poll-failure paths (`refresh`, `refreshQueues`, `refreshQueueFiles`, `refreshCategories`, `onQueueAction` catch) now refuse to re-show the exact dismissed message; a successful poll clears the memory so a genuinely new failure with the same text still shows. X button records the dismissed message and gains `style={{ WebkitAppRegion: 'no-drag'} as CSSProperties}` (with a `type CSSProperties` import).
- **D3 — stale completion dialog:** `app/src/App.tsx` — the first-snapshot branch now seeds `completeDialogHandled` with all already-completed download IDs.
- **D4 — deleted files listed as Finished:** `app/engine/src/store.rs` — `DownloadRow` gains `file_missing: bool` (not a DB column; `#[serde(default)]`); `mark_missing()` sets it in `list_downloads` and `get_download` for `complete` rows whose `save_path` doesn't exist (a `stat` per complete row per poll); `app/src/types.ts` gains `file_missing?: boolean`; `descriptionFor` in DownloadTable returns "File missing from disk" for complete+missing rows.
- **D5 — probe guards:** `app/src/components/AddDownloadDialog.tsx` — probe fires only when the trimmed input starts with http://, https:// or ftp:// AND passes `new URL(val)` AND contains no second `://` (concatenated URLs); new monotonic `probeReqId` counter discards stale responses (old `probedUrl` string guard kept as a second check).
- **Verification — two regressions the suite caught and the fixes they forced:** (1) The first full run failed `ftp_download_without_size_is_open_ended_and_byte_exact` (proxy_tests): my loop-top check broke only when `done >= len` for every segment — an open-ended segment's length reads as u64::MAX, so the supervisor spun forever. Fixed: break only when NO segment is waiting on a deferral (`deferrals > 0 && done < len`); every other incomplete segment always has a task, so an empty task set with no deferred segment means done. (2) The second full run failed `stall_watchdog_fails_a_wedged_download` (round55_tests, max_retries = 1, server sends headers then silence): my first `is_deferrable` treated ANY stall as deferrable, so the segment deferred forever instead of failing. Fixed: the deferrable stall is ONLY the send-phase stall (no response headers at all — new `send_stall_error()` / `SEND_STALL_MSG_PREFIX` on the two `.send()` timeout sites); a body stall keeps `stall_error()` and stays on the old retry-then-fail path. The fast-retry loop is also bounded by `max_attempts` (`attempt < max_attempts.min(4)`), so a max_retries = 1 segment gets exactly one attempt per run.
- **Verification (final):** engine suite **469/469 (0 failed, 24 binaries)** — 468 after Round 70 + 1 new Round 71 test; per-binary: lib 56, antivirus 17, clipboard 15, engine 122, extension 38, final_verify2 4, final_verify 16, grabber 28, grabber_verify 14, history 8, kerberos 14, options 15, progress 8, proxy 15, quota 19, real_download 2, round55 8, round56 4, round57 2, round71 1, sync 23, update 9, video 31, doc-tests 0. The first full run (before the two fixes above) completed in ~256 s with exit 0 but its per-binary counts were not recovered (head-truncated); the final re-run after all fixes is the counted one (334 s). Host tests unit 2/2 + spawn 2/2 (incl. framed-ping tests). Extension Node tests: video 17/17, popup 4/4, background 4/4, watcher 13/13 (was 12/12; +1 new ping test "watcher never pauses when the host does not answer the ping"), manifest validators OK. npm build passes (tsc + vite, 4.39 s). `cargo fmt --check`: my Round 71 regions clean after fixes (4× `try_split` call sites → multi-line; `log_segment_attempt` signature; `store.rs` collect + `mark_missing`; `round71_tests.rs` add_download; `if best.is_none()` block; the `.send()` match brace at manager.rs:5927); remaining diffs are pre-existing newer-rustfmt drift in untouched code (809 hunks in the engine crate alone — do not reformat the repo). App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile of the new Rust (`window_ready` / `backgroundColor` / `visible:false`, `clean_windows_path`, the whole C3 show-when-ready flow are uncompiled here, though the APIs were verified against vendored tauri-2.12.5). NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): extension popup Connected on a real Chrome + the host actually launched by Chrome (host.log must exist this time); auto-capture hands the download over (no swallowed click); video button appears on hover AND immediately; OVH 100Mb.dat completes in the app (the deferral path); no console flash on host spawn; white-flash timing per window (main, progress, Add URL, error, completion, Properties, Settings) with the show-when-ready flow; main-list speed vs progress-window speed on the same download; red banner X stays dismissed across polls; completion dialog does not re-show for an earlier finished download; a Finished row whose file was deleted shows "File missing from disk"; probe does not fire on "X", partial protocols, or concatenated URLs.

## Round 72 — Extension auto-injection, OVH patient tail, start-with-1 segmentation — 2026-11-18

- **Basis (user-approved "نفذ" 2026-11-18 ~22:29 EET):** the Round 71 retest verdict (~22:21 EET): OVH 100Mb.dat failed at 99% (final segment, 5th retry, connect timeout); extension popup Connected for the first time BUT auto-capture swallowed file:// clicks and the video button never appeared on the real w3schools page even though the popup said "1 downloadable video found"; Settings "Connections" field flaky ("02"/"22" typed by accident); Resume/Redownload clicks once did nothing on a Failed row. The user's confirmed extension spec (~22:28 EET): the video button appears automatically next to any video (IDM-like); clicking a download link sends it straight to the program and the Add URL dialog opens pre-filled. Dynamic segmentation (start-with-1) was already approved as part of this round (~19:31 EET trigger, re-approved ~22:29 EET).
- **A1 — auto-injection (the root cause, VERIFIED IN CODE ~22:22 EET):** `app/extension/manifest.json` gains `content_scripts` (matches `http://*/*`, `https://*/*`, `run_at: document_idle`, js `video-detect.js` + `content.js`) — before this round the detector was injected ONLY via the right-click "Find videos on this page" menu item (`background.js` `injectVideoDetector`, `chrome.scripting` + `activeTab`), so retesters who hovered without the menu click never had anything injected; the popup's "1 downloadable video found" was a stale `lastVideoScan` from an earlier page. `content.js` keeps the `window.__fetchlineVideoInstalled` double-injection guard (set before the async gate); new `video_allowed` gate (asks background once, defaults to allowed on failure); scan logic extracted into `scanAndAttach()` (re-scans, prunes detached videos, attaches mouseenter once per video via an `attached` set, re-reports `videos_found`); `init()` wires mouseover delegation + immediate show + scroll listener once (rescans only re-scan); new `chrome.runtime.onMessage` handler for `rescan_videos` replying with the summary. Also fixed a pre-existing bug in `sendVariant` (~line 391: `v.format` → `variant.format` — the reply callback threw ReferenceError and the quality button stayed stuck on "Sending…"). Round 71 A4 behaviour kept: button shows immediately for the first video with variants + document-level mouseover delegation, never gated on native-host health. Right-click menu item still works.
- **A2 — popup scan line:** `app/extension/popup.js` — `refreshScanLine` rewritten: on popup open it triggers a fresh scan (`scan_page_for_videos`) and renders the reply directly as "This page: …" when it carries numbers; waits 300 ms and reads storage after the injection fallback; stored scans are labelled with their page host ("Last scan (<host>): …") when they are not the current page; a fresh-scan failure shows only when nothing is stored. New `renderScan` + `hostOf` helpers. `app/extension/background.js` — menu "Find videos" now calls new `scanActiveTab()` (rescan via `chrome.tabs.sendMessage`, falls back to `injectVideoDetector` when the tab has no content script); new `video_allowed` message handler (integration on + not excluded, defaults allowed on state-read failure); `scan_page_for_videos` handler returns the rescan summary; `injectVideoDetector` kept as the fallback. `app/extension/tests/validate_manifest.py` updated — it used to forbid `content_scripts`; it now pins exactly one entry (matches http/https only, js video-detect.js + content.js, run_at document_idle) and still forbids `host_permissions`.
- **B — auto-capture, verified in code (no weakening of "never cancel until confirmed"):** the watcher (`chrome.downloads.onCreated` → catch list → `pingHost` 3 s → pause → `handToFetchline(download_link)` → cancel ONLY on confirmed receipt → resume on failure) is correct as written. The file:// question is settled: the watcher keys on `item.url` (the file's URL), never the page scheme, so an http(s) link on a file:// page is caught normally; a `file://` download itself is correctly skipped (the file is already local; the engine probe rejects non-http(s)/ftp with "Only http://, https:// and ftp:// addresses are supported" — verified in `probe.rs`); comment added to the watcher. The Round 71 retest's swallowed file:// clicks are most plausibly the fixture server being down at that moment. Direct navigation to an mp4 opens Chrome's media player — no download event exists, so the watcher correctly never fires (test artifact, not a bug). Add URL dialog path verified still wired (Round 67 design: engine `drain_pending_links` skips browser/video rows; captures queue one at a time; dialog opens with URL pre-filled).
- **C1 — OVH patient tail (the big engine change):** `app/engine/src/manager.rs` — new `SegOutcome::TailRetryable(String, EngineError)` (carries the segment row id); `is_tail_patient(e)`: transport errors incl. received-bytes cases, 429/503, send-stall; NEVER body stalls or 404/416/other statuses; `TAIL_BACKOFFS_SECS = [5, 15, 30, 60]` (cap 60 s); `PatientTailGuard` arms a `patient_tail` flag on the `RetrySlot`; all three watchdogs (tokio `run_watchdog`, thread watchdog, in-loop watchdog in `run_inner`) suppress on the patient flag; `run_segment`/`run_segment_inner` take `tail: bool` with the patient wait via `patient_tail_wait` (publishes to the retry slot with `max_attempts = 0` = unbounded, resets the attempt counter, interruptible by pause/Retry now); supervisor `TailRetryable` arm respawns as a patient tail with the tail client when `unfinished_count == 1`, else fails as before; `unfinished_count` helper; initial spawn + both `try_split` spawns pass `tail` (initial spawn tail when 1 unfinished; deferred respawn tail when 1 unfinished; split-off half never tail). `app/engine/src/proxy.rs` — tail client (`TAIL_CONNECT_TIMEOUT` = 20 s, `build_tail_segment_client`) + `tail_segment_client` field/init/`apply_settings`/accessor in the manager. Round 71's per-segment fast retry + deferral + cap ratchet + stealing kept for the non-tail phase; checkpoints preserved so a later Resume continues from the tail.
- **C2 — the `is_request()` discovery (caught by the new test, see Verification):** a dropped connection surfaces from reqwest as `Kind::Request` ("connection closed before message completed", mid-send close) — `is_connect()` (connect phase only), `is_timeout()` and `is_body()` (body streaming only) all miss it, so the tail test's drop error took the slow path (2/4/8/16 s backoffs) and never entered patient retry. Fix: `is_tail_patient`'s Http arm now also accepts `re.is_request()`. Deliberately NOT added to `is_deferrable`: the slow path's retry countdown for a dropped connection is pinned by `final_verify_tests::final_retry_countdown_on_dropped_connection_then_retry_now_recovers` (a fast-retry path publishes no countdown), so a dropped connection keeps its slow backoffs and only escapes into patient retry here, after `max_attempts`. Verified against vendored reqwest-0.12.28 `error.rs` (`is_request` = `Kind::Request`, `is_body` = `Kind::Body` only).
- **C3 — Redownload in place (the dead Resume/Redownload clicks):** new `redownload_download(id)` in the manager (bounded 2 s wait for a run to unregister; probe with the row's cookies; on probe failure the row survives — complete rows keep their status, others record the new error; then delete_segments + reset + fresh `initial_ranges` + start; the `.part` file is kept, not deleted). Tauri command `redownload_download` registered in `app/src-tauri/src/lib.rs` (same shape as `start_download`/`resume_download`, in `generate_handler!`); `app/src/lib/api.ts` `redownloadDownload`; `app/src/App.tsx` `handleRedownload` rewritten to it (cancel-first only when downloading, then select the same id); `run()` gains a delayed (400 ms) `actionBusy` state shown as "Working…" in the status-bar footer, so a click always has visible feedback. (The Round 71 retest's dead clicks were never reproduced in code — the likely cause was the row state machine having no in-place redownload path for Failed rows; the new command gives one, with immediate UI feedback.)
- **C4 — Settings connections field:** `app/src/components/SettingsDialog.tsx` — the field is now a draft string (`type="text"`, `inputMode="numeric"`, digits-only, max 2 chars, commit on blur/Enter clamped 1–32, empty draft keeps the setting, draft reset on open); hint rewritten ("Maximum simultaneous connections per download. A file starts as one part and splits into more parts as connections free up…"). This kills the "02"/"22" accidents from the retest.
- **D1 — dynamic segmentation (start-with-1, IDM model):** `MIN_SPLIT_BYTES` = 1 MiB replaces `MIN_SPLIT_REMAINING`/`MIN_SEGMENT_BYTES`/`MIN_SIZE_TO_SEGMENT`; `initial_ranges` now returns exactly 1 segment (or 1 open-ended segment) regardless of the connections setting; `try_split` splits at `remaining >= MIN_SPLIT_BYTES` in both paths (stuck-steal + largest-remaining). Kept: probe, `set_len` pre-allocation + `write_at` in-place writes (NO merge step), checkpoints ~1 s, per-segment retry + deferral + cap ratchet + stealing, 8 s connect timeout, connections cap (default 8, max 32), single-stream fallback when no Accept-Ranges or file <2 MB. `initial_ranges` path kept for the single-stream fallback. The connections setting is now a pure concurrency cap, never a segment count.
- **D2 — UI clarity (the "6 parts on 2 connections" confusion):** `ProgressEvent.active_connections` added (count of `running` states) and populated in `emit()`; `app/src/lib/types.ts` gains it; `RetryInfo` doc notes `max_attempts` 0 = unbounded. ProgressWindow parts header shows "{N} connection(s) · {done}/{total} completed" beside the kept "Parts (N parts)" label, and the retry line renders "— retrying until it succeeds" when `max_attempts` is 0. `DownloadRow` gains `active_connections` + `segment_count` (Rust `store.rs` struct with `#[serde(default)]`, `row_to_download` indices 27/28, all 4 full selects updated, `schema.sql` columns added, `migrate()` ALTER loop added, `store.checkpoint()` takes the two new params, `reset_download()` zeroes them, manager `checkpoint()` computes them from states); `app/src/components/DownloadTable.tsx` shows "{N} connection(s) · {M} part(s)" under the progress bar for downloading rows only.
- **Tests — new:** `app/engine/tests/round72_tests.rs` (4 tests): start-with-1 + growth to 8 segments on an 8 MiB file at cap 8 (also asserts `row.segment_count >= 2`); tail patient retry (512 KiB, cap 1, `drop_get` flipped off after polling the log for "entering patient retry (tail)", asserts status stays "downloading" then completes byte-exact); tail never patient on a body stall (`set_stall_get`, `max_retries` 1, asserts error with "stalled" and no patient log line); redownload (complete row survives a gone-server probe unchanged; redownload after recovery completes with 1 segment; failed row via `set_fail_get` then redownload after recovery completes).
- **Tests — rewritten:** `app/engine/tests/engine_tests.rs` — the three max-connections tests rewritten to the start-with-1 model (`max_connections_from_settings_changes_segment_count`: 1 at creation; grows to 4 at cap 4, stays 1 at cap 1, grows to 2 at cap 2; `max_connections_setting_raises_segment_count` RENAMED to `max_connections_setting_is_a_cap_not_a_segment_count`: 1 at creation at cap 8 and 32; grows only to 4 via the 1 MiB floor; `max_connections_change_does_not_alter_running_download`: waits for steady state 4 segments before changing the setting to 1, then asserts still 4). All use 4 MiB files — the 1 MiB floor applies to the bytes REMAINING, so a 2 MiB file only reaches 2 segments (see Verification). `app/engine/tests/extension_tests.rs` — `extension_manifest_is_mv3_and_minimal` updated: it used to assert no `content_scripts`; it now pins exactly one entry (matches http/https, js video-detect.js + content.js, run_at document_idle). `app/engine/tests/round71_tests.rs` — the OVH mock test reworked: 2 MiB → 4 MiB file, chunk delay 2 s → 1 s (a 2 MiB file can only grow to 2 segments under start-with-1, so the 4th GET never fired and the mock never refused anything; 4 MiB grows to 4 segments by ~750 ms, the 4th GET is refused, deferral fires; total ~33 s). `app/engine/tests/proxy_tests.rs` — `ftp_download_multi_segment_byte_exact` reworked: 2 MiB → 4 MiB + `chunk_delay: 50 ms` (the old file finished in ~300 ms before it could grow past 2 segments; now 4 segments, 4 RETRs).
- **Extension tests:** 49/49 (was 38): background +5 (rescan via content script, injection fallback, opted-out refusal, `video_allowed` gate, menu rescan), popup +4 (fresh scan wins over stale stored, stored scan labelled with its host, fresh failure only when nothing stored, injection-fallback storage read), watcher +2 (file:// skipped, http(s) link on a file:// page caught). Fakes extended (`tabs.sendMessage`, `scripting.executeScript`, `URL` in vm contexts, popup `runtime.sendMessage` + `tabs.query`); cross-realm assertions use JSON comparison.
- **Verification — three regressions the suite caught and the fixes they forced:** (1) The first round72 run failed 2/4: the tail test never entered patient retry (the `is_request()` discovery above — the drop error took the slow path, so the log poll expired at 30 s; fixed by adding `is_request()` to `is_tail_patient` and bumping the poll to 60 s, since the slow backoffs take ~30 s first) and the growth test ended at 4 segments, not 8 (the 1 MiB floor is on REMAINING bytes — a 4 MiB file's 1 MiB halves shrink below it before the next tick; fixed by using an 8 MiB file). (2) The first full run failed `extension_manifest_is_mv3_and_minimal` (it asserted no `content_scripts` — stale since A1) and `download_completes_when_server_limits_concurrency_to_three` ("the mock never refused a request" — the Round 71 test's 2 MiB file; fixed as above). (3) The second full run failed `ftp_download_multi_segment_byte_exact` (1 RETR — the 2 MiB FTP file; fixed as above). All three fixes are test-side except the `is_request()` engine fix.
- **Verification (final):** engine suite **473/473 (0 failed, 25 binaries)** — 469 after Round 71 + 4 new Round 72 tests; per-binary: lib 56, antivirus 17, clipboard 15, engine 122, extension 38, final_verify2 4, final_verify 16, grabber 28, grabber_verify 14, history 8, kerberos 14, options 15, progress 8, proxy 15, quota 19, real_download 2, round55 8, round56 4, round57 2, round71 1, round72 4, sync 23, update 9, video 31, doc-tests 0 (single clean run, 444 s, exit 0; the run before it was 472/473 with only the FTP failure, fixed after). Host tests unit 2/2 + spawn 2/2. Extension Node tests 49/49 (background 9, popup 8, video-detect 17, watcher 15), manifest validators OK. npm build passes (tsc + vite). `cargo fmt --check`: my Round 72 regions clean (fixed: try_split condition join, active_connections join, checkpoint params! reflow, 36× add_download call sites → multi-line, 4× round72 assert_eq joins, 2× FTP test reflows); remaining diffs are pre-existing newer-rustfmt drift in untouched code (772 hunks in the engine crate alone — do not reformat the repo; two hunks directly adjacent to my tail-client block — the import ordering at manager.rs:51 and the segment_client fallback at :594 — are pre-existing lines, left as-is). App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile of the new Rust (`redownload_download` command follows the existing command pattern exactly and is registered in `generate_handler!`; no new window APIs were used in this round). NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): the floating video button appears AUTOMATICALLY on the w3schools video page (no menu click) and on hover; the popup's scan line says "This page: …" for the current page; clicking a direct download link opens the Add URL dialog pre-filled (nothing downloads in Chrome); OVH 100Mb.dat completes (the tail keeps retrying past 99% — watch the parts table show "1 connection · 6 parts" and the retry line "retrying until it succeeds"); Redownload on the Failed OVH row restarts it in place; the Settings connections field accepts only digits and clamps to 1–32; the main list shows "N connection(s) · M part(s)" under downloading rows.

## Round 73 — Video button rework, OVH tail coalesce, probe retries + stored-info fallback, auto-capture hardening — 2026-11-19

- **Basis (user-approved "نفذ" 2026-11-19 ~03:03 EET):** merged decision from TWO second-opinion reviews of the Round 72 retest (brief `~/workspace/your_files/sonic-manager-problems-round73-second-opinion.md`; replies ~03:00 and ~03:02 EET). Both confirmed the 4 diagnoses; the merged plan takes the stronger variant of each fix.
- **Video (extension):** `content.js` reads the preference keys directly via `chrome.storage.local.get` (no `video_allowed` service-worker round-trip on init's critical path; the background handler was removed); scan runs immediately on init + capturing `loadedmetadata` listener + debounced `MutationObserver` for injected `<video>`/`<source>`; `manifest.json` content_scripts gain `all_frames: true` (+ `match_about_blank`); hover detection via throttled (~100 ms) `mousemove` + `document.elementsFromPoint` (shows the button when the pointer is inside a mapped video's rect, not only when `e.target === video`); per-video `variants` deduped by URL (reuses `dedupeVariants`); sizes dropped from the panel (no `v.size`, no HEAD requests); timing logs behind a debug flag (injection, storage resolve, first scan, first show, first mouseover). New `app/extension/tests/content.test.js` (7 tests).
- **OVH tail (engine):** supervisor `TailRetryable` branch coalesces contiguous unstarted Pending segments into the failing segment when `tasks.is_empty()` and `total_done > 0`, so `unfinished_count` becomes 1 and the patient tail engages (defer-and-spawn-pending was rejected: the pending segment would hit the same fatal gate ~30–40 s later); tail eligibility re-evaluated at respawn time; the gate decision (`unfinished_count`, `tasks.len()`, `total_done`, `is_tail`) is always logged; new setting `tail_zero_progress_cap_minutes` (default 10) — zero byte progress for N minutes while patient retrying fails visibly; `Retry-After` honoured on 429/503 + per-host connection cap (2) after a 429; 250–500 ms jitter on deferred respawns. Pinned invariants kept: body stalls never patient; a dead server still fails (total_done > 0 guard + the cap). `replica/schema.sql` + `store.rs` carry the new setting.
- **Probe / Resume / Redownload (engine + UI):** fast retries (500 ms / 1.5 s / 3 s) inside the shared probe path for transport errors, 429, 503 only; 404/410/401/403/416 fail immediately with the dead-link message. Resume continues from stored info on transient probe failure (no longer sets the row to error). Redownload proceeds from the stored `total_size` when all probe retries fail transiently (still restarts from zero, row intact on 404). Distinct probe-failure message ("Could not reach the server; the link may still work. Try Resume."). `run()` gains an `immediate` flag — Redownload/Resume show "Working…" at once; Redownload on Failed/partial rows warns in the UI ("Starts over; Resume keeps your progress").
- **Auto-capture:** `scripts/e2e/fixture-server.mjs` gains `/test-link.html` (download-attr link, `Content-Disposition: attachment` link, plain-link control, small .zip link); `background.js` wraps `pause()` in `.catch`, cancels if `in_progress` else erases if already complete, and matches on `item.url` when `item.filename` is empty at `onCreated`.
- **Verification (final):** engine suite **479/479 (0 failed, 26 binaries)** — 473 after Round 72 + 6 new Round 73 tests (`tail_coalesces_unstarted_pending_segment_and_completes`, `dead_server_fails_after_the_zero_progress_cap`, `redownload_retries_transient_probe_and_restarts_failed_row`, `redownload_refuses_on_404_probe`, `resume_continues_from_stored_info_on_transient_probe_failure`, `host_is_capped_at_two_connections_after_a_429`). Host tests unit 2/2 + spawn 2/2. Extension Node tests **59/59** (was 49: +7 content, +3 watcher). npm build passes. App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile. RETEST MUST CHECK (not verifiable locally): the video button appears instantly on hover on the w3schools page (no click, no ~3 s wait) including on videos added after load and behind overlays; qualities list shows "320 × 176" exactly once and no sizes; OVH 100Mb.dat completes past 92% (watch the parts table coalesce to one tail); a dead server fails after the zero-progress cap with a clear message; Redownload AND Resume both restart the Failed OVH row (Resume keeps 92%, Redownload starts over with a warning); auto-capture via the new fixture page AND via `chrome.downloads.download()` from the service-worker console; "Working…" appears immediately on Redownload/Resume clicks.

## Round 74 — User-found 16 bugs + whole-download stall retry — 2026-11-19

- **Round 74 fix (2026-11-19 ~08:46 EET, after the VPS build failed with 7× E0308):** `show_window_ready` took `&tauri::Window` but all 7 call sites pass `&tauri::WebviewWindow` (and the `window_ready` command injected `tauri::Window`). Fixed: `show_window_ready(window: &tauri::WebviewWindow)` + `window_ready(window: tauri::WebviewWindow)` — verified against vendored tauri 2.12.5 (WebviewWindow has show/label/set_focus/set_always_on_top + Clone; no public WebviewWindow→Window conversion exists).
- **Basis (user-approved "نفذ كله" 2026-11-19 ~07:08 EET):** 12 user-found bugs presented ~06:48 EET, plus his later additions — capture dialog = same Add URL window with filename title + locked URL (~06:51), dialogs pop in front as independent windows (~06:53), dialog-close audit (~06:57), video dropdown (item 15, separate agent), drag & resize narrowed to main + active progress window only (~07:10–07:11). Item 15 (video Download dropdown) was implemented by a parallel agent (extension suite 63/63) and is NOT in this entry's file list. Implemented by two builders (backend: src-tauri + engine; frontend: src) coordinated by a parent, plus a coordinator follow-up for the missing queue commands.
- **(1) Two stacked progress windows:** root cause was a race — two concurrent `open_progress_window` calls could both pass the already-open check before either built. Fix: `WINDOW_BUILD_MUTEX` + `lock_window_build()` in `lib.rs` serializes ALL window creation; the mutex is held across check+build in `open_progress_window` / `open_complete_dialog` / `open_error_dialog` / `open_add_url_window` and across close+rebuild in `reload_webview_now` (which now polls up to 2 s for the label to free before rebuilding); every builder re-checks for an existing window and focuses it instead of building a second.
- **(2) Free-space stale:** `App.tsx` now recomputes free space right after remove / delete-completed / download-finish, not only on the 30 s poll.
- **(3) "Download removed" window:** restyled in `ProgressWindow.tsx` as a compact centred card matching the other dialogs (was too big and empty).
- **(4) Redownload on Finished rows:** frontend already called `redownload_download` in place (engine handles complete rows; a failed probe keeps the row complete and the error reaches the banner) — the "old 100% window" symptom is consistent with a failed probe on an expired/tokenised link or the already-open progress window being focused (it polls live). New: `file_missing` rows show "File missing" and Open / Open with... / Open folder are disabled + guarded (row menu + progress window + App handlers warn instead of opening the deleted location).
- **(5) Status pills:** `StatusBadge.tsx` — all pills one fixed size (`min-w-[96px]`, centred); complete + file-missing reads "File missing" in danger tone.
- **(6) Queue delete/rename:** the real gap — NO `delete_queue` / `rename_queue` command or engine method existed at all (only `create_queue` / `update_queue`, and `update_queue` takes no name), so the sidebar buttons had nothing to call. New: `Store::delete_queue` (main queue refused; a running queue is refused manager-side; the queue's downloads move back to the main queue in order, appended after its current files; grabber projects fall back to the main queue) + `Store::rename_queue` (trimmed, non-empty, unique name, main queue keeps its name) + `DownloadManager::delete_queue` / `rename_queue` + Tauri commands `delete_queue` / `rename_queue` (registered in `invoke_handler`). Sidebar UI (inline rename, delete with confirm) and `api.ts` wrappers were wired by the frontend builder. New engine tests: `fresh_queue_can_be_renamed_and_deleted`, `deleting_a_queue_moves_its_downloads_to_the_main_queue`.
- **(7) Dialogs disappearing on outside click:** backdrop-click-to-close removed in 16 places (AddDownloadDialog, AddCategoryDialog, AddQueueDialog, SchedulerDialog, DownloadAllDialog, BatchDialog, VideoGrabberDialog, SiteGrabberDialog, SettingsDialog, UpdateDialog, RefreshAddressDialog, ClipboardOfferDialog, DownloadPropertiesDialog, HistoryView confirm, MenuBar About, App remove/redownload confirms). Dialogs now close only via Cancel/X/Escape.
- **(8) WebView context menu:** global `contextmenu` prevention added in `main.tsx` (every window); only our own menu appears, and only on file rows (DownloadTable RowMenu).
- **(9) Single instance:** `tauri-plugin-single-instance = "2"` (resolved 2.5.5; tauri stays 2.12.5, tao 0.37.2, no other version moved) registered in `lib.rs` — a second launch shows + focuses the main window and exits at once.
- **(10) File > Exit:** verified `exit_app` (`app.exit(0)`) was already correctly wired in Rust; the MenuBar handler called the hide-to-tray path instead — now it calls `actions.exit()` (`exit_app`), so File > Exit quits fully.
- **(11) Cancel confirmation:** new small fixed-size `ConfirmDialog.tsx` ("Are you sure?", Escape/Cancel/X, no backdrop close, no drag) wired into both real Cancel buttons in ProgressWindow (footer + retry box). DownloadTable has no Cancel path (its "Stop Download" is pause; toolbar Delete is remove, already confirmed).
- **(12) Clipboard monitoring:** backend — the shell poller was rewritten to create a fresh `arboard::Clipboard` per tick (a stuck RDP-channel read used to poison the shared handle forever via `try_lock`; now it loses only its own tick), plus logging on start, first read failure, recovery, every offer emitted, and offer-check errors (`arboard = "3"` was already a dependency). Frontend — the clipboard/capture dialog is now the SAME Add URL window: `AddDownloadDialog` gains capture mode (file name as the title, URL entry locked/disabled) and a `standalone` variant; the clipboard offer accept path opens Add URL in capture mode instead of a separate dialog.
- **(13) Independent dialogs that pop in front:** new standalone Add URL window — `build_add_url_window` in `lib.rs` (label `add-url-<pending-link-id>`, init script `window.__ADD_URL_ID__ = "<id>";`, frameless, 560×520 fixed, `resizable(false)`, centered, `#0B0F24`, hidden until ready, shared WebView2 environment, title = file name from the link URL or "Add URL") + commands `open_add_url_window` / `get_pending_link` (registered; `add-url-*` added to `capabilities/default.json`); `main.tsx` routes `__ADD_URL_ID__` / `#add-url=<id>` to the new `StandaloneAddUrlWindow.tsx` (fetches the pending link, renders AddDownloadDialog in capture mode, closes on X/Cancel/Escape and rejects the capture unless submitted; a missing link now shows "This captured link is no longer available." instead of hanging on "Loading…"); App.tsx pending-link flow opens the standalone window for captures instead of the in-window overlay (manual Add URL stays in-window, editable). Raise-in-front: `show_window_ready()` (show + set_focus + always-on-top ~1 s, then clear) is used by `window_ready`, `spawn_show_fallback`, and all focus paths for `add-url-*` / `complete-*` / `error-*` labels, so these dialogs appear over whatever window the user has open even when the main window is hidden to tray.
- **(14) Dialog-close audit:** Escape/Cancel/X added or verified on every dialog/overlay (Add URL, Settings, complete, error, Update, QueueAction, Properties, HistoryView confirm, MenuBar About, remove/redownload confirms, shutdown-countdown Escape in ProgressWindow). Progress windows with an active download and the main window still minimize to tray on X (by design).
- **(16) Drag & resize — NARROWED by the user (~07:10–07:11 EET):** applies ONLY to the main window and the active download progress window; the error dialog, complete dialog, Add URL dialog, and Cancel-confirm dialog stay FIXED-SIZE. `TitleBar.tsx` gains a `draggable` prop (default true) omitting `data-tauri-drag-region` when false; the three fixed dialogs pass `draggable={false}`. Verified in vendored tao 0.37.2: `RESIZABLE` → `WS_SIZEBOX`, `WM_NCHITTEST` hit-tests undecorated resizable windows; main (`resizable:true`) and progress (`.resizable(true)`) already set, dialogs stay `.resizable(false)`. Caveat: our windows carry tao's shadow flag, so tao only hit-tests the top edge and defers other edges to `DefWindowProc` (expected to work with `WS_THICKFRAME`, not provable from this VM) — named fallback if side/bottom edges don't resize: `.shadow(false)` on those two windows (loses the drop shadow — do NOT apply pre-emptively).
- **(A) Whole-download stall retry (engine):** a download stalling mid-file at 0 B/s no longer fails after the 180 s no-data timeout — the supervisor and in-loop watchdog enter stall-retry mode instead (`RetrySlot::whole_stall_retry`, `is_whole_stall_retryable()`: body-stall prefix + patient-eligible transport/429/503; 404/416/disk errors still fail at once), respawning the stalled segments (`respawn_stalled_segments()`); tokio/thread watchdogs suppress during stall-retry with a one-check grace; the patient-tail cap extends to stall-retry with message "No data arrived for {cap} while retrying — the server appears to be down", using the existing `tail_zero_progress_cap_minutes` setting (default 10 min); in-loop watchdog now uses the settable `run_watchdog_timeout_secs()` knob (default still 180 s). 0-byte runs still fail fast (Round 55 pins that). Mock gains `stall_after_bytes` / `set_stall_after_bytes()`. New tests: `whole_download_stall_mid_file_recovers_and_completes` (mid-file stall recovers, byte-exact), `persistently_stalled_download_keeps_retrying_then_gives_up_at_cap` (still "downloading" 5 s past the stall, errors at the cap with the "while retrying" message).
- **(B) Progress-window Resume button:** no frontend defect (same `resumeDownload(id)` as the working toolbar Resume; only missing busy feedback was added — button shows "Resuming…", failures land in the action error) and no engine defect (`resume_download` is correct, proven live by toolbar Resume keeping 53%). Most likely cause is the orphaned duplicate window from (1), whose IPC is detached after its label is overwritten in Tauri's manager — fixing (1) should fix (B). Note: a stalled-but-`downloading` row has no Resume button at all by design (only Pause/Cancel), which can read as "Resume does nothing".
- **(C) Popup "no videos found":** extension-side, no app/host cause. The scan path is entirely extension-side (popup → background → content-script DOM scan); "no videos found" requires a fresh scan reply with `total: 0`. The only app/host involvement (`refreshDesktopSettings` → `get_settings` → `browser_integration` flag) can only produce a "Video scan failed:…" line, never "no videos found". Nothing touched in `app/extension/**`.
- **Verification (final, coordinator re-ran independently):** engine suite **483/483 (0 failed, 27 "test result: ok")** — 479 after Round 73 + 2 stall tests (A) + 2 queue tests (6); `cargo test` exit 0, `/tmp/fetchline-test-*` cleaned before/after. Round 74 tests alone 4/4 (38.76 s). Host tests unit 2/2 + spawn 2/2. npm build passes (tsc + vite). `cargo fmt --check`: changed regions clean (my queue regions + backend regions verified line-by-line); remaining diffs are pre-existing newer-rustfmt drift in untouched code (do not reformat the repo). App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile of the new Rust (`tauri-plugin-single-instance` init, `WINDOW_BUILD_MUTEX`, `build_add_url_window`, clipboard poller rewrite, stall-retry mode). NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): only ONE progress window opens per download (hammer Start/double-click); second launch focuses the running instance; File > Exit quits fully (no tray icon left); Cancel asks "Are you sure?" first; clipboard offer end-to-end (copy a link → same Add URL window, filename title, URL locked); extension capture pops the Add URL window IN FRONT over Chrome with the main window hidden to tray; complete/error dialogs pop in front; queue delete/rename on a fresh queue (delete moves its files to the main queue); free space refreshes after a delete; status pills uniform; no WebView menu anywhere except rows; Escape/Cancel/X on every dialog; drag all four edges + a corner on main and progress windows (see (16) caveat); a mid-file stall retries instead of failing at 180 s (use a flaky local mock, NOT OVH — real IDM also fails the OVH link); progress-window Resume restarts a stalled/failed download.

## Round 75 — User's own-PC test, 7 items — 2026-11-19

- **Basis (user-approved "نفذ" 2026-11-19 ~14:38 EET):** his own-PC test of the Round 74 portable ~14:23 EET found 7 items: (1) extension "sends" but nothing appears in downloads (linkId); (2) Categories click does nothing; (3) clipboard works ONCE then never again + the offer shape differs from agreed; (4) Cancel dialog must match the Sonic theme + fit the window; (5) linkId banner keeps reappearing after closing; (6) extension popup stays "Connected" even when the app is closed; (7) window controls (max/min) work fine — no action. NOT in scope (dropped/closed separately): window resize/shadow — works on his PC, VPS failure was RDP-only; AWS Security Group / brute-force; YouTube/social downloads (next step, needs yt-dlp). All 7 implemented + verified locally; NOT pushed, rides with the next push.
- **(1) linkId:** root cause PROVEN in code — Tauri v2 commands default to camelCase arg keys (vendored tauri-macros 2.7.6 `wrapper.rs:51`); `api.ts:177` `getPendingLink` and `:185` `openAddUrlWindow` sent `link_id`, so EVERY call failed with "missing required key linkId". Fix is JS-side only (no `rename_all="snake_case"` on Rust): both now send `{ linkId }`. New guard test `app/src/lib/apiKeys.test.mjs` (2 tests, passing) scans every `.ts/.tsx` under `app/src` and fails on any invoke argument key containing `_`. **The guard caught TWO MORE broken invokes of the same class:** `update_queue` (keys `schedule_type, start_at, stop_at, start_on_startup, on_complete_action, on_complete_program, sync_interval_minutes`) and `create_grabber_project` (keys `start_url, max_pages, schedule_start_at, queue_id, on_complete_action, on_complete_program`) — verified against `app/src-tauri/src/lib.rs` that the Rust params are REQUIRED (`schedule_type: String`, `start_on_startup: bool`, `on_complete_action: String`; `start_url: String`, `max_pages: i64`, `on_complete_action: String`), so with Tauri's default camelCase keys those calls ALWAYS failed ("missing required key scheduleType" / "startUrl") — queue scheduling could never be saved from the UI and no Site Grabber project could ever be created from the UI. Both fixed to camelCase keys. (Claude's earlier claim that linkId were the only two snake_case invoke keys was wrong.) The 7 stuck `pending` rows (4× mov_bbb video, 3× El Kalam browser) will be offered one at a time by the existing auto-open queue once the invoke works; no-spam is enforced by the item-(6) `failedCaptureIds` set, covered by the `bannerErrors.test.ts` "seven stuck captures" test.
- **(2) Categories:** root cause PROVEN — `CategorySidebar.tsx:169-171` called THREE handlers per click (`onSelectCategory(c.id)` + `onSelectQueue(null)` + `onSelectFilter('all')`); each App.tsx handler clears the others, React batches the setStates so the LAST write wins → selectedCategory ends null → "All downloads" stays highlighted, table stays full. Same bug broke queues (~300-302). Fix: each click calls EXACTLY ONE handler — All downloads → `onSelectFilter('all')`; category row → `onSelectCategory(c.id)` only; queue row → `onSelectQueue(q.id)` only. Selection state + handlers extracted from `App.tsx` into new `app/src/lib/useSidebarSelection.ts` (`useSidebarSelection` hook + pure `selectVisibleRows`), used by both App.tsx and the test — so the test drives the real App selection functions. New `app/src/components/sidebar_selection.test.tsx` (4 tests, passing): category click moves `aria-current` + filters rows by `category_id`; queue click shows queue files + `aria-current` on the queue; All downloads shows everything; each click calls exactly one handler. **The test pins the bug: with the old triple-call restored, 3/4 tests fail; with the fix, 4/4 pass.** Installed vitest 2 + jsdom 25 + @testing-library/react 16 as devDeps; new `app/vitest.config.mts` (jsdom, `src/**/*.test.{ts,tsx}`) and `test:ui` script (`vitest run`). Category ids, `list_downloads`, drag-drop untouched.
- **(3) Clipboard (sequence number):** `GetClipboardSequenceNumber()` (windows-sys 0.61, `Win32_System_DataExchange`) is the change signal. Engine (`app/engine/src/clipboard.rs`, `manager.rs`): `check_clipboard_offer(&self, text, sequence: Option<u32>)`. DROPPED the same-text hash (`last_text_hash`), the session-long `seen` set (`remember()`), and the `url_exists` hard block. `ClipboardMonitor` now holds only `last_sequence: Option<u32>`; the sequence is recorded even when the text is not a link (a non-link clipboard is not re-read every poll). An already-downloaded URL is offered with `already_in_list: true`. New engine methods `clipboard_sequence_seen(seq)` (shell read gate) and `note_clipboard_sequence(seq)` (record after our own writes). `dismiss_clipboard_offer` REMOVED (engine method, shell command, frontend wrapper). Shell (`app/src-tauri/src/lib.rs`): poll loop reads the sequence first; unchanged → skip the read entirely (no arboard open, no RDP channel cross); on read failure nothing is recorded so the next tick retries; on success sequence + text go to the engine. New command `note_own_clipboard_write` (reads the OS sequence, records it in the engine). Frontend: the offer opens the Add URL dialog directly in capture mode — `ClipboardOfferDialog.tsx` DELETED, `clipboardOffer` state removed; `onClipboardLink` listener sets `clipboardCapture` + `setShowAdd(true)` with a `clipboardCaptureRef` guard (one at a time, never a second window for the same URL). `AddDownloadDialog` gained an `alreadyInList` prop rendering "This link is already in your list — you can download it again or cancel." in `text-warning`. `UpdateDialog.tsx` calls `noteOwnClipboardWrite()` after `navigator.clipboard.writeText(url)` (the only app-UI clipboard write). TS `ClipboardOffer` gained `already_in_list: boolean`. Rewrote `app/engine/tests/clipboard_tests.rs`: 17 tests, all passing (detection tests pass `None` as sequence; new sequence tests: same sequence across 5 polls → offered once; same text with new sequence → offered again; recopied after other text → offered again; `clipboard_sequence_seen`; own-write sequence not offered; already-downloaded → offered with flag).
- **(4) Popup scan (aggregate, NOT frameId:0-only — videos can live in iframes):** `app/extension/background.js` — in-memory `tabVideoScans` map keyed by tab + frameId (`recordFrameScan`, `tabVideoAggregate`). `videos_found` records per frame and writes the SUM to `chrome.storage.local.lastVideoScan` (a 0-video frame can never overwrite a non-zero aggregate); logs which frameId answered (`console.debug`, counts only, never a URL). Frame 0 reporting a new page URL clears that tab's frames (navigation reset); tab removal clears via `chrome.tabs.onRemoved` (guarded — the test fake has no `onRemoved`). `scanActiveTab` now ignores the first-wins `sendMessage` reply, waits ~400 ms for frames to report, then returns the aggregate sum. No `webNavigation` permission exists, so frames are enumerated via their `videos_found` reports, not `getAllFrames`. `popup.js` needed no change (it already renders the reply/storage, which now carry the sum). Tests: `background.test.js` 13/13 (was 9; updated the rescan test to simulate the frame's `videos_found` + 600 ms wait, added 4 aggregate tests: iframe-first-0 → shows 1; two frames → sum; 0-video frame never overwrites; navigation resets). `popup.test.js` untouched for this item.
- **(5) Connected (Named Mutex `Local\FetchlineAppRunning`):** Engine (`app/engine/src/native_host.rs`): `APP_ALIVE_MUTEX_NAME` const (single source of truth) + `mutex_name_wide()` helper; `AppAliveMutexGuard` (closes handle on drop); `create_app_alive_mutex()`; `app_is_alive() -> Option<bool>` (`OpenMutexW` with `SYNCHRONIZE` = `0x0010_0000`; `Some(true/false)` on Windows, `None` off Windows). `HostReply` gained `app_alive: Option<bool>` (skip-if-none); Ping handler sets it. Engine `Cargo.toml` gained target-`cfg(windows)` `windows-sys 0.61` (features `Win32_Foundation`, `Win32_System_Threading`); `app/engine/Cargo.lock` now contains windows-sys 0.61.2. Shell (`app/src-tauri/src/lib.rs` `run()`): mutex created at the very top of `run()` (before the panic hook, before any window exists), moved into the setup closure (now a `move` closure) and held via `app.manage(guard)` for the process lifetime. Second launch opens the same mutex and gets its own handle; single-instance exits that process, closing only its own handle. `app/src-tauri/Cargo.toml` gained target-`cfg(windows)` `windows-sys 0.61` (features `Win32_Foundation`, `Win32_System_DataExchange`, `Win32_System_Threading`). NOTE: `app/src-tauri/Cargo.lock` does NOT yet contain windows-sys 0.61.2 — the VPS build will resolve it (normal for a new dep). Popup (`app/extension/popup.js` `refreshStatus`): Connected ONLY when `reply.app_alive` is not false; when `app_alive === false` shows exactly "Sonic Manager is closed - open it to catch downloads". Absent `app_alive` (off Windows / old host) keeps the old behaviour (host answer = Connected). Tests: `popup.test.js` 10/10 (was 8; +2 app_alive tests). Engine test added in `extension_tests.rs`: `host_ping_app_alive_follows_the_mutex` (`#[cfg(windows)]`, serialised with a static mutex; absent → `Some(false)`, held → `Some(true)`, dropped → `Some(false)`); `host_ping_and_get_settings` asserts `app_alive` is `None` on non-Windows. The Windows test compiles out on Linux — it runs only on a Windows build. windows-sys 0.61 API verified from the vendored source (`~/.cargo/registry/src/*/windows-sys-0.61.2`): `GetClipboardSequenceNumber() -> u32` (`Win32::System::DataExchange`); `CreateMutexW`/`OpenMutexW` (`Win32::System::Threading`); `CloseHandle` (`Win32::Foundation`); `PCWSTR = *const u16`; `w!` returns `*const u16`; `windows_link::link!` generates plain `extern` fns so all calls need `unsafe`; there is NO bare `SYNCHRONIZE` const in Threading (hence the `0x0010_0000` literal). Raw pointers are Send+Sync, so the guard can live in managed state.
- **(6) Banner:** new pure module `app/src/lib/bannerErrors.ts` (`BannerError {source, message}`, `BannerDismissals`, `FailedCaptures`) + `app/src/lib/bannerErrors.test.ts` (5 tests, passing). App.tsx: `bannerError`/`error` are now `BannerError | null`; dismissals keyed per (source, message); sources are `poll:downloads`, `poll:queues`, `poll:queue-files`, `poll:categories`, `queue-action`, `capture`, `action`. A successful poll clears ONLY its own source's dismissal (`clearPollErrorForSource`) and only a poll error from that source — a dismissed capture failure is never un-dismissed by an unrelated poll. `setPollError(source, msg)` / `showBannerError(source, msg)` gate on the dismissal map; `actionError` → `showBannerError('action', …)`. Auto-open effect: failed ids go into `failedCaptureIds` and are never retried (the old code deleted the id from `seenCaptureIds`, so the next poll re-queued it and the banner looped every second); a failure sets `failedCapture` state and a sticky `capture` banner. The banner shows a **Retry** button for capture failures ("Capture failed — Open": clears the id from `failedCaptureIds`, forgets the `capture` dismissal, re-queues the capture at the front); dismissing the banner also drops the Retry (the link can still be re-sent from the browser as a NEW pending id, which opens normally). A per-row capture action was not feasible — there is no captures-list UI in the main window; the banner Retry is the feasible surface. Successful auto-open clears a sticky `capture` error. Successful `run()` action still clears `bannerError` (Round 55 behaviour kept).
- **(7) Cancel dialog:** `app/src/components/ConfirmDialog.tsx` restyled to the Sonic theme: `bg-bg` (= #0B0F24 via `--color-bg`), neon gradient top bar `from-[#8B20FF] via-[#F02DCD] to-[#00C8FF]` (purple→pink→cyan, 2px, same pattern as DownloadCompleteDialog/DownloadErrorDialog/StandaloneAddUrlWindow), fixed `w-[420px] max-w-[92vw]`, `overflow-hidden` wrapper with padded inner content. Safe button (cancelLabel — "Keep downloading" on the cancel-download confirm in `ProgressWindow.tsx:940`, "Keep it" elsewhere) gets `autoFocus` so Enter keeps the download. Fixed size kept (no drag/resize); Escape/X/Cancel close unchanged. `Button` accepts `autoFocus` (extends `ButtonHTMLAttributes`).
- **Verification (final):** engine suite **485/485, 0 failed** (27 "test result: ok" lines; 483 after Round 74 + 2 new clipboard tests — the Windows-only mutex test adds +1 on Windows; `cargo test` exit 0, ~8.7 min, `/tmp/fetchline-test-*` cleaned before/after). Engine clipboard tests 17/17. Engine `extension_tests` host_ping: 1 passed on Linux (non-Windows `app_alive: None` fallback). Extension Node tests **69/69** — background 13, content 11, popup 10, video-detect 17, watcher 18 (was 63/63: +4 background, +2 popup). Manifest validators pass (`validate_manifest.py`, `validate_host_manifest.py`). Host tests **4/4** (unit 2/2 + spawn 2/2). Frontend Node tests: `apiKeys.test.mjs` 2/2; `speedGraph.test.mjs` 8/8. Vitest UI tests **9/9** (`npm run test:ui`: sidebar_selection 4 + bannerErrors 5). `npm run build` passes (tsc + vite, 1640 modules) — one mid-gate fix: `SidebarFilter` import became unused in App.tsx after the hook extraction (TS6133), import removed. `cargo fmt` on changed Rust regions: clean (introduced lines reformatted to rustfmt's preference); pre-existing drift left alone and NOT reformatted (`antivirus.rs`, `manager.rs` imports, `lib.rs` chains, `extension_tests.rs` imports — a whole-crate `cargo fmt` would create large unrelated diffs). **NOT verifiable on this VM:** the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile of all the new Windows FFI (clipboard sequence, mutex create/probe, `move` setup closure) and of the shell changes generally; the Windows-only mutex test also only runs on Windows. NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): (1) extension capture opens the Add URL window (the 7 stuck pending rows offer one at a time, no spam, no block); queue schedule saves from the UI (was silently broken before); a Site Grabber project can be created from the UI (was silently broken before); (2) clicking a category/queue/All downloads highlights the right row and filters the table; (3) clipboard end-to-end — copy a link → Add URL window opens directly (no small dialog); copy the SAME link again → offered again; copy an already-downloaded link → Add URL opens with the "already in your list" note; copy from the app UI (Update dialog "Copy link") → no offer; 5 polls with an unchanged clipboard → one offer only; (4) popup on a page with videos in an iframe shows the total; a 0-video iframe does not zero the count; navigating resets the count; (5) popup shows "Sonic Manager is closed - open it to catch downloads" with the app closed and Connected with it open; a second launch still focuses the running instance (mutex handle not closed); app crash → popup flips to closed; (6) dismiss a capture-failure banner → it stays gone (no 1 s loop); Retry re-opens the capture; a dismissed capture failure is not un-dismissed by a successful downloads poll; (7) Cancel dialog is Sonic-themed, fixed 420px, Enter keeps the download.

## Round 75b — VPS build fix: AppAliveMutexGuard Send + Sync — 2026-11-19

- **VPS build FAILED ~15:55 EET (retest NOT run):** git pull OK (head 49e1b799), host build PASS, frontend PASS, but the Tauri Rust compile failed with 2x E0277 — `AppAliveMutexGuard` (engine/src/native_host.rs:241) wraps a raw windows-sys HANDLE (`*mut c_void`), neither Send nor Sync, but `app.manage(guard)` (app/src-tauri/src/lib.rs:3207) requires `T: Send + Sync + 'static`. The Tauri app crate cannot compile on this VM (no gdk-3.0), so the VPS build was the first real compile of the new Windows FFI — local engine tests passed because they never call `app.manage`.
- **Fix (2 lines, engine/src/native_host.rs, after the Drop impl):** `#[cfg(windows)] unsafe impl Send for AppAliveMutexGuard {}` + `#[cfg(windows)] unsafe impl Sync for AppAliveMutexGuard {}`. Sound because the guard is just an owned Windows HANDLE (a process-wide kernel-object value): moving it between threads is safe, sharing &references is safe because the only thing that touches the handle is Drop (needs &mut self), and the guard is never cloned/copied so CloseHandle runs exactly once. The compiler reported exactly 2 errors, both the same call failing the two bounds — no other errors were hiding behind them.
- **Local verification:** `cargo check -p fetchline-engine` passes; full engine suite re-run (expect 485/485). The VPS build is the real proof for the Tauri part.
- **Retest:** same 7-item Round 75 checklist + remember-path repro add-on + Round 74 tail, on a fresh RDP session (the 75 session expired).

## Round 76 — user's 6 findings + Escape + Download-removed restyle — 2026-11-19

Approved by the user ("تمام نفذ", ~19:06 EET). 8 items, all from his own test of the Round 75b build on the server, plus the retest's Escape finding and his Download-removed preview. No push yet — local verification only; push + VPS retest follow.

- **(1) Add URL window (capture/clipboard ONLY) — title + drag:** `app/src/components/StandaloneAddUrlWindow.tsx` — the TitleBar is now `title={filenameFromUrl(link.url) || 'Add URL'}` (the pending row carries no filename, so the URL's last segment) and `draggable` (was `draggable={false}`). `filenameFromUrl` is now exported from `app/src/components/AddDownloadDialog.tsx` instead of duplicated. The NORMAL manual Add URL (`AddDownloadDialog.tsx`, toolbar) is untouched — still "Add URL". No Rust change: the OS window title was already the filename (`lib.rs` build_add_url_window).
- **(2) Remember path — the stale-categories cause:** `app/src/App.tsx` — the main Add dialog's `onAdded` now calls `void refreshCategories()` (the submit already stored the folder via `updateCategory`; the dialog's autofill reads `categories` state, which `refreshAll()` never refreshed — the next dialog showed the old folder). The submit runs before `onClose()` in the same flow, so this also covers close-after-submit. `app/src/components/StandaloneAddUrlWindow.tsx` — refetches categories on window `focus` (a window focused from an earlier mount held that mount's categories). `app/src/components/AddDownloadDialog.tsx` — a failed `updateCategory` now calls `onLateError(...)` instead of the silent `catch {}` (main-window banner / standalone-window alert; the download still starts). The checkbox stays disabled with no category picked (by design).
- **(3) Download Complete dialog race — close made authoritative, both ways:** `app/src-tauri/src/lib.rs` `open_complete_dialog` now closes the `progress-<id>` window first (via the existing `progress_window_label` + `close()` idiom) before building or focusing the dialog — the old two-poll race left both windows open, deterministically for an already-complete download. `app/src/components/ProgressWindow.tsx` — the completion effect now closes whenever the status IS complete and the dialog is enabled, not only on a downloading/paused → complete flip; the shutdown / close-after options still need a flip (they were chosen for a run that already ended). The effect also re-runs when the settings load, so a complete status that arrived before them still closes. `app/src/components/DownloadCompleteDialog.tsx` — `draggable` in both TitleBar branches (was `draggable={false}`).
- **(4) Double scrollbar — progress window:** `app/src/components/ProgressWindow.tsx` — the outer content column is now `overflow-hidden` (was `overflow-y-auto`); the parts table keeps its own `max-h-[140px] overflow-y-auto` scroller. The footer (Pause/Cancel/Open) sits outside the column, so it is unaffected. Caveat for the retest: with the outer scroll gone, the retry box + full parts table in one view may clip at the bottom of the fixed 660x480 window — check the retrying state.
- **(5) Free space — faster:** `app/src/App.tsx` — the poll is now every 10 s (was 30 s), plus a downloads-signature effect that refreshes the figure whenever the list changes shape (a row vanishing or changing status — cancel / finish / remove / delete completed). The signature ignores byte progress, so the 1 s poll does not spam the drive. Note: the main window has no direct cancel path (cancelling happens in the progress window, which deletes the partial file), so the refresh is driven by the observed row change, not a cancel call.
- **(6) Delete Completed — verified, no confirmation:** the filter is `selectCompletedDownloads` (`app/src/lib/useSidebarSelection.ts`, new pure helper: `status === 'complete'` only), now used by `handleDeleteCompleted` in `app/src/App.tsx`. NO confirmation dialog was added, per the user's decision — as long as it deletes completed-only it executes directly. Scope is the WHOLE list, not just the visible view (a finished download in another category/queue is completed too — accepted, unchanged). New test `app/src/lib/delete_completed.test.ts` (3 tests): only complete rows from a mixed list of all six statuses; complete rows across three categories all selected; empty / no-complete selects nothing.
- **(7) Escape closes the Add URL dialog:** `app/src/components/StandaloneAddUrlWindow.tsx` — a window-level Escape listener calling `handleClose()`, with a `closingRef` guard (the dialog's own handler also calls `onClose`, so one press would otherwise resolve the capture as rejected twice). Root cause found while implementing: the dialog's Escape handler only exists while the dialog is mounted — during "Loading captured link…" (or the load error) there is no dialog, so Escape did nothing. The in-app Add dialog already had its own Escape handler (`AddDownloadDialog.tsx`: Escape closes unless a submit is in flight) — verified present, unchanged.
- **(8) Download-removed restyle — approved preview (`workspace/download-removed-preview.html`):** `app/src/components/ProgressWindow.tsx` `!download` branch — the icon is now a 64 px pink circle (`border-danger bg-danger-soft text-danger`, glow `rgba(240,45,205,0.35)` — `danger` IS pink #F02DCD in the theme) with a 32 px FileX, and Close is the purple→pink gradient primary (same inline style as the complete dialog's Open button). The title bar shows the LAST-SEEN filename (`lastFilename` state, set on every poll that returns a row; fallback "Sonic Manager" when no poll ever returned a row, e.g. the poll-error case). The filename appears ONLY in the title bar — the body text is byte-identical to before. The Cancel confirmation dialog (`ConfirmDialog "Cancel this download?"`) is NOT touched — the user explicitly said that is not the one.
- **Local verification:** engine suite **485/485** (full `cargo test -p fetchline-engine` run, exit 0; `-- --list` confirms 485 tests); extension Node **69/69**; UI Vitest **12/12** (sidebar_selection 4 + bannerErrors 5 + delete_completed 3, new); `npm run build` (tsc + vite) passes. `cargo fmt --check` on `app/src-tauri` shows only pre-existing drift (lines 168–2232, none in the Round 76 hunk) — left untouched. The Tauri app crate cannot compile on this VM (no gdk-3.0): the `open_complete_dialog` Rust change uses only established idioms from the same file (`get_webview_window`, `close()`, `log_startup`, `progress_window_label`) — the VPS build is the first real compile of it.
- **VPS retest checklist:** (1) capture/clipboard Add URL window shows the filename in its title bar and drags by it; manual Add URL still says "Add URL"; (2) remember-path end-to-end: pick a category + check the box + custom folder → next Add URL for that category pre-fills the remembered folder (clipboard AND extension); a failed save surfaces a banner/alert; (3) download completes → ONLY the complete dialog shows (progress window gone), including a download that was already complete when its progress window opened; complete dialog drags; (4) progress window shows ONE scrollbar (parts only) at full parts + while retrying; (5) cancel a download → status-bar free space updates within ~10 s (and on the next row change); (6) Delete Completed with a mixed list (complete rows in several categories + downloading/paused/error rows) → only the complete rows vanish, no confirmation; (7) Escape closes the standalone Add URL window while loading, while loaded, and after an error; Escape still closes the in-app Add dialog (not while submitting); (8) cancel → progress window turns into the Sonic-identity "Download removed" card with the filename in the title bar only and the same body text; poll-error state still falls back to "Sonic Manager".

## Round 78 — yt-dlp integration (YouTube, Facebook, Instagram, TikTok) — 2026-11-19

Approved by the user ("تمام نفذ", ~22:40 EET). Implemented from the merged plan in `your_files/sonic-manager-ytdlp-plan-second-opinion.md` §9 (extension subagent: 107/107 Node tests; resolver module `app/engine/src/ytdlp.rs` delivered separately; engine integration + host + Tauri + frontend in this session). No push yet — local verification only; push + VPS build/retest follow.

- **Engine — resolver + integration:** `ytdlp.rs` (resolve with 30 s timeout, tokio kill-on-drop, CREATE_NO_WINDOW; format rules progressive-first → pair → fragmented, https only; per-host caps googlevideo 8 / fbcdn 4 / tiktokcdn 2; SABR warnings parsed, one internal retry; error map with `YtDlpErrorKind` + `user_message()`; `quality_rows` one row per height 360p+; `select_quality`; `sanitize_title_filename`; `default_host_segment_cap`; `write_cookies_tempfile` Netscape 0o600 + RAII delete + `sweep_stale_cookie_files`; `build_ytdlp_args` with `--ignore-config --no-playlist --` + `--js-runtimes quickjs:<path>`). NEW in this session: `clean_windows_path` (strips `\\?\` / `\\?\UNC\`, same as lib.rs) used for the qjs path; `YtDlpPaths` gains `ffprobe`; `sha256_hex` + `parse_sha256sums` (pure, unit-tested); `ytdlp_version` (`--version`, 10 s timeout, None when missing); `update_ytdlp` (downloads yt-dlp.exe + SHA2-256SUMS from the GitHub `latest/download` release, installs ONLY on SHA-256 match into the AppData bin dir via a `.new` file + rename, honours the user's proxy, 5 min total timeout). `sha2` moved from dev-dependencies to dependencies (update verification ships in the app). Schema/store: downloads gain parent/child DASH columns + `source_url`/`format_id`/`direct_ytdlp`; pending_links gain `filename`/`headers`/`source_url`/`format_id`/`audio_url`/`fragmented`; settings gain `platform_downloads_enabled` (default 1) + `ytdlp_extractor_args` (hidden override). `row_to_pending_link` index bug found by the full suite (created_at is index 8, not filename) and fixed — all 18 extension_tests failures were that one shift. Manager: per-download header map into probe + segments, `probe_url_for_capture_with_headers`, `headers_from_json` (drops Cookie keys), `proxy_config()` accessor, DASH parent + two children + ffmpeg merge supervisor, (B) `run_ytdlp_direct` now passes `--ffmpeg-location <exe_dir>` (only when ffmpeg AND ffprobe are present) and `--js-runtimes quickjs:<path>` — without them yt-dlp cannot find the bundled tools.
- **ffprobe — VERIFIED (the open question):** YES, the (B) direct yt-dlp fallback needs ffprobe as well as ffmpeg. yt-dlp's own README: "ffmpeg and ffprobe — Required for merging separate video and audio files"; the runtime error is literally "ERROR: Postprocessing: ffprobe and ffmpeg not found". The bundle therefore ships `ffprobe.exe` too (BtbN LGPL-shared includes it). The engine's own DASH merge (`merge_dash_files`, plain `ffmpeg -c copy`) does NOT need ffprobe — only yt-dlp's merge does. `YtdlpStatus.ffmpeg` in Settings means "ffmpeg AND ffprobe both present".
- **Host:** `native_host.rs` gains `HostMessage::ResolveVideo` (`{"type":"resolve_video","page_url","request_id","with_cookies","cookies","extractor_args"}`) + `HostReply` fields (`request_id`, `title`, `is_live`, `degraded`, `warning`, `formats` (trimmed `QualityRow` list, never raw `-J`), `needs_login`, `error_kind`); the sync `handle_host_message` refuses ResolveVideo with an error ("use the async host path"); `enqueue_resolved` stores resolver metadata on the pending row (Cookie keys stripped from headers, defence in depth) and routes resolved variants in the VideoFound arm. `app/host/src/main.rs`: intercepts ResolveVideo before the sync handler, runs the resolve with a `resolve_progress` heartbeat every 5 s on the same stdout (dropping the future on a closed stdout kills yt-dlp via kill-on-drop), sweeps stale cookie temp files at startup, resolve is store-optional (platform flag enforced when the DB opens). Host tests 4/4 unit + 2/2 spawn (new: trimmed-rows reply + error-kind/login-flag reply).
- **Tauri:** new commands `ytdlp_version` (no args), `ytdlp_resolve` (app-side resolve, no cookies, honours the stored extractor-args override + platform flag), `ytdlp_update` (SHA-256-verified, returns fresh status) — all registered in `invoke_handler`. `probe_url` gains optional `headers` (resolver headers JSON). `tauri.conf.json`: `externalBin` += `binaries/yt-dlp`, `binaries/ffmpeg`, `binaries/ffprobe`, `binaries/qjs` (packaging must place them as `<name>-x86_64-pc-windows-msvc.exe` in `src-tauri/binaries/` — Tauri strips the triple at bundle time, so they land as plain `*.exe` next to the app exe, which is what `locate_sidecars` looks for; the ffmpeg shared-build DLLs ship via `bundle.resources` and land next to the exe on Windows). `THIRD_PARTY_NOTICES.md` at the repo root (yt-dlp/Unlicense, QuickJS-ng/MIT, BtbN FFmpeg LGPL-shared — version + SHA-256 slots to fill at packaging; gyan.dev explicitly forbidden) is bundled as a resource and referenced from Help > About.
- **Frontend:** `api.ts` `probeUrl(url, cookies?, headers?)` + `ytdlpVersion()` / `ytdlpResolve(url)` / `ytdlpUpdate()`; `types.ts` Settings gains `platform_downloads_enabled` + `ytdlp_extractor_args`, `PendingLinkRow` gains the six resolved fields, new `YtdlpStatus` / `QualityRow` / `YtdlpResolveResult` types. `AddDownloadDialog` gains `initialHeaders` (rides the size probe) and prefills the filename from `captureTitle` at once, marking it so the probe never overwrites it with the URL's last segment ("videoplayback" for googlevideo links). `StandaloneAddUrlWindow` passes the row's `filename` as `captureTitle` + `headers` as `initialHeaders`, and its title bar prefers the row filename over `filenameFromUrl`. `SettingsDialog` General tab: "Download videos from YouTube, Facebook, Instagram and TikTok" toggle (gated on browser integration) + "Video component (yt-dlp)" section showing version + solver/merge-tool presence, missing-tool hints, and an "Update yt-dlp" button with result text. `ytdlpResolve` is exposed but not yet wired into the Add URL dialog (pasted-YouTube-URL flow) — next step, not this round.
- **Local verification:** engine suite **520/520, 0 failed** (full `cargo test`, exit 0, ~10 min; lib 85 incl. 6 new: sha256/parse_sha256sums/clean_windows_path/ytdlp_version-None + 2 progress-line tests; new `tests/ytdlp_tests.rs` 6/6: old-DB migration round-trip, resolved host hand-off + metadata, platform-off refusal, bad-URL refusal, headers parsing, DASH merge with system ffmpeg 8.1.2). Host **6/6**. Extension Node **107/107**. UI Vitest **12/12**. apiKeys guard + speedGraph **10/10**. Manifest validators OK. `npm run build` (tsc + vite) passes. `cargo fmt` clean on all changed regions (pre-existing drift elsewhere left alone). **NOT verifiable on this VM:** the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile of `ytdlp_version` / `ytdlp_resolve` / `ytdlp_update` and the `probe_url` change; the new Tauri code was reviewed line-by-line against the engine API instead (including one real catch fixed in review: a partial move of `video.warnings` before `quality_rows(&video)`). No live YouTube resolves were attempted here (egress proxy blocks them; no binaries downloaded — packaging fetches them).
- **VPS step (left for the parent):** build (host + app) → QuickJS acceptance test (~20 varied YouTube URLs, cold starts, `-v`, BOTH `quickjs` and `quickjs-ng` provider names, p50/p95 resolve time vs a Deno run on the same set; ship QuickJS on zero solver failures + acceptable times, else Deno) → extension live test on TikTok/Instagram/Facebook public videos first, then YouTube (all qualities 360p+, DASH pair + merge, (B) fallback on a fragmented format, SABR-degraded message, login-gated video + consent flow, update button end-to-end). Packaging checklist: pin BtbN branch (n8.1 or n9.0) win64-lgpl-shared, verify `ffmpeg -L` (reject `--enable-gpl`/`--enable-nonfree`/fdk-aac), fill THIRD_PARTY_NOTICES version + SHA-256 per binary, ship FFmpeg LICENSE.txt, sign the installer + submit binaries to Microsoft for AV analysis.

## Round 79 — fix round for the two Round 78 YouTube failures + unresponsive yt-dlp Update button — 2026-11-20

Approved by the user ("نفذ", ~05:35 EET). Fix round for the Round 78 VPS retest with login (2026-11-20 ~03:55 EET): (1) the extension's "Couldn't load download formats" even logged in to YouTube, (2) manual Add URL downloading the watch-page URL instead of the resolved video, (3) the "Update yt-dlp" button showing no progress. No push yet — local verification only; push + VPS build/retest follow.

- **(1) Extension resolve + cookie consent (`app/extension/content.js`, `background.js`):** root causes found in code, not assumed. The first resolve always ran WITHOUT cookies: the click handler never read the stored per-site consent before the first attempt, so a login-gated video failed before consent was ever asked (and being logged in to YouTube did not help, because no cookies were sent). "Try again" then repeated the identical failing request. And a host error reply with no `request_id` was dropped by the background port handler, so the resolve hung to the 32 s timeout instead of surfacing the real error. Fixes: `openResolveDropdown` is now async — it renders "Checking…" immediately, reads `ytdlp_cookie_consent:<page host>` from `chrome.storage.local` FIRST, and starts the first resolve with `withCookies: true` + the page cookies when a grant is stored (no doomed cookieless attempt first; no stored grant → the old cookieless-first behaviour is unchanged; if the dropdown is closed while the storage read is in flight, no resolve starts). New `lastResolveAttempt` state (`{video, error, withCookies, cookies}`), recorded by `recordResolveFailure`; the retry button now calls `retryResolve` — if the last error had `needs_login` and the attempt was cookieless it runs the consent flow (shared new `askCookieConsent` helper, extracted from `handleResolveError` with the confirm wording unchanged — a stored grant answers silently, a session denial is never re-asked, otherwise `confirm()` once) and retries with cookies if granted, else re-shows the error; otherwise it retries with the SAME cookies the failed attempt used. A cookied first attempt that also fails with `needs_login` shows the error with no loop and no second ask. `handleResolveError` itself now uses the same helper, so its behaviour is unchanged. `background.js` `startNativeResolve`: a message with `type === "error"` and a missing/null `request_id` is now delivered as the pending request's error (forwarded unchanged, which surfaces the stale-host "Could not understand the message…" text immediately); a present-but-different `request_id` is still ignored, and an id-less `resolved` is still dropped. The 32 s timeout, resolve cache, per-site consent rules (only when needed, only to that site, max 32 cookies, temp file deleted after use) and the host reply contract are all untouched. Tests: extension Node **111/111** (was 107; +5 new — stored grant → first resolve is cookied, and a cookied `needs_login` failure shows the error with no retry/loop; "Try again" after a denied `needs_login` asks nothing and opens no new port; "Try again" after a cookied plain-error failure retries with the same cookies; "Try again" after a `needs_login` denial retries with cookies once a grant is stored; id-less host error delivered as the pending error; mismatched-`request_id` replies and id-less `resolved` still ignored). One existing assertion updated: "no-cookies-first" now reads as cookieless-first only when no consent is stored. `node --check` clean on both edited files.
- **(2) Engine error mapping (`app/engine/src/ytdlp.rs` `map_error`):** bot-check needles FIRST — "confirm you're not a bot", "not a bot", "login_required", "playability status" → `LoginRequired` — before the Private rule. A bot-blocked video no longer masquerades as private (which is why the retest's failure never reached the cookie-consent flow); a genuinely private video that also mentions signing in still maps to Private. Tests added in `ytdlp.rs` (3 bot-check cases + 1 private-with-sign-in case).
- **(3) Host resolve logging (`app/host/src/main.rs`):** resolve failures are logged to host.log with kind + yt-dlp stderr tail; the resolve request is logged (page URL + with_cookies flag); `resolve_with_heartbeat` now takes log_path. This is what makes the next live failure diagnosable from the log instead of from a generic dropdown message.
- **(4) Manual Add URL wired to the resolver (Bug 2 — the core fix):** new engine method `DownloadManager::add_resolved_video_download(page_url, row, filename, folder, category_id, queue_id, speed_limit_kbps, start_now)` (`app/engine/src/manager.rs`): takes one `QualityRow` from `ytdlp_resolve` — its stream URL(s), headers, format id and fragmented flag — and creates the download from those, NEVER from the page URL (a watch-page URL probes size 0 with no ranges; the retest's row was "watch", 0 B, Error, "Server ignored the range request"). Branches mirror `confirm_pending_link`: fragmented → the (B) direct yt-dlp download (no probe, no segments); DASH pair (`audio_url` present) → the video stream is the parent and the audio child is created eagerly via `ensure_dash_children` — even when saved for later — so a later start finds the pair; otherwise (progressive) → a plain segmented download of the stream URL with the row's headers. Resolver metadata (page URL + format id) is recorded via `set_download_ytdlp_meta`, like the confirm path, so a 403/410 can re-resolve; the speed cap is applied before the start. Returns the created download id and its probe info. New Tauri command `add_resolved_video_download` (`app/src-tauri/src/lib.rs`, registered in `invoke_handler`) + `api.ts` wrapper `addResolvedVideoDownload`. Frontend (`app/src/components/AddDownloadDialog.tsx`): new exported helpers `isVideoPageUrl` (YouTube watch/shorts/live/embed/v + youtu.be, TikTok, Instagram, Facebook — mirrors the extension's `isSupportedPlatform`, with a path hint on YouTube so a channel page is not resolved) and `bestQualityRow` (the last row — rows are one per height, sorted ascending). The debounced probe (500 ms, same reqId guard) now resolves instead of probing when the pasted URL is a video page: on success it pre-fills the filename from title + best-row ext, shows the quality line (`1080p · MP4 · merged`) + size + SABR warning, and caches the result keyed by URL (`resolvedForUrl`); on failure it shows the real error muted ("Could not resolve this video — Start will try again") and stores nothing. Submit: uses the cached resolve when fresh, otherwise re-resolves; refuses live streams and empty format lists with a clear error; a resolve failure throws — the error is shown (pink alert + `onSubmitError`) and NO download is created. Capture mode (`pendingLinkId`) is untouched — already-resolved rows still go through `confirmPendingLink`. Non-video URLs keep the existing probe + add path unchanged. Clearing the URL invalidates any in-flight resolve/probe (reqId bump). `ensure_dash_children` was extracted from `start_dash_download` (creates or reuses the video+audio children; linking sets the parent's `merge_status='pending'`), and `confirm_pending_link` now calls it eagerly too — the same defect class as Bug 2's Download Later case. Engine tests added in `app/engine/tests/video_tests.rs` (3): `resolved_video_download_uses_stream_url_not_page_url` (mock stream server; asserts the download URL is the stream URL, the filename is the resolved one, `source_url`/`format_id` are recorded, and the download completes byte-exact); `resolved_dash_pair_creates_audio_child_even_when_saved_for_later` (both children exist before any start, parent's merge pending, parent not started); `resolved_video_download_failure_creates_no_download` (unreachable stream URL → error, zero download rows). One test correction during verification, code unchanged: `merge_status='pending'` lives on the PARENT row (set by `link_child_to_parent`), not on the children — the test initially asserted it on the audio child. UI tests added in `app/src/components/add_download_dialog.test.tsx` (9): `isVideoPageUrl` routing (watch/shortsyoutu.be/TikTok/IG/FB true; channel page/plain file/ftp false), `bestQualityRow` picks the highest row, video submit resolves and adds the best row's stream URLs (addDownload and startDownload never called — the engine starts it), resolve failure shows the real error and creates no download, live stream refused with no download, non-video URL probes and adds normally and never resolves.
- **(5) "Update yt-dlp" button (Bug 3, `app/src/components/SettingsDialog.tsx`, frontend-only — no engine change, no signature change):** the handler already set "Updating…" + success/failure messages; the retest's "no visible progress" is most likely the slow ~23 MB download outlasting the ~10 s observation (or missed RDP clicks). It now sets "Checking for updates…" IMMEDIATELY on click (visible status text next to the button, not just the button label) and says "yt-dlp is already the newest version (X)." when the version is unchanged, by comparing the version before/after. UI tests added in `app/src/components/settings_dialog_ytdlp.test.tsx` (3): immediate status + "already the newest" on an unchanged version, new-version wording, failure shows the reason.
- **Local verification:** engine suite **523/523, 0 failed** (full `cargo test`, exit 0, ~8.9 min; 520 after Round 78 + 3 new; `/tmp/fetchline-test-*` cleaned before/after). Extension Node **111/111**. Host **6/6** (4 unit + 2 spawn). UI Vitest **24/24** (was 12/12; +9 dialog + 3 settings). `npm run build` (tsc + vite) passes. `cargo fmt` clean on engine, host, and the Tauri crate (one pre-existing host line reformatted by fmt; changed regions otherwise clean). **NOT verifiable on this VM:** the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile of the new `add_resolved_video_download` command (arg keys verified by hand against the camelCase rule: pageUrl/row/filename/folder/category/queue/speedLimitKbps/startNow; the `QualityRow` JSON the frontend sends is snake_case, matching the Rust struct fields). No live YouTube resolve was attempted here (egress proxy blocks them; no binaries downloaded). Live-browser behaviour of the extension fixes is verified only by the vm harness tests.
- **VPS step (left for the parent):** build (host + app) → retest checklist: (1) extension on logged-in YouTube — hover shows the button, click loads the qualities (360p-up, DASH pair + merge, SABR-degraded message where applicable); login-gated video → cookie consent asked once, then resolves; "Try again" after a failure does not repeat the identical request; (2) manual Add URL with a YouTube watch URL — resolves, shows the quality line, Start creates a download of the stream (not "watch", 0 B) and it completes; a bad/login-gated URL shows the real error and creates no row; (3) fixture server + normal download; TikTok/Instagram/Facebook; (4) "Update yt-dlp" — click shows "Checking for updates…" at once, then "already the newest version (2026.10.26)" (or the new version / the failure reason). Round 77 (path problem) still queued after Round 79.

## Round 80 — fix round for the Round 79 retest defects — 2026-11-20

Approved by the user ("نفذ", ~13:39 EET, "عاوز تسأل ولا هتنفذ" → no questions). Fix round for the Round 79 VPS retest (2026-11-20 ~13:36 EET, BUILD PASS, YouTube fixed): TikTok 403, Instagram URL rejection, Facebook resolve hang, ffmpeg DLLs missing from the build output, the main window hiding mid-session, the generic bad-URL error in the manual dialog, and first-click-only-focuses in dialogs. Round 77 (path problem) stays queued after Round 80. No push yet — local verification only; push + VPS build/retest follow.

- **(1) TikTok 403 → direct yt-dlp fallback (`app/engine/src/manager.rs` `add_resolved_video_download`):** cause proven in code (mechanism, not the CDN's exact header demand): the probe sends the row's headers (UA + Referer-if-present) via `probe_with_headers`; on a 403 HEAD it falls back to a ranged GET with headers, which also 403s → `EngineError::Status(403, "403 Forbidden while checking the address")`. 403 is NOT probe-retryable (only 429/503 in `is_probe_retryable`), so the identical repeat was the user retrying Start, not the probe looping. Segments DO carry the per-download headers (`headers_for`), so a probe pass implies segments would too. The exact header TikTok's CDN wants is NOT provable from this VM (no yt-dlp binary, no live TikTok) — so instead of guessing headers, the fix reuses the proven (B) mechanism: on probe 401/403 the add falls back to `add_direct_ytdlp_download` (yt-dlp re-extracts the page for the same format id with the format's full header set). A DASH pair falls back with the combined `"video+audio"` selector (`run_ytdlp_direct_inner` already runs `yt-dlp -f <format_id> <page_url>`, so yt-dlp merges the pair itself); DASH children are skipped for fallback rows and start routes to `start_direct_ytdlp`. Scope is 401/403 only — a 404, a timeout or an unreachable URL keeps today's behaviour (no row, the real error). Tests: 3 new in `video_tests.rs` (403 → fallback row with `direct_ytdlp=1`, page URL as `source_url`, format id kept, probe size None, not started; 403 DASH → `"137+140"` selector + zero children; 404 → error surfaces, no row).
- **(2) Instagram rejection — not an app bug (`app/src/components/AddDownloadDialog.tsx`):** proven by direct evaluation: the validation regex `/^https?:\/\/.+/i` accepts `https://www.instagram.com/reel/DdwixbhzY3_/` with and without a trailing slash. The retest's typed text had `https;//` (semicolon for colon) — exactly what the documented RDP Shift-key quirk produces (Shift+; → ";" while typing URLs). The rule was already correct, so it is unchanged; the fix is narrow robustness: `normalizeUrlScheme` repairs just that typo at input time (the field self-corrects) and `isHttpUrl` is exported so the rule is locked in by tests (the exact retest URLs, plus the semicolon typo rejected). No other validation path gates the URL.
- **(3) Facebook resolve hang — two real defects (`app/src/components/AddDownloadDialog.tsx`, `app/engine/src/ytdlp.rs`):** (a) a resolve succeeding with ZERO formats surfaced NO error — the debounced effect set the file size to null and never set `resolveError`, so the dialog showed no quality line and no error and looked hung (the Round 79 retest's `facebook.com/watch/` feed page). Now it sets a clear "No downloadable video was found on this page — it may be a feed or a page, not a video." error and clears the cached resolve, so Start re-resolves and throws the same message. The `isVideoPageUrl` gate was deliberately NOT tightened: a feed page is a valid Facebook URL and tightening risks rejecting real video URLs (`fb.watch`, `/videos/`, `watch?v=`). (b) The pipe-drain awaits in `resolve_once` ran UNBOUNDED after the 30 s timeout kill — a yt-dlp grandchild (JS runtime / ffmpeg) holding a pipe open hung the resolve forever even though the child itself was reaped. Both drains are now bounded to 5 s each (continuing with what arrived). The 30 s per-attempt timeout itself was already enforced with kill-on-drop.
- **(4) ffmpeg DLL packaging (`app/src-tauri/tauri.conf.json`):** proven in config: `bundle.externalBin` copies `ffmpeg.exe`/`ffprobe.exe` to `target/release`, but NOTHING copied the 7 FFmpeg DLLs (no `bundle.resources` entries, no `build.rs` copy — `build.rs` is just `tauri_build::build()`). `locate_sidecars` resolves ffmpeg from the exe dir, so the DLLs must sit next to `ffmpeg.exe`; without them the first resolve failed with DLL-not-found dialogs (the retest task copied them manually). Fix: the 7 DLLs added to `bundle.resources` (`binaries/avcodec-62.dll`, `avdevice-62.dll`, `avfilter-11.dll`, `avformat-62.dll`, `avutil-60.dll`, `swresample-6.dll`, `swscale-9.dll` → same names at the resource root, i.e. next to the exe). `tauri.conf.json` re-validated as JSON. NOT verifiable on this VM: this tree has no `app/src-tauri/binaries/` at all (binaries are downloaded on the VPS at build time) and the Tauri crate cannot compile here — the VPS build MUST verify the DLLs land in `target\release` next to `ffmpeg.exe` AND in the MSI/NSIS installers (if `--no-bundle` does not copy resources, a post-build copy step is the fallback).
- **(5) Window loss — no change (`app/src-tauri/src/lib.rs`):** verified in code, not assumed. The only main-window `hide()` is the designed CloseRequested → hide-to-tray (Round 65); tray restore (left-click and the Show menu item) does `show()` + `set_focus()` — correct, and the user verified tray restore PASS in the R76 retest. The retest task never tried the tray icon; AppActivate returning True without restoring is Windows behaviour for a hidden window, not an app bug. No code change; the designed tray behaviour is untouched.
- **(6) Generic bad-URL error in the manual dialog (`app/src/components/AddDownloadDialog.tsx`):** proven in code: the probe/resolve error lines rendered hardcoded generic strings ("Could not resolve this video — Start will try again") with the real error only in the `title` tooltip, so the retest saw the generic text and never the real yt-dlp error. Both lines now render the real error text visibly (truncated, full text in the tooltip). The submit path already showed the real message (`setError(msg)` + `onSubmitError`) and is unchanged. No existing test asserted the generic strings.
- **(7) First-click focus — no change, no proven cause:** checked in code: dialog buttons are plain `onClick` (no `autoFocus`, `onMouseDown`, or `preventDefault` anywhere in the dialog components), `data-tauri-drag-region` exists only in `TitleBar.tsx`, and dialog windows get `set_focus()` on show (`show_window_ready`). No focus-stealing or click-eating code exists to fix. Most likely a WebView2/RDP activation artifact (the first click activates the window/canvas). Deliberately NOT guess-fixed.
- **Local verification:** engine suite **526/526, 0 failed** (full `cargo test --release`, exit 0, ~15.5 min; 523 after Round 79 + 3 new; `/tmp/fetchline-test-*` cleaned before/after). Extension Node **111/111**. Host **6/6** (4 unit + 2 spawn). UI Vitest **28/28** (was 24/24; +2 `isHttpUrl` + 2 `normalizeUrlScheme`). `npm run build` (tsc + vite) passes. `cargo fmt` clean on engine, host, and the Tauri crate (two of the new engine lines reformatted by fmt). **NOT verifiable on this VM:** the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile of the `tauri.conf.json` change and of any Tauri-side effect; no live TikTok/Instagram/Facebook resolve was attempted here; the DLL landing in `target\release` is unverified until the VPS build.
- **VPS step (left for the parent):** build (host + app) → FIRST check the 7 ffmpeg DLLs are in `target\release` next to `ffmpeg.exe` (run `ffmpeg -version` from that dir with no manual copying) and in the installer → retest checklist: (1) TikTok video via Add URL (and extension where playable): the 403 must fall back to the yt-dlp download and complete — retry must not repeat the identical 403; (2) Instagram reel URL in Add URL: accepted, resolves, downloads; (3) Facebook feed URL in Add URL: a clear "no downloadable video" error, no hang; a real Facebook video URL resolves and downloads; (4) a bad YouTube URL shows the REAL yt-dlp error in the dialog and creates no row; (5) regression: YouTube extension + manual Add URL still complete (230 MB Rick Astley), yt-dlp Update button still shows status, fixture normal download still passes. Round 77 (path problem) stays queued after Round 80.

## Round 81 — fix round for the user's own-test defects (reproduced 2026-11-20 ~14:58 EET) + double title — 2026-11-20
Ordered by the user ("خلاص صلح كله مره وآحده" — fix 80 + 81 together, no questions). Implemented on top of Round 80, local only, NOT pushed. The double-title fix (R77 item a) was folded in at the user's pointing-out (~15:15 EET); the other R77 items stay queued.
- **(1) Extension quality-pick fails silently (`app/extension/video-detect.js`, `app/extension/content.js`, `app/engine/src/video.rs`):** proven in code: `buildResolvedVariant` sent `headers` as a raw JSON map; the engine's `VideoVariantInput.headers` is `Option<String>` (a JSON-encoded string, parsed by `headers_from_json`) — the host refused the whole `video_found` message with "invalid type: map, expected a string" (the exact lastHandoff error), so no download row was ever created. Fix: `JSON.stringify` the headers map; the misleading `headers is a JSON map` comment in `video.rs` clarified. A failed pick now also re-renders the error in the dropdown (the 2.6 s toast alone was invisible in the reproduction). Tests: extension 2 new (headers-is-string; failed pick re-opens the dropdown with the error) + engine test deserializing a resolved variant with string headers (map form asserted to fail).
- **(2) Tom & Jerry "needs sign-in" even with cookies (`app/engine/src/ytdlp.rs`, `app/extension/content.js`, `app/host/src/main.rs`):** proven in code (mechanism): the resolve ran with_cookies=true (stored grant exists) and still failed login_required, while Rick Astley resolves fine with the same cookies — not a cookie problem. Fixes: new `YtDlpErrorKind::BotBlocked` ("bot_blocked") for the explicit bot-check needles ("confirm you're not a bot", "not a bot") with the user message "YouTube is blocking this video on this connection…"; "login_required"/"playability status" stay LoginRequired. Extension: a needs_login failure that already used cookies renders "You're already signed in, but this video is still blocked on this connection." with NO "Try again", and `retryResolve` re-renders instead of repeating the identical request (the R79 "never repeats" claim was false for the cookied case). A cookieless bot-block still gets the one consent retry. Tests: engine map_error cases updated, host test (bot_blocked → needs_login false), extension R79 test updated to the new terminal message + no-Try-again assertion.
- **(3) TikTok feed page (`app/extension/video-detect.js`, `app/extension/content.js`):** proven in code: the resolve ran for `location.href` (the /foryou feed URL). Fix: `videoPageUrlFromElement` walks up from the playing video element to the nearest ancestor anchor pointing at a real video page (TikTok /@user/video/<id>, YouTube watch/shorts/embed); the resolve (page_url, cache key, consent host, cookie fetch, re-resolve source_url) all use that URL, falling back to the page URL. Tests: 2 new in resolve.test.js + a content test that a feed video resolves its own page.
- **(4) Add URL window not on top after clipboard copy (`app/src-tauri/src/lib.rs`, `app/src/lib/api.ts`, `app/src/App.tsx`):** proven in code: the clipboard offer handler opened the Add URL dialog directly inside the main window, bypassing the pending-capture queue (whose standalone window pops in front); with the main window hidden to tray the user saw nothing. Fix: new Tauri command `enqueue_clipboard_capture` (registered) + api wrapper — the offer is enqueued as a pending capture (`source = "browser"`), the 1 s poll picks it up, and the standalone window opens in front; on enqueue failure it falls back to the in-app dialog. Accepted loss: no "already in your list" warning in the standalone dialog (the row carries no such flag); the engine still flags duplicates at submit.
- **(5) Double title (ex-R77 item a) (`app/src/components/AddDownloadDialog.tsx`):** proven (again): the standalone window's own TitleBar AND the dialog's inner `<h2>` both show the filename. Fix: the dialog's inner header (logo + title + X + divider) is hidden in `standalone` mode; `aria-labelledby` dropped there too. Tests: 2 new in add_download_dialog.test.tsx.
- **(6) Transparent app icon (`app/src-tauri/icons/logo.png`, `app/src-tauri/icons/`, `app/extension/icons/`, `app/public/icon.png`):** user-approved ("تمام نفذها"). The icon source `logo.png` is now the transparent enlarged logo (1024x1024 RGBA, same quills + arrow artwork); the old boxed source is kept as `logo-boxed-2026-11-17.png`. `scripts/generate-icons.py` regenerated `icon.png` + the extension icons (16/32/48/128, matching `manifest.json`; the popup header uses `icons/icon32.png`, so it picks the new art up with no code change); `npx @tauri-apps/cli@2 icon` regenerated the Tauri set (32x32, 128x128, 128x128@2x, icon.ico — 6 images, all 32bpp). Verified by pixel: 32x32 RGBA, corners alpha 0, artwork bbox 97% x 100% (was ~55%); visually checked on dark (#0B0F24) and light backgrounds. The tray needs no separate asset (`default_window_icon()`). `app/public/icon.png` (TitleBar + AddDownloadDialog header, byte-identical to the old boxed logo) replaced with the same transparent PNG; `npm run build` passes. Artwork itself unchanged.
- **Local verification:** engine suite **527/527, 0 failed** (full `cargo test --release`, exit 0, ~16.5 min; 526 after Round 80 + 1 new; `/tmp/fetchline-test-*` cleaned before/after); extension **115/115** (111 + 4 new); host **6/6** (4 unit + 2 spawn); UI Vitest **30/30** (28 + 2 new); `npm run build` passes; `cargo fmt --check` clean on engine, host, src-tauri.
- **VPS step (left for the parent):** build (host + app — the Tauri crate cannot compile here, so this is the first real compile of the new `enqueue_clipboard_capture` command + the App.tsx change) → retest checklist: (1) extension: pick a quality on Rick Astley — a download row MUST be created (the Round 81 reproduction failure); pick with the host down / a bad row must show the error in the dropdown; (2) Tom & Jerry: the dropdown must say YouTube is blocking it (or the signed-in-but-blocked message), NOT "needs sign-in", and Try again must not repeat the identical failure — also read the host.log stderr tail next to the login_required line to confirm which needle matched; (3) TikTok feed page: the extension must resolve the playing video (qualities load), not "not a supported video page"; (4) copy a download link: the standalone Add URL window must pop IN FRONT over the browser (main window hidden to tray); (5) capture Add URL window: the file name appears ONCE; (6) regression: Round 80 items (TikTok 403 fallback, Instagram, Facebook feed error, ffmpeg DLLs in target\release, bad-URL real error, YouTube extension + manual 230 MB Rick Astley). (7) transparent icon: the built exe/installer + taskbar + tray show the new transparent icon (no dark box, artwork fills the canvas), and the extension toolbar icon + popup header show it too. Round 77 (auto-pick category, remember-path revert, stale popup) stays queued.
- **Round 82 — fix round for the Round 80+81 VPS retest failures (local only, NOT pushed; on top of head 462fad77):** (1) Quality-pick: host now accepts `VideoVariantInput.headers` as a JSON string OR a raw map (`de_string_or_map`, video.rs) — proven by live host-binary reproduction that the R81 fix is correct and the VPS failure was the stale extension (old payload → byte-identical "invalid type: map" error; current payload → enqueued). Regression tests in extension_tests.rs with the exact payloads. (2) TikTok feed: `videoPageUrlFromElement` (video-detect.js) gains a second pass — a video-page anchor among an ancestor's descendants is accepted only when it is the SOLE such anchor in that subtree (sibling overlay links now found; a neighbour slide's link never picked). (3) Tom & Jerry: new `YtDlpErrorKind::VideoUnavailable` ("video_unavailable"), checked in map_error BEFORE the login needles; extension terminal message, no Try again, no cookie retry. The MUyWv16cGys stderr was verified live with yt-dlp 2026.10.26 on this VM. (4) Manual YouTube download: the Round 80 probe-403 → yt-dlp fallback is now TikTok-only (`is_tiktok_page`, manager.rs) — a YouTube probe 403 surfaces the real probe error with no row, as in Round 79. Engine suite 530/530 (527 in R81 + 3 new: 2 extension_tests headers-tolerance + 1 video_tests YouTube-403; exit 0, 28 test-result lines, 0 failures; the first full run caught 2 lib failures — the R81 video.rs test asserting the map form fails, now updated to the new tolerant behaviour, and the ytdlp needle missing "is", now "video is unavailable" OR "video unavailable" — both fixed the same session, full suite re-run green);  extension 117/117 (115 + 2 new: TikTok subtree search, video_unavailable content test); host 6/6; UI Vitest 30/30; `npm run build` passes; `cargo fmt` clean on engine/host/src-tauri. NOT verified locally: the Tauri app crate cannot compile here (no gdk-3.0); no live TikTok/YouTube resolve attempted here (the Tom & Jerry stderr was verified with a standalone yt-dlp on this VM, not through the app).
- **Round 82 — VPS RETEST CHECKLIST (must-do before any item counts as verified):** (0) After `git pull` to the new head, RELOAD the Chrome extension EXPLICITLY (chrome://extensions → Reload on extension ID mioamdgogndabpcjbdekggcnlkkfpmbj) BEFORE testing items (1)-(2), and verify the loaded extension matches the new code — the Round 80+81 retest never reloaded it, which is why every extension fix failed while every app fix passed. (0b) Build WITH bundle this time (`npx @tauri-apps/cli@2 build`, not --no-bundle) so the installer DLL check can be verified. (1) Quality-pick: Rick Astley → pick 360p in the extension dropdown → a download row MUST appear (no "invalid type: map" toast). (2) TikTok feed: open ANY public TikTok video's feed page, pause the playing video, click its Download button → it must resolve THAT video's own page (/@user/video/<id>), NOT the /foryou feed URL. (3) Tom & Jerry (MUyWv16cGys): the error must now say the video itself is unavailable (host.log kind=video_unavailable), NOT "needs you to be signed in", with no Try again loop. (4) Manual YouTube Add URL: Rick Astley 2160p must download the STREAM (230 MB) via the segmented path; if the probe 403s again, capture the probe status + yt-dlp stderr from the logs before retrying. (5) TikTok 403-fallback retest: ANY public TikTok video — a TikTok video whose probe 403s must still download via the yt-dlp fallback (the Round 80 fix must keep working after the Round 82 scoping). (6) Instagram reel retest: ANY public Instagram reel (no invented IDs — the last retest used a made-up reel ID and proved nothing). (7) Installer DLL check: all 7 ffmpeg DLLs (avcodec-62, avdevice-62, avfilter-11, avformat-62, avutil-60, swresample-6, swscale-9) present in the installer, not just in target\release. (8) YouTube regression: extension qualities 360p–2160p + manual Add URL download, as in Round 79.

- **Round 83 — fix round for the Round 82 VPS retest failures (TikTok feed + TikTok download exit 1) + grabber rename/delete + boxed logo + right-click paste (local only, NOT pushed; on top of head c7804e8f):** (1) TikTok feed: `videoPageUrlFromElement` (video-detect.js) counts DISTINCT video ids (the R82 second pass counted every anchor, even several to the same video → null) and extracts the id from the xgwrapper id / data-vid / data-aweme-id / video-src query, validated as a TikTok snowflake, with the author handle from data-e2e="video-author-uniqueid" or a profile anchor; builds `@<handle>/video/<id>` or `@/video/<id>`; the resolve uses the visible playing video (centre of the viewport, not paused), never a neighbour's; a feed page with no derivable id shows "Open the video's own page (click the video), then press Download." and never sends /foryou to the resolver. The `@/video/<id>` form was verified with standalone yt-dlp 2026.10.26 on this VM (formats listed); the Rust URL check needed no change. Tests from the reconstructed VPS dump fixture (the real files are only on the VPS). (2) TikTok download: the direct yt-dlp runner now keeps a 40-line ring buffer of BOTH streams, parses progress from stdout (where yt-dlp actually prints it), and on failure logs the redacted argv + exit status + yt-dlp version + format_id + page URL + tail to startup.log and puts the mapped stderr reason in the error message (the R82 "yt-dlp exited with exit code: 1" with no reason anywhere cannot recur); safe fixes alongside — format selector fallback `<id>/b[ext=mp4]/b`, `-P <dir> -o <name with % as %%>`, and the capture's cookies + the format's User-Agent/Referer now reach the direct call (the Add URL path stays cookieless — its resolve never had cookies, Jumani's "resolve passes cookies but download doesn't" was wrong for that path). Two fake-yt-dlp end-to-end tests (video_tests.rs) cover the failure surfacing + cookie forwarding. The final root-cause choice awaits the retest's stderr tail. (3) YouTube bot-block: accepted as datacenter-IP behaviour, no workarounds; both the mapped detail and the user message now carry "This is common on cloud servers; it normally works on a home connection."; the player-client check found no forcing in production code. (4) Grabber rename/delete: store + manager + two Tauri commands + api wrappers + sidebar inline rename (Pencil) and confirm delete (Trash2, running projects refused, downloads kept untagged); 4 new grabber tests. (5) Boxed logo: audit found every live usage transparent (verified by pixel); the dead `app/src/assets/logo.jpg` was deleted; `app/src-tauri/icons/logo.jpg` kept as the documented fallback + the single 2048 px original. (6) Right-click paste: the document-level contextmenu suppression now skips inputs/textareas/contenteditables, so WebView2's native menu offers Paste in the Add URL field. Engine suite 540/540, 0 failures, exit 0 (28 test-result lines, log ~/workspace/r83-engine-final.log); extension 127/127; host 6/6; UI Vitest 30/30; `npm run build` passes; `cargo fmt` clean on engine/host/src-tauri. NOT verified locally: the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile of the new commands + UI changes.
- **Round 83 — VPS RETEST CHECKLIST (must-do before any item counts as verified; fits one ~60 min RDP session):** (0) After `git pull` to the new head, RELOAD the Chrome extension EXPLICITLY (chrome://extensions → Reload on extension ID mioamdgogndabpcjbdekggcnlkkfpmbj) BEFORE testing items (1)–(2), and verify the loaded extension matches the new code — the Round 80+81 retest never reloaded it, which is why every extension fix failed while every app fix passed. (0b) Build WITH bundle (`npx @tauri-apps/cli@2 build`, not --no-bundle). (1) TikTok feed — open ANY public TikTok video's feed page, click its Download button → it must resolve THAT video's own page (`/@user/video/<id>`), NOT the /foryou feed URL; verify against the REAL fixture files on the VPS (`C:\fetchline-r55\app\extension\tests\fixtures\` — do NOT overwrite or delete them; the retest task should commit them or leave them in place). Also check a feed page where no id can be derived shows "Open the video's own page (click the video), then press Download." (2) TikTok download — a TikTok video MUST download; then force a failure (e.g. a format that no longer exists) and check the dialog + startup.log show the REAL stderr reason, not just the exit code — this is the capture that decides the final Q2 fix. (3) Quality-pick 360p + manual YouTube 2160p download — DO NOT close the Add URL dialog with X before the row appears (the R82 retest closed it early, so both were inconclusive). (4) Grabber rename + delete work (rename inline in the sidebar, delete asks first, a running project refuses delete, its downloads stay in the list). (5) The logo at the top of the main window has no dark box — also check `dist/icon.png` corners on the VPS build if the box is still seen (the audit found every live usage transparent in the repo; a stale `dist/icon.png` is the prime suspect if it persists). (6) Right-click Paste works in the Add URL field. (7) YouTube bot-block message now carries the cloud-server hint ("This is common on cloud servers; it normally works on a home connection."). (8) YouTube regression: extension qualities 360p–2160p + manual Add URL download, as in Round 79.
- **Round 84 — fix round for the Round 83 VPS retest failures (TikTok pre-check 403 + boxed logo + Add URL window / Download-complete Close) + reference check against Ezoo + Mister (local only, NOT pushed; on top of head 2126369e):** (1a) Pre-check skip: the resolved size rides extension `buildResolvedVariant.filesize` → `VideoVariantInput.filesize` → `ResolvedLink.filesize` → new `pending_links.filesize` column (migration + schema.sql) → `PendingLinkRow.filesize` → dialog `initialFileSize`; the dialog never probes a resolved capture and renders the pre-resolved size. (1b) Headers/cookies to the worker: verified R78 already forwarded them to segment requests — the probe was the only bare request; new `add_resolved_stream_download` creates the download probe-free (synthetic ProbeInfo, size = filesize, accept_ranges false → one open-ended segment), sharing the tail with the probed add via `create_download_from_probe`; `tiktok.com` hosts capped at 1 segment (v16-webapp-prime.us.tiktok.com was the R83 host). (1c) `confirm_pending_link` never marks a row rejected on an add failure (stays pending, retryable; policy rejections unchanged); the resolved marker is a non-empty `format_id`; the 401/403 fallback is the existing run-level `reresolve_once`. Reference check: Ezoo (headers on probe + download) + Mister (browser-like probe, "size unknown" tolerated, 401/403 → NeedsLogin state) confirm the approach; Ezoo's `relay_refused` browser-relay ladder and Mister's `--extractor-args "generic:impersonate"` lead are RECORDED in bugs.md, NOT implemented. (2) Logo: the baked-tile hypothesis is disproven by pixel (icon.png is byte-identical to the transparent cut; the opaque region is a diagonal quills shape); the box was TitleBar's `rounded-[5px]` + purple glow — both removed, frameless now; preview ~/workspace/titlebar-logo-preview.html. (3a) Standalone Add URL window: `closeWindow()` falls back to `destroy()` on a rejected `close()`. (3b) Download-complete Close: already the same `close()` as the X and `allow-close` is present — the missing-permission hypothesis is wrong; `closeDialog()` now falls back to `destroy()`. Engine suite 543/543, 0 failures, exit 0 (log ~/workspace/r84-engine.log); extension 127/127; host 6/6; UI Vitest 32/32; `npm run build` passes; `cargo fmt` clean on engine/host/src-tauri. NOT verified locally: the Tauri app crate cannot compile here (no gdk-3.0) — the VPS build is the first real compile.
- **Round 84 — VPS RETEST CHECKLIST (must-do before any item counts as verified; fits one ~60 min RDP session):** (0) After `git pull` to the new head, RELOAD the Chrome extension EXPLICITLY (chrome://extensions → Reload on extension ID mioamdgogndabpcjbdekggcnlkkfpmbj) BEFORE testing items (1)–(2), and verify the loaded extension matches the new code — the Round 80+81 retest never reloaded it, which is why every extension fix failed while every app fix passed. (0b) Build WITH bundle (`npx @tauri-apps/cli@2 build`, not --no-bundle). (1) TikTok extension quality-pick MUST create a download row and download — the R83 blocker. The dialog must show the pre-resolved size (no probe error) and Start Download must work. (2) Manual Add URL of the TikTok page URL (https://www.tiktok.com/@chisbetyu81/video/7223205454032178475 — the R82 exit-1 path): if it fails, the dialog + startup.log must show the REAL stderr reason via the R83 ring buffer. (3) Retry after a probe failure is NOT locked out — the "already decided (rejected)" message must not recur (try a failing link twice: the second attempt must reach the add path with the real error, not "already decided"). (4) Logo — no dark box in the main-window title bar (take an enlarged crop). (5) Grabber rename/delete (R83, unreached). (6) Right-click Paste in Add URL (R83, unreached). (7) YouTube bot-block hint (R83, unreached; Looney Tunes v=Fg_KxhW8-PY). (8) Manual YouTube 2160p + quality-pick redo WITHOUT closing the Add URL dialog (R83, unreached). (9) Add URL window closes on its own after Start Download. (10) Download-complete dialog Close button dismisses it. ALSO: report plainly whether the app window restores from the taskbar — the R83 retest could not restore it in the last 13 min; do not assume it is a bug or not a bug. NOTE: the real fixture files live only on the VPS (`C:\\fetchline-r55\\app\\extension\\tests\\fixtures\\tiktok-foryou-dump.json`, 5,109 B, restored over the reconstructed 2,604 B one) — do NOT overwrite them; Round 84 did not touch the fixture, so no discrepancy to record.
