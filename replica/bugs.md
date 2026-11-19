# Bugs: Fetchline (IDM rebuild) — replica-test pass

Date: 2026-11-14. Build: slices 1 + 2 (S01–S05). Env: this Linux VM,
local mock HTTP server, `cargo test` in `app/engine`.
All four S2 bugs below were reproduced by a failing test first, then
fixed, and the tests now pass (51 integration + 4 unit, 3 consecutive
full runs, 0 failures). No S1 was found. S3/S4 are listed, not fixed,
per the test brief.

Update (2026-11-14): the S3/S4 bugs from this pass (BUG-005/006/007)
and from round 2 (BUG-008–011) were all fixed in later bug-fix
passes — see their Status lines. No open bugs remain in this file.

---

### BUG-001: Saving "No schedule" in the scheduler always fails

- Severity: S2
- Flow / case: F04 / F04-S6
- Screen: S05 (SchedulerDialog)
- Build: 2026-11-14

Steps
1. Open Queues & scheduler for any queue.
2. Select "No schedule — start manually".
3. Click "Save schedule".

Expected: the schedule is cleared and "Schedule saved." is shown.
Actual: the save fails with "Periodic synchronisation is not built yet —
use one-time scheduling for now", even though nothing periodic was asked
for. Once a schedule is set, it can never be removed from the UI.
Evidence: cargo test `update_queue_accepts_none_and_clears_schedule`
(failed before the fix with exactly that error).
Suspected cause: `Store::update_queue` rejected every `schedule_type`
other than `"one_time"`, but the UI's `QueueScheduleType` includes
`"none"` and `api.updateQueue` passes it straight through. The schema
only allows `'one_time'`/`'periodic'`, so there was no stored
representation for "no schedule".
Status: FIXED. The engine now accepts `"none"` and stores it as
`'one_time'` with NULL start/stop times (the scheduler already skips
queues with no start time), and `SchedulerDialog` reads a stored
`'one_time'` queue with no start time back as "No schedule". A fresh
queue (which the engine creates as unscheduled) now also shows "No
schedule" instead of demanding a start time. Files:
`app/engine/src/store.rs` (`update_queue`),
`app/src/components/SchedulerDialog.tsx` (form fill).

---

### BUG-002: Adding the same link twice gives both downloads the same file

- Severity: S2
- Flow / case: F02 / F02-N6
- Screen: S01/S02
- Build: 2026-11-14

Steps
1. Add a link (do not start it).
2. Add the same link again (or a different link with the same filename)
   into the same folder.

Expected: the second download is saved as `file (2).bin`.
Actual: both rows point at the same `save_path`. Starting both races
on one `.part` file and the second completion overwrites the first —
one file on disk, two rows claiming it.
Evidence: cargo test `duplicate_add_gets_different_save_path` (failed
before the fix: both paths identical).
Suspected cause: `unique_path` only checked the disk. A download that
has not started yet has no file on disk, so the disk check passed for
both adds.
Status: FIXED. `add_download` now also checks the database: a path
already reserved by a live (not deleted) download counts as taken, so
the second add is suffixed. Files:
`app/engine/src/manager.rs` (`unique_path`, `add_download`),
`app/engine/src/store.rs` (`is_save_path_used`).

---

### BUG-003: A finished download in a queue arms the completion action after downloading nothing

- Severity: S2
- Flow / case: F04 / F04-N1, F04-N2
- Screen: S05
- Build: 2026-11-14

Steps
1. Add a link to a queue, then start it directly from the main list
   (not via the queue) so it finishes while still filed in the queue.
2. Give the queue a completion action (e.g. shutdown or run program).
3. Start the queue.

Expected: the finished file leaves the queue and nothing else happens —
the queue downloaded nothing.
Actual: the queue "drains" at once and registers the completion action
with no file: a `run_program` action would execute immediately, and a
shutdown action would show its countdown for a queue that downloaded
nothing. The finished file also stayed in the queue forever, since only
the runner unqueues files and it never picked it up. Dragging a finished
row onto a queue in the UI produced the same state (the drop called
`add_to_queue`, which accepted finished downloads).
Evidence: cargo test
`queue_with_only_completed_files_registers_no_action_and_drops_them`
(failed before the fix: a shutdown action was pending) and
`add_to_queue_rejects_completed_download` (failed before the fix:
the add succeeded).
Suspected cause: `run_queue` only looked for queued/paused/downloading/
error files, so a finished file was invisible to it yet still counted
as a clean drain; `add_to_queue` never checked the download's status.
Status: FIXED, both ends. `add_to_queue` now refuses finished downloads
with "This download is already finished, so it cannot be added to a
queue". `run_queue` drops finished files from the queue when it meets
them and only registers a completion action when at least one file
actually completed in that run. Files:
`app/engine/src/store.rs` (`add_to_queue`),
`app/engine/src/manager.rs` (`run_queue`).

---

### BUG-004: Deleting a category fails with a raw database error after its downloads were Removed

- Severity: S2
- Flow / case: F10 / F10-N4
- Screen: S04
- Build: 2026-11-14

Steps
1. Create a custom category and add a download to it.
2. Remove the download with the main list's Remove button (this sets
   status `deleted`, it does not erase the row).
3. Delete the category.

Expected: the category is deleted — the engine's own check only counts
live downloads, and there are none left.
Actual: the delete fails with `Database error: FOREIGN KEY constraint
failed`. The user cannot delete the category at all from the UI: the
removed downloads are invisible, so there is nothing to move away.
Evidence: cargo test `delete_category_allowed_when_downloads_deleted`
(failed before the fix with the foreign-key error).
Suspected cause: `downloads.category_id` is `ON DELETE RESTRICT`, and
the foreign key does not care about the download's status. The
engine's pre-check excluded deleted rows, but the actual DELETE still
hit the constraint.
Status: FIXED. `delete_category` now hard-deletes the soft-deleted
download rows in that category first (their segments cascade), then
deletes the category. File: `app/engine/src/store.rs`
(`delete_category`).

---

## S3 — broken with a workaround, or visibly wrong (listed, not fixed)

### BUG-005: The engine stores a malformed schedule start time without complaint

- Severity: S3
- Flow / case: F04 / F04-S9
- Screen: S05

Steps: save a queue with `start_at = "tomorrow at noon"` (via the engine
API — the desktop UI always sends valid ISO or null).
Expected: the save is rejected as an invalid time.
Actual: it is stored, and the scheduler tick then silently skips the
queue forever — no error, no run.
Evidence: cargo test
`update_queue_malformed_start_time_is_stored_but_never_runs` (passed:
it documents the current behaviour).
Workaround: the UI never sends malformed times, so a desktop user
cannot hit this. Fix for later: validate ISO-8601 in `update_queue`.
Status: FIXED (2026-11-14, bug-fix pass). `Store::update_queue`
now validates both times with the existing `parse_iso_to_unix`
(UTC ISO-8601, fractional seconds accepted) and rejects a malformed
start or stop time with a clear error — nothing is stored. The
parser also rejects impossible dates (30 February, 31 April,
29 February in a non-leap year), which previously rolled silently
into the next month. Legacy rows written before the fix cannot be
created any more; if one is found, `scheduler_tick_at` reports it
instead of silently skipping it (see below) — the tick still runs
every other queue first, then returns an error naming the bad
queues ("Some queues have an invalid schedule and were skipped:
… Re-save their schedules."). Re-saving such a queue from the
dialog validates and clears the bad times. Regression tests:
`update_queue_rejects_malformed_start_time`,
`update_queue_rejects_malformed_stop_time`,
`scheduler_tick_reports_legacy_malformed_schedule_but_runs_valid_queues`
(DOC test converted), plus the timeutil unit test
`rejects_impossible_dates`. Verified: full suite 109 integration +
13 unit, 0 failed, 2 consecutive runs. Files:
`app/engine/src/store.rs` (`update_queue`, `clean_time`),
`app/engine/src/timeutil.rs` (days-in-month check),
`app/engine/src/manager.rs` (`scheduler_tick_at`).

### BUG-006: The engine stores a stop time before the start time without complaint

- Severity: S3
- Flow / case: F04 / F04-S10
- Screen: S05

Steps: save a queue with start in 1 hour and stop in 1 minute (via the
engine API).
Expected: the save is rejected, or the window is treated as invalid.
Actual: it is stored and the queue silently never runs (the tick sees
the window as already passed).
Evidence: cargo test `update_queue_stop_before_start_never_runs`
(passes: it documents the current behaviour).
Workaround: the Scheduler dialog blocks this ("The stop time must be
after the start time"), so a desktop user cannot hit it. Fix for later:
validate stop > start in `update_queue`.
Status: FIXED (2026-11-14, bug-fix pass). `Store::update_queue`
now rejects a stop time that is not after the start time with
exactly the UI's message — "The stop time must be after the start
time." — so engine and UI agree; a stop equal to the start and a
stop with no start are rejected too, and nothing is stored in any
of these cases. Legacy inverted rows are reported by
`scheduler_tick_at` the same way as malformed ones (BUG-005).
Regression tests: `update_queue_rejects_stop_before_start`,
`update_queue_rejects_stop_equal_to_start_and_stop_without_start`
(DOC test converted). Verified: full suite 109 integration + 13
unit, 0 failed, 2 consecutive runs. File:
`app/engine/src/store.rs` (`update_queue`).

## S4 — cosmetic

### BUG-007: The sidebar delete-category button is mouse-only

- Severity: S4
- Flow / case: F10 / F10-M1
- Screen: S04 (`CategorySidebar`)

Steps: tab through the sidebar with the keyboard only.
Expected: the delete button for a custom category can be reached and
activated from the keyboard.
Actual: the button is `hidden` until its row is hovered
(`group-hover:block`), so it is never focusable by keyboard. A
keyboard-only user cannot delete a custom category.
Evidence: code review of `app/src/components/CategorySidebar.tsx`
(`hidden … group-hover:block`, no `focus-within` variant). Not
reproducible in an automated test here (no browser automation on
this VM).
Workaround: use the mouse. Fix for later: show the button on
`group-focus-within` as well as hover.
Status: FIXED (2026-11-14, bug-fix pass). Both hover-only buttons
in the sidebar — delete-category (`:111`) and the queue schedule
button (`:157`, same pattern, noted in round 2) — no longer use
`hidden … group-hover:block`. They now use `opacity-0` with
`group-hover:opacity-100 group-focus-within:opacity-100
focus-visible:opacity-100`: `opacity-0` keeps the button in the
keyboard tab order (unlike `display:none`), it appears when its
row is hovered, when anything in the row has focus, and when the
button itself has keyboard focus. Both buttons already carry
`aria-label`s. Verified without a desktop binary by (a) a source
check — no `group-hover:block` / `hidden` remains on either
button in `CategorySidebar.tsx`, and (b) the built CSS contains
the emitted rules
`.group:focus-within .group-focus-within\:opacity-100{opacity:1}`
and `.focus-visible\:opacity-100:focus-visible{opacity:1}`
(`npm run build` passes). A real keyboard walk-through still
needs the desktop binary. File:
`app/src/components/CategorySidebar.tsx`.

---

## Known limitations, not bugs

These come from `replica/build-log.md` and were verified, not re-reported
as bugs:

- Speed graph missing in the progress dialog (numbers shown instead).
- No native folder-picker button in S02 (plain text field).
- Periodic synchronisation deferred to L (disabled in the UI, rejected
  by the engine).
- No Windows `.exe`; the Tauri shell does not compile on this Linux VM
  (missing `gdk-3.0`) — the 16 queue/category commands are uncompiled,
  drag-and-drop and the countdown dialog are UI-only until the first
  Windows build.
- Pending completion actions live only in engine memory: closing the app
  before answering the countdown loses the action.
- Shutdown / hibernate run real OS commands and have never executed on
  Windows here.

## To check (not reproduced, needs the desktop / Windows)

- F02-T1: filenames over ~200 chars may hit Windows path limits at file
  creation (the engine does not truncate).
- F03-T1: resume after killing the app mid-download (same code path as
  pause/resume, but never exercised across a process restart).
- F10-E3: a category folder that does not exist yet (the engine creates
  it on download via `create_dir_all`, so this is expected to work).
- The 8 manual UI cases in `test-plan.md` (dialog validation, Escape /
  backdrop, pause buttons, drag-and-drop, scheduler validation,
  countdown dialog, scheduler tabs) — all blocked on the desktop binary.

---

## Round 2 — Settings (S09), proxy, speed limiter (S12) — 2026-11-14

Build: slices 1–4 (S01–S05, S07, S09, S10, S12). Engine: 98
integration + 12 unit tests, 0 failed (2 consecutive full runs).
No S1 and no new S2 was found in this round. BUG-008–011 below were
fixed in a later bug-fix pass (2026-11-14) — see their Status lines;
the suite is now 106 integration + 12 unit, 0 failed.

### BUG-005 / BUG-006 / BUG-007 — re-checked in round 2, still open then (superseded: all three FIXED in the bug-fix pass — see their Status lines above)

- BUG-005 (malformed schedule start time) and BUG-006 (stop before
  start): the two DOC tests
  (`update_queue_malformed_start_time_is_stored_but_never_runs`,
  `update_queue_stop_before_start_never_runs`) still pass, so both
  behaviours are unchanged by milestone 4.
- BUG-007 (sidebar delete-category button mouse-only): still present —
  `app/src/components/CategorySidebar.tsx:111` is still `hidden …
  group-hover:block` with no `focus-within` variant. The queue Start
  button at `:157` has the same hover-only pattern.

Update (2026-11-14, bug-fix pass): all three are now FIXED — see
their Status lines above. The DOC tests were converted into
regression tests (`update_queue_rejects_malformed_start_time`,
`update_queue_rejects_stop_before_start`, and the tick test for
legacy rows); the sidebar buttons are keyboard-reachable. No open
bugs remain in bugs.md.

## S3 — broken with a workaround, or visibly wrong (listed, not fixed)

### BUG-008: An absurd speed limit crashes the settings save (debug) / wraps silently (release)

- Severity: S3
- Flow / case: S12 / S12-E1
- Screen: S09 Connection tab (`SettingsDialog`, speed limit field)
- Build: 2026-11-14

Steps
1. Save a speed limit of `i64::MAX` KB/s (via the engine API
   `set_speed_limit`; the desktop number field has no maximum).
2. The engine applies the settings.

Expected: the value is rejected as out of range, or clamped to a
sane maximum.
Actual: in a debug build the save panics — `attempt to multiply
with overflow` at `app/engine/src/manager.rs:189`
(`(k.max(0) as u64) * 1024`). In a release build the multiply wraps
instead of panicking, so some huge values silently become a small
cap (e.g. 2^54 + 16 KB/s wraps to a ~16 KB/s cap) with no error.
Evidence: a temporary probe test calling
`mgr.set_speed_limit(Some(i64::MAX))` panicked exactly as above
(test removed after reproducing; the suite has no panicking test).
A sane huge value (10_000_000 KB/s) works fine and is covered by
`speed_limit_rejects_negative_and_accepts_huge_sane_value`.
Workaround: save a normal speed limit; the stored row is only a
number, so a sane save fixes it. Fix for later: validate an upper
bound in `Store::update_settings` (or saturate in `apply_settings`).
Status: FIXED (2026-11-14, bug-fix pass). Rejected, not clamped:
`Store::update_settings` now rejects any cap above
`MAX_SPEED_LIMIT_KBPS = 10_000_000` KB/s with "The speed limit is too
large — the maximum is 10,000,000 KB/s", so no value can overflow the
bytes-per-second conversion; `apply_settings` also uses
`saturating_mul` as defence in depth, and `SettingsDialog` enforces
the same bound (validation + `max={10000000}`). The 10,000,000 KB/s
sane-huge value still works. Regression test:
`speed_limit_absurd_value_is_rejected` (was failing with the debug
panic before the fix). Verified: full suite 106 integration + 12
unit, 0 failed, 2 consecutive runs. Files:
`app/engine/src/store.rs` (`MAX_SPEED_LIMIT_KBPS`, `update_settings`),
`app/engine/src/manager.rs` (`apply_settings`),
`app/src/components/SettingsDialog.tsx` (validation, input max).

### BUG-009: The proxy Test button reports "OK" for a 407 (wrong credentials)

- Severity: S3
- Flow / case: S09 Proxy tab / S09-P7
- Screen: S09 (`SettingsDialog`, Test the proxy)
- Build: 2026-11-14

Steps
1. Configure an HTTP proxy that demands Basic auth, with the wrong
   password.
2. Click Test (any URL through that proxy).

Expected: the test reports failure — the proxy refused the
credentials.
Actual: `test_proxy` returns the proxy's status as a success value,
so the dialog shows "OK — the proxy answered with status 407."
The number 407 is visible, but the verdict says OK.
Evidence: cargo test
`test_proxy_returns_407_status_for_wrong_credentials` (passes: it
documents the behaviour — `test_proxy` returns `Ok(407)` and the
proxy records 0 hits). Code review of `SettingsDialog.tsx`
`runTest`: any returned status is rendered as "OK — …".
Workaround: read the status number (407 = bad credentials). Fix
for later: `test_proxy` should error on 4xx/5xx, or the UI should
label non-2xx as a failure.
Status: FIXED (2026-11-14, bug-fix pass). `test_proxy` now returns
an error on a 407 — "Proxy authentication failed (407) — check the
proxy username and password" — and on any other non-2xx status, so
the dialog's existing "Failed — …" path shows it as a failure. The
DOC test was converted into the regression test
`test_proxy_wrong_credentials_is_an_error`. Verified: full suite 106
integration + 12 unit, 0 failed, 2 consecutive runs. File:
`app/engine/src/manager.rs` (`test_proxy`).

### BUG-010: The proxy Test button reports success without ever contacting the proxy

- Severity: S3
- Flow / case: S09 Proxy tab / S09-P6
- Screen: S09 (`SettingsDialog`, Test the proxy)
- Build: 2026-11-14

Steps
1. Configure a proxy with "Use the proxy for: HTTPS only".
2. Click Test with the default test URL `http://example.com/`
   (or any http:// URL).

Expected: the test fails, or warns that the proxy does not cover
http:// URLs — the proxy was never used.
Actual: the request goes direct (the scope says https only), returns
200, and the dialog shows "OK — the proxy answered with status 200"
even though the proxy saw 0 requests.
Evidence: cargo test
`test_proxy_https_only_scope_never_contacts_proxy_for_http_url`
(passes: it documents the behaviour — status 200, proxy hits 0).
Workaround: test with an https:// URL, which does go through the
proxy. Fix for later: `test_proxy` should fail when the configured
scope does not cover the test URL, or the UI should warn.
Status: FIXED (2026-11-14, bug-fix pass). `test_proxy` now checks
before sending: a disabled proxy, a non-http(s) test URL, an FTP
proxy, and a proxy whose `use_for` scope does not cover the test
URL's scheme are all errors with a clear message (e.g. "This proxy
is only used for https, so it cannot be tested with an http:// URL
— use a https test URL instead"), so a test can no longer report
success for a proxy it never used. The DOC test was converted into
`test_proxy_https_only_scope_rejects_http_test_url`, plus
`test_proxy_http_scope_uses_proxy_for_http_url` (positive case) and
`test_proxy_ftp_type_is_rejected`. Verified: full suite 106
integration + 12 unit, 0 failed, 2 consecutive runs. File:
`app/engine/src/manager.rs` (`test_proxy`).

## S4 — cosmetic

### BUG-011: Two settings fields are stored but do nothing

- Severity: S4
- Flow / case: S09 / —
- Screen: S09 (no tab edits them)
- Build: 2026-11-14

Steps: save settings, then look for any effect of
`file_types_auto` or `excluded_sites`.
Expected: every stored setting is either editable in the UI or used
by the engine.
Actual: both fields are persisted by `Store::get/update_settings`
and exist in the `Settings` type the UI receives, but nothing in
the engine or the UI ever reads them — the File Types tab edits
category extension lists instead, and there is no site-exclusion
check anywhere in the download path.
Evidence: code search — the only references are the struct fields,
the settings-table reads/writes in `store.rs`, and the type
definition in `src/lib/types.ts`.
Workaround: none needed — they are inert. Fix for later: remove
them from the schema/type, or wire them (auto file-type list /
site exclusion) as a real feature.
Status: FIXED (2026-11-14, bug-fix pass) — wired in, not deleted.
Semantics (recorded in the build log): `file_types_auto` is the
allow-list for auto-assign — empty means every extension is
auto-assigned (the old behaviour); a non-empty list restricts
auto-assign to the listed extensions and anything else falls back
to Documents. `excluded_sites` is a host list (exact host or any
subdomain) that Fetchline refuses to download from:
`add_download` refuses before any probe, `fetch_page_links`
refuses an excluded page and flags excluded links with an error
instead of probing them, and batch lines to excluded sites are
reported with their line number and skipped. Both lists are
normalised on save (extensions lowercased, no dots, de-duplicated;
hosts lowercased, no scheme/path). The Settings dialog now edits
both: "Excluded sites" in the General tab, "Auto-assign file types"
in the File Types tab. Regression tests:
`file_types_auto_limits_auto_assign`,
`excluded_sites_blocks_add_download`,
`fetch_page_links_flags_excluded_sites`,
`batch_add_flags_excluded_site_lines`,
`update_settings_normalizes_file_types_and_excluded_sites`.
Verified: full suite 106 integration + 12 unit, 0 failed, 2
consecutive runs. Files: `app/engine/src/store.rs`
(`category_for_extension_filtered`, `site_is_excluded`,
`clean_extensions`, `clean_excluded_sites`, `update_settings`),
`app/engine/src/manager.rs` (`add_download`, `fetch_page_links`),
`app/src/components/SettingsDialog.tsx` (both fields).

---

## Known limitations, not bugs (round-2 additions)

- No Windows `.exe`; the Tauri shell does not compile on this Linux
  VM (missing `gdk-3.0`) — all 35 Tauri commands are uncompiled,
  and the Settings dialog, proxy Test button and status-bar cap
  button are UI-only until the first Windows build.
- DPAPI password storage has never run on Windows; on this VM proxy
  passwords are base64-obfuscated only (`b64:` prefix), which is
  NOT encryption.
- A proxy change applies to new downloads and to segments created by
  a dynamic split; a segment already running keeps the HTTP client
  it started with, so it finishes through the old proxy (verified
  by code review of `manager.rs`; see test-plan.md S09-I4).
- The proxy password is never logged anywhere in the codebase
  (checked by searching for password output in `engine/src`,
  `src-tauri/src`, `src/`), and the UI row carries only
  `has_password` (verified by
  `proxy_row_serialised_contains_no_password`).
- Passwords for the Test button are not read back from storage: the
  user must re-type a saved password to test with it (the dialog
  says so next to the field).

## To check (round-2 additions, not reproduced, needs the desktop / Windows)

- S09-T1: SettingsDialog partial save — settings are saved first,
  then each changed category is saved in turn; if a later category
  fails, the earlier saves persist and the dialog shows the error
  (code review of `SettingsDialog.tsx` `save()` only).
- The 6 manual UI cases in the round-2 test plan (Settings dialog,
  default action / confirm-delete, status-bar button, proxy Test,
  theme, status-bar cap) — all blocked on the desktop binary.

## Round 3 — Browser extension (S13 / F01) — 2026-11-14

Build: slices 1–5 plus S13. Engine: 109 integration + 13 unit +
19 new extension tests, 0 failed. `npm run build` passes;
`npm run test:extension` passes. No new bug was found in this
round: the extension tests were written failing-test-first and all
19 passed once the implementation landed (two failures on the
first run were a missing manifest — the extension did not exist
yet — and a wrong count in the test itself, which expected 7
page links where the extractor also returns the page's stylesheet
link, making 8; neither was a product bug). One compile error was
caught by the compiler during development (a moved `String` in
the host's `db_candidates`) and fixed before anything shipped;
it is recorded in the build log, not as a BUG entry.

### To check (not reproduced, needs the desktop / Windows)

- The Windows handshake: host manifest registry install, the
  extension ID in `allowed_origins`, and the first drain against
  the real desktop app's database on Windows (the exact
  app-data folder still needs confirming there).
- A real browser pass of the popup, the context menu and the
  download watcher (no browser automation on this VM).
- A drained link's S02 confirm step: the drain leaves downloads
  queued, and no dialog opens for them yet.

## Round 4 — Phase 2 verification of S13 (2026-11-14)

Independent verification pass over the whole app including S13:
full engine suite run twice (109 integration + 13 unit + 21
extension tests, 0 failed), `npm run build` passes,
`npm run test:extension` passes, `node --check` passes on both
extension scripts, `cargo check --tests` warning-free, and the
real `fetchline-host` binary driven with native-messaging frames
against a local HTTP server only (27 smoke checks — ping,
get_settings, enqueue paths, exclusion, integration-off,
malformed/oversized/truncated frames, locked DB, pre-S13
migration, drain of the host-produced SQLite file: 7 accepted
with distinct save paths, 1 dead link rejected with 404, double
drain a no-op). Two new bugs were found by the smoke test and
fixed failing-test-first in this round (BUG-012/013 below).
BUG-001–011 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

### BUG-012: "Download all links on this page" bypasses the browser-integration off switch (and an all-skipped Download All reports success)

- Severity: S2
- Flow / case: F01 / F01-N5, F01-N6
- Screen: S13 (extension context menu → host)
- Build: 2026-11-14 (S13)

Steps
1. Turn browser integration off in Fetchline Settings
   (`settings.browser_integration = 0`).
2. In the browser, right-click a page and choose "Download all
   links on this page with Fetchline" (the extension sends
   `download_all` with `links: []`; the host expands the page).
3. Turn browser integration back on and wait for the next drain
   (or app start).

Expected: nothing is queued while catching is off — the desktop
setting is the master switch, and the host boundary refuses
everything while it is off (as `download_link` does).
Actual: the host replied `{"ok": true, "enqueued": 2}` and two
rows landed in `pending_links` while the flag was off. The
drain's own re-check only rejects rows while the flag is still
off, so once the user turned it back on, those links were
downloaded — files the user had explicitly declined to catch.
A second, smaller symptom in the same handler: a `download_all`
with links where every link was refused (e.g. integration off)
replied `ok: true` with `enqueued: 0` and the refusal buried in
a message string, so the extension treated it as success and
the user was never told nothing was queued.
Evidence: host smoke test, fresh DB, local HTTP server only —
`C2 download_all empty links when integration off enqueues
NOTHING` failed before the fix (reply `ok:true, enqueued:2`,
rows 7 → 9); `C3` failed the same way (`ok:true, enqueued:0`
with the refusal only in `message`). The `download_link` path
(C1) and the excluded-page path (B3) were already correct.
Suspected cause: the host binary's empty-links branch called
`Manager::enqueue_page_links` directly, and that function never
checked the flag (unlike `handle_host_message`'s `enqueue_one`,
which every other path uses). The `DownloadAll` handler in
`native_host.rs` built an `ok` reply whenever the loop finished,
even with zero enqueued and every link skipped.
Status: FIXED. `Manager::enqueue_page_links` now refuses with
"Browser integration is turned off in Fetchline settings"
before fetching the page, so the boundary holds for every
caller; `handle_host_message` returns an error reply when a
`download_all` enqueues 0 links and at least one was skipped
(a partial skip still returns `ok: true` with the message, as
before). Regression tests:
`enqueue_page_links_refuses_when_browser_integration_is_off`
(failed before the fix: `Ok(8)`), 
`host_download_all_all_skipped_is_an_error` (failed before the
fix: `ok: true`). Re-verified by the host smoke test after the
fix: C2/C3 both PASS (error reply, row count unchanged).
Files: `app/engine/src/manager.rs` (`enqueue_page_links`),
`app/engine/src/native_host.rs` (`DownloadAll` branch).
Full suite after the fix: 109 integration + 13 unit + 21
extension, 0 failed, 2 consecutive runs.

### BUG-013: A malformed JSON native message kills the host with no reply

- Severity: S4
- Flow / case: F01 / F01-E6
- Screen: S13 (native host)
- Build: 2026-11-14 (S13)

Steps
1. Send the host one length-prefixed frame whose payload is not
   valid JSON (e.g. `{not json`).
2. Send a `ping` in the same host process.

Expected: a framed error reply for the bad message, and the host
keeps serving (the same behaviour as an unknown message type,
which already gets a framed error and the loop continues).
Actual: the host exited at once with no reply at all — the
extension only learns the host died (`lastError`), not what was
wrong. (A truncated payload or an oversized length still exits
the host, which is correct: the stream can no longer be
trusted. A truncated frame with the pipe left open correctly
waits for the rest of the frame — it is not a hang, it is a
slow sender; closing the pipe ends it at once, verified.)
Evidence: host smoke test `D1 malformed JSON -> framed error
reply, host stays alive` failed before the fix (reply `None`,
then `BrokenPipeError` on the next ping).
Suspected cause: `main.rs` treated every `decode_message`
error the same — log and break. A JSON parse failure happens
after a good length prefix, so the stream is still aligned
and a reply is possible.
Status: FIXED. The host loop now distinguishes the error: a
`EngineError::Json` gets a framed error reply and the loop
continues; framing/I-O errors still end the process. Verified
by the host smoke test after the fix: D1/D2/D3 PASS, a
zero-length frame gets a framed error reply, oversized and
truncated frames still exit promptly (rc 0, no hang).
File: `app/host/src/main.rs`. No engine test covers the host
binary's loop (it is a `main`, not a library function); the
smoke script in `/tmp` is the regression check — re-run it
against any future change to the loop.

## Known limitations, not bugs (round-4 additions)

- A locked database makes the host wait ~5 s (rusqlite's
  default busy timeout) and then exit 1 with "Database error:
  database is locked" on stderr — clean, but the extension
  only sees the host exit. Transient locks from the desktop
  app's own short writes are absorbed by the same timeout.
- The download watcher cancels the browser download *before*
  the hand-off to the host (background.js): if the hand-off
  then fails, the browser download is already gone and the user
  must right-click the link and try again. Documented in the
  code; no automatic retry, by design (a retry could download
  the file twice).

## Round 5 — Video grabber (S08 / F05) — 2026-11-14

Build: slices 1–5 plus S13 plus S08. Engine: 109 integration +
13 unit + 21 extension + 21 video tests, 0 failed. Extension JS
tests: 11 passed, 0 failed. Host smoke: 11/11 checks against the
real binary. Three bugs were found by the new tests, all fixed
failing-test-first in this round (BUG-014/015/016 below).
BUG-001–013 re-checked: all regression tests pass, no regression.
No open bugs remain in this file.

### BUG-014: A live (.m3u8) stream was invisible to the video detector

- Severity: S2
- Flow / case: F05 / F05-E5
- Screen: S08 (video panel, desktop + extension)
- Build: 2026-11-14 (S08)

Steps
1. Open a page with a live stream (`<video src="stream.m3u8">`).
2. Scan the page for videos (desktop panel or extension).

Expected: the stream is listed as a live item (M3U8), offered as
a playlist file.
Actual: nothing is found — the page reports "No downloadable video".
Evidence: cargo test `fixture_live_stream_is_marked_live` (failed
before the fix: 0 videos found) and the JS test of the same
fixture.
Suspected cause: `detect_videos_in_html` (and the JS detector's
`extractMediaFromHtml`) filters by `classify_kind`, and
`classify_kind` had no arm for `m3u8` — it fell through to
`Other`, so the media filter dropped it. The extension was not
involved: both detectors share the rule.
Status: FIXED. `classify_kind` now maps `m3u8` to Video (a
playlist is a video stream, and Download All groups it with
video too — the same fix, in the shared classifier). Regression
tests: `fixture_live_stream_is_marked_live` (Rust + JS).
Files: `app/engine/src/links.rs` (`classify_kind`).

### BUG-015: The host trusted an invented quality claim ("Ultra HD 8K Pro")

- Severity: S3
- Flow / case: F05 / F05-N7
- Screen: S08 (video panel)
- Build: 2026-11-14 (S08)

Steps
1. Send a `video_found` whose variant claims quality
   "Ultra HD 8K Pro" for a plain `clip.mp4` with no label and no
   dimensions.
2. Look at the queued variant's quality.

Expected: "Unknown quality" — the claim is marketing copy, not a
quality the sender ever saw.
Actual: the claim was kept verbatim ("Ultra HD 8K Pro"), so the
panel would promise a quality that does not exist.
Evidence: cargo test
`video_validation_replaces_an_invented_quality_and_keeps_a_real_one`
(failed before the fix: kept "Ultra HD 8K Pro").
Suspected cause: the quality backstop in
`validate_video_variants` kept a claim when
`quality_from_label(claim)` found a resolution *inside* it —
"8K" appears in "Ultra HD 8K Pro", bounded by spaces. A
substring check is not a label check.
Status: FIXED. The backstop now keeps a claim only when it IS a
clean label — the whole string, case-insensitively, is one of
`720p` / `1080p` / `4K` / `8K` (etc.) or a dimension label
(`1280 × 720`); anything else is replaced by the honest
derivation ("Unknown quality" here). A real "720p" claim is
still kept. Regression test:
`video_validation_replaces_an_invented_quality_and_keeps_a_real_one`.
File: `app/engine/src/video.rs` (`is_clean_quality_label`).

### BUG-016: Every <source> quality inherited the playing video's dimensions

- Severity: S3
- Flow / case: F05 / F05-E1
- Screen: S08 (extension floating panel)
- Build: 2026-11-14 (S08)

Steps
1. Open a page with a `<video>` playing at 1920×1080 and a
   `<source src="movie-720p.mp4">` child.
2. Scan the page with the extension detector.

Expected: the 720p source is labelled "720p" (from its own URL).
Actual: it was labelled "1920 × 1080" — the parent element's
dimensions, which describe whichever source is currently playing,
were lent to every `<source>` child.
Evidence: Node test "live DOM scan reads real dimensions, live
and DRM state" (failed before the fix: expected "720p", got
"1920 × 1080").
Suspected cause: `scanLiveMedia` passed the parent's
`videoWidth`/`videoHeight` into the `<source>` loop. The Rust
detector never had this bug (it derives source qualities from
their URLs only).
Status: FIXED. `<source>` children are now scanned with no
dimensions, so their quality comes from their own URL label —
or "Unknown quality" when they have none, which is honest: that
source's own quality is unknown. Regression test: the Node live
DOM scan test above. File:
`app/extension/video-detect.js` (`scanLiveMedia`).

## Known limitations, not bugs (round-5 additions)

- The desktop video panel cannot detect DRM from static HTML: a
  protected stream is only discovered when its probe fails, and
  the panel shows that error. The extension panel can detect it
  (live `mediaKeys`), the desktop panel cannot.
- An `.m3u8` playlist is offered as-is and saved as-is; HLS/DASH
  segmented streams are never merged into a single video file.
- No cookies are ever sent, so a video that needs a login fails
  at the probe — the same limit as S13.
- The extension's floating button and panel have never run in a
  real browser on this VM; the `detect_page_videos` Tauri command
  is uncompiled (missing `gdk-3.0`), as with every earlier
  milestone.

## Round 6 — Site grabber (S06 / F06) — independent verification, 2026-11-14

Build: slices 1–5 plus S13 plus S08 plus S06. Shipped suite re-run
twice: 109 integration + 21 unit (17 old + 4 in `grabber.rs`) + 21
extension + 15 grabber + 21 video = 187, 0 failed. `npm run build`
passes; `npm run test:extension` passes; `node --check` passes on
all four extension JS files; extension video-detector Node tests 11
passed, 0 failed. BUG-001–016 re-checked: all regression tests pass,
no regression. Three new bugs were found by the verifier's own
adversarial tests in `app/engine/tests/grabber_verify_tests.rs`
(14 tests: 10 pass, 4 fail — the 4 failures are the tests for
BUG-017/018/019 below, kept failing-test-first). No app code was
changed by the verifier. BUG-017–019 were OPEN at verification
time; all three were FIXED on 2026-11-15 (see each entry), and the
verifier suite now passes 14/14 on three consecutive runs, with
the full shipped suite at 201/201, 0 failed.

### BUG-017: A robots.txt Allow in the Fetchline group loses to a Disallow in the `*` group when the `*` group comes first

- Severity: S3
- Flow / case: F06 / F06-E1 (site blocks crawling — robots.txt)
- Screen: S06 (engine crawler)
- Build: 2026-11-14 (S06)

Steps
1. Serve a robots.txt whose `*` group comes first and disallows a
   subtree, with a Fetchline group allowing a sub-path under it:
   `User-agent: * / Disallow: /private/` then `User-agent:
   Fetchline / Allow: /private/open/`.
2. Run a grabber over a site with `/private/page.html` (links
   `/private/secret.pdf`) and `/private/open/page.html` (links
   `/private/open/open.pdf`), depth 2, all types.

Expected: `/private/open/page.html` is crawled and `open.pdf` is
matched — the build log says "an Allow in a Fetchline group must
beat a Disallow in the `*` group" — while `/private/page.html`
stays blocked.
Actual: the open page is skipped too: 4 pages explored instead of
5, `open.pdf` never matched. The same Allow works when the
Fetchline group is written first, so the result depends on group
order in the site's robots.txt, which the user does not control.
Evidence: verifier tests
`verify_robots_fetchline_allow_beats_star_disallow_both_orders`
(failed: the star-first case returns "disallowed" for
`/private/open/page.html`) and
`verify_robots_allow_under_disallow_cross_group_crawl` (failed:
4 pages, no `open.pdf`). The same-group case passes
(`verify_robots_allow_under_disallow_same_group`: Allow beats
Disallow inside one group), and a blocked start page still fails
cleanly, so the parser is only wrong across groups.
Suspected cause: `robots_disallows` in
`app/engine/src/grabber.rs` returns on the first applying group it
meets — it checks that group's Allow, then its Disallow, and
returns at once — instead of collecting the most specific
(Fetchline) group's rules first and letting its Allow win over
the `*` group's Disallow.
Status: FIXED (2026-11-15). Fix: `robots_disallows` now collects
the rules of every applying group and decides once — any matching
`Allow` in any applying group (in particular the Fetchline group)
beats any matching `Disallow` in any other (in particular `*`),
whichever group comes first in the file. Regression tests kept:
the two failing tests above in
`app/engine/tests/grabber_verify_tests.rs`, now passing 3/3 runs.

### BUG-018: A site excluded after the project was created is still crawled (and listed in explore-only mode)

- Severity: S3
- Flow / case: F06 / F06-N3 (excluded site)
- Screen: S06 (engine crawler)
- Build: 2026-11-14 (S06)

Steps
1. Create a grabber project for `http://127.0.0.1:<port>/`
   (not excluded yet).
2. Add `127.0.0.1` to Settings → Excluded sites.
3. Run the project in explore-only mode.

Expected: the run refuses before any network request — the same
refusal `create_grabber_project` gives for an excluded start site —
and lists no files. Excluded sites are a user control: Fetchline
refuses to download from them (BUG-011).
Actual: the run fetches `GET /robots.txt` and `GET /index.html`
(2 requests observed on the fixture server), crawls, and finishes
with status `done`, listing the site's files. In download mode the
files would then fail one by one at `add_download` (which does
check exclusion), but the pages are still crawled and explore-only
mode lists files from a site the user excluded.
Evidence: verifier test
`verify_excluded_after_create_stops_the_run_before_any_fetch`
(failed: 2 requests, `["GET /robots.txt", "GET /index.html"]`,
status `done`). The create-time check passes
(`verify_excluded_start_site_makes_no_request`: error, 0 requests).
Suspected cause: `run_grabber_task` in
`app/engine/src/manager.rs` never re-checks
`site_is_excluded` — the exclusion check lives only in
`create_grabber_project` (and in `add_download`, which runs too
late to stop the crawl). The scheduler tick path has the same gap:
it starts idle projects without checking exclusion either.
Status: FIXED (2026-11-15). Fix: `run_grabber_task` re-checks the
start URL's host against `excluded_sites` at its start, before
fetching robots.txt or any page, and fails the run with the same
"excluded" message as `create_grabber_project` — zero network
requests, nothing listed, in explore-only mode too. The
scheduler-tick grabber branch in `scheduler_tick_at` got the same
check: an excluded idle project is failed with the same message
instead of being started, so it is not retried on every tick.
Regression test kept: the failing test above in
`app/engine/tests/grabber_verify_tests.rs`, now passing 3/3 runs.

### BUG-019: `run_grabber` returns before the project reads as running, so a poller can see a stale terminal state

- Severity: S3
- Flow / case: F06 / F06-M1 (wizard progress view)
- Screen: S06 (`SiteGrabberDialog` progress view)
- Build: 2026-11-14 (S06)

Steps
1. Create a grabber project, run it, wait until it is `done`.
2. Run the same project again and immediately read it back with
   `get_grabber_project`.

Expected: the project already reads `exploring` (or
`downloading`) — the run has started, and a poller must never
conclude from a stale `done` that the new run is over.
Actual: the immediate read still says `done` — the background
task is what first persists `exploring`, and `run_grabber` returns
before that write happens. The verifier's own wait loop hit the
same trap: it returned the run-1 row at once and had to be taught
to wait for a running state first.
Why it matters in the UI: `SiteGrabberDialog` only polls while the
status is `exploring`/`downloading`. On a fresh run it calls
`getGrabberProject` right after `runGrabber`; if that read lands
before the task's first write, the dialog shows `idle` ("Waiting
for its scheduled start time") and never starts polling, so the
crawl runs to completion unseen. On a re-run the same stale read
shows the old finished summary and the new run's progress never
appears. In practice the task usually wins the race (the
immediate write is fast), so this is timing-dependent, not
every-run.
Evidence: verifier test
`verify_rerun_status_is_stale_immediately_after_run` (failed:
status immediately after re-run is `done`; reproduced 3/3 runs).
Suspected cause: `run_grabber` in
`app/engine/src/manager.rs` spawns `run_grabber_task` and returns
without synchronously setting the status to `exploring` first.
Status: FIXED (2026-11-15). Fix: `run_grabber` now writes the
project status to `exploring` (counts reset) synchronously via
`update_grabber_project`, BEFORE spawning the task, and registers
the run in `grabber_runs` before spawning too (so `stop_grabber`
and a second `run_grabber` see it immediately). Any read after the
call now sees a running state. UI side, by source inspection
(`SiteGrabberDialog.tsx`): the dialog reads the project back right
after `runGrabber` and only starts its 1 s poll while the status
is `exploring`/`downloading` — with the synchronous write that
read now returns `exploring`, so the dialog can no longer get
stuck on `idle` / a stale summary for this reason; no UI change
was needed. Direct observation of the dialog still needs the
desktop binary (F06-M2 stays BLOCKED).
Regression test kept: the failing test above in
`app/engine/tests/grabber_verify_tests.rs`, now passing 3/3 runs
(it reproduced the race 3/3 before the fix).

## Known limitations, not bugs (round-6 additions)

- `stop_grabber` during the download phase waits for the current
  file to finish (the stop flag is only checked between files) —
  by design, per the code comment ("it never kills a download
  mid-chunk"). Only the crawl-phase stop was measured: 508 ms
  from Stop to `stopped` with 2 of 7 pages explored.
- The crawl's politeness was measured, not just read from the
  constant: smallest gap between consecutive page fetches was
  504 ms over 6 pages (500 ms sleep + request time).
- A scheduled project that is manually Run starts immediately,
  ignoring its schedule (already noted in the build log).
- The 5 new Tauri commands are uncompiled on this Linux VM
  (`gdk-3.0`), as with every earlier milestone; the wizard is
  verified via `npm run build` + source inspection only. No
  Windows `.exe` exists. The crawl has never run against a real
  website — all checks used local fixture servers only.

## To check (round-6 additions, not reproduced, needs the desktop / Windows)

- F06-M1: the wizard click-through (all 3 steps, Run, live
  progress, Stop, finished/failed/stopped states) in the real
  desktop app — blocked on the binary, as with every milestone.
- F06-M2: BUG-019's UI symptom (dialog stuck on idle / stale
  summary) in the real app — timing-dependent, needs the binary.
  The engine-side race is FIXED (2026-11-15: synchronous
  `exploring` write in `run_grabber`); direct observation of the
  dialog still needs the binary.
- The Windows handshake for the 5 new Tauri commands (same
  unproven status as every earlier milestone).

## Round 7 — Periodic synchronisation (S05 / F04) — 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic sync.
Engine: 111 integration + 21 unit + 21 extension + 15 grabber + 14
verifier + 15 sync + 21 video = 218, 0 failed, 2 consecutive full
runs. `cargo check --tests` warning-free (0 warnings — the shared
test mock's dead-code warnings in the `engine_tests` binary are now
silenced with `#![allow(dead_code)]`, the same pattern
`video_tests.rs` / `extension_tests.rs` already use). `npm run build`
passes (tsc + vite). One new bug was found by the new sync tests,
fixed failing-test-first (BUG-020 below). BUG-001–019 re-checked:
all regression tests pass, no regression. No open bugs remain in
this file.

### BUG-020: A sync retry of a failed re-download reports the stale error at once and never retries

- Severity: S2
- Flow / case: F04 / F04-Y6 (sync retry after a failed re-download)
- Screen: S05 (engine sync; the dialog would show the same failure)
- Build: 2026-11-15 (S05 periodic sync)

Steps
1. Put a finished file in a periodic sync queue; change the file on
   the server (new size + ETag).
2. Make the server fail GETs with 500 and run a sync.
3. The sync sees the change, tries to re-download, fails after the
   segment retries, and records the file as `failed` (correct).
4. Make the server healthy again and run a second sync.

Expected: the second sync retries the file — resume re-probes,
continues (or restarts) the download, and records `changed` with
the new bytes on disk.
Actual: the second sync returns at once with `failed` and the old
"Server replied 500" message. No GET is made at all (server GET
count unchanged), the file stays in `error` with 0 bytes
re-downloaded, and every later sync repeats the same instant
failure — the retry rule ("failed probes are retried at the next
sync") never happens for a failed re-download.
Evidence: cargo test
`sync_reports_failed_probe_and_retries_at_the_next_sync` (failed
before the fix: second sync `changed: 0, failed: 1`; a manual GET
against the same server at the same moment returned 200, and the
server's GET counter did not move during the second sync, proving
the failure was reported without any network request).
Suspected cause: `wait_for_download` in
`app/engine/src/manager.rs` polled the stored status and treated
any `error` as terminal. A file being retried still reads `error`
with the old message until its new run flips the status to
`downloading`, so the first poll saw the stale error and returned
it. The queue runner already guards against this exact trap with
its "ignore the pre-start status until downloading is seen" rule;
the sync waiter did not have any equivalent guard.
Status: FIXED. `wait_for_download` now treats an `error` as
terminal only when the download is no longer registered in the
manager's `running` map — `start_download` registers the run
synchronously and `run()` unregisters it when done, so the map is
the exact guard (a status-only guard would still misread a failure
faster than the 200 ms poll). Regression test kept:
`sync_reports_failed_probe_and_retries_at_the_next_sync` (now
passing; the second sync re-downloads the file byte-identical).
Verified: full suite 218/218, 0 failed, 2 consecutive runs.
File: `app/engine/src/manager.rs` (`wait_for_download`).

## Known limitations, not bugs (round-7 additions)

- A sync run waits up to 120 s per re-downloaded file
  (`wait_for_download`); a slower file reports `failed` with "did
  not finish in time" and is retried at the next sync.
- A file the server describes with no size and no validator at all
  (no Content-Length, ETag or Last-Modified) is reported `skipped`
  with "cannot tell whether this file changed" — sync refuses to
  guess, in either direction.
- A periodic queue's stop time ends a sync after its current file
  (the stop flag is checked between files, like a queue drain) —
  it never kills a download mid-chunk.
- The `sync_queue_now` Tauri command and the `update_queue`
  interval parameter are uncompiled on this Linux VM (`gdk-3.0`),
  as with every earlier milestone; the Scheduler dialog changes
  are verified via `npm run build` + source inspection only. No
  Windows `.exe` exists. Sync has never run against a real
  website — all checks used local fixture servers only.

## To check (round-7 additions, not reproduced, needs the desktop / Windows)

- F04-M4: the Scheduler dialog's periodic radio, interval field,
  last-sync status and "Sync now" button in the real desktop app
  — blocked on the binary, as with every milestone.
- The Windows handshake for the `sync_queue_now` command (same
  unproven status as every earlier milestone).
- A sync against a real website that sends `Last-Modified` but no
  ETag (the mock sends ETag + size; the Last-Modified comparison
  path is covered by code only, not by a fixture that sends the
  header).

## Round 8 — Finish browser catch (S13 / F01) — Milestone 2, 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus the S13 finish. Engine: 222 total, 0 failed, 2
consecutive full runs (21 unit + 111 integration + 25 extension
+ 15 grabber + 14 verifier + 15 sync + 21 video). Extension JS:
21/21 (`node --test`). Host smoke: download-all 14/14, video
11/11. **No new bug was found in this round: no BUG-021.**
BUG-001–020 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

Two wrong test expectations were corrected during this round;
neither was a product bug, so neither gets a BUG entry:

- The first `download-all-smoke.py` D8 expected an explicit
  `download_all` list containing a dead link to enqueue only the
  good one. The host enqueues without probing by design (the
  probe happens at drain time, through `add_download`), so both
  links are queued and the dead one is rejected at the drain —
  pinned by the existing engine test
  `drain_marks_dead_link_rejected_and_keeps_going`. The smoke
  check was corrected to assert enqueued 2.
- The first `popup.test.js` / `background.test.js` runs failed on
  the fakes, not the extension: the fake `sendNativeMessage`
  took 2 arguments where the real API takes 3 (host, message,
  callback), and `assert/strict`'s `deepEqual` rejects a message
  built inside the `vm` realm (different Object prototype) even
  with identical structure. Fixed in the tests (3-arg stub; JSON
  string comparison).

## Known limitations, not bugs (round-8 additions)

- A failed drain start (`DrainSummary.start_failed`) has no
  triggering test: a fresh download's `start_download` cannot
  fail in the paths exercised here, so the recording is
  code-reviewed only.
- A second `download_all` for the same page enqueues the same
  links again (enqueueing does not de-duplicate against the
  queue) — the drain's duplicate-path guard (BUG-002) is what
  gives the second copy a different save path. Pinned by smoke
  check D5.
- The "Download all links" context-menu item already existed
  from S13; this milestone verified it rather than adding it,
  and no new permission was needed (`contextMenus` already
  covers a page-context item).

## To check (round-8 additions, not reproduced, needs the desktop / Windows)

- F01-T1: the Windows handshake — registry key, extension ID in
  `allowed_origins`, app-data path, first live handoff — now
  written as a checklist in `app/extension/README.md`; none of
  its steps has run.
- F01-M4/M5/M6: the drain starting a download in the real
  desktop app, the popup's pending count / last hand-off, and
  the Download All menu in a real browser — all blocked on the
  binary / a real browser, as with every milestone.

## Round 9 — Finish video grabber (S08 / F05) — Milestone 3, 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus the S08 finish (HLS decision + playlist understanding).
Engine: 235 total, 0 failed, 2 consecutive full runs (24 unit +
111 integration + 25 extension + 15 grabber + 14 verifier + 15
sync + 31 video; was 222 — +3 unit in `video.rs`, +10 video
tests). Extension JS: 27/27 (`node --test`: 17 video-detect +
6 popup + 4 background; was 21 — +6 video-detect). Host smoke:
video 13/13 (was 11/11 — +2: real .m3u8 and .mpd handoffs),
download-all 14/14 re-run green. `cargo check --tests`
warning-free. `npm run build` passes; `npm run test:extension`
passes; `node --check` passes on all four extension scripts.
One new bug was found by the new tests, fixed failing-test-first
(BUG-021 below). BUG-001–020 re-checked: all regression tests
pass, no regression. No open bugs remain in this file.

### BUG-021: An encrypted HLS playlist was never flagged as encrypted

- Severity: S3
- Flow / case: F05 / F05-N10 (new case in this milestone)
- Screen: S08 (desktop video panel + extension detector)
- Build: 2026-11-15 (S08 finish, Milestone 3)

Steps
1. Parse an `.m3u8` media playlist containing
   `#EXT-X-KEY:METHOD=AES-128,URI="key.bin"` (the shared fixture
   `extension/tests/fixtures/encrypted.m3u8`).
2. Read the parsed `encrypted` flag (Rust `parse_m3u8` / JS
   `parseM3u8`), or scan a page linking that playlist with
   `detect_page_videos`.

Expected: `encrypted` is true, and the desktop panel lists the
playlist as not downloadable ("encrypted — the playlist alone
will not play").
Actual: `encrypted` was false in BOTH parsers, so the playlist
was offered as a normal downloadable playlist — a user saving it
would get a file that cannot play, with no warning.
Evidence: new tests `parseM3u8: VOD counts segments, live has no
end, encrypted is flagged` (JS, failed: `false !== true` on
`enc.encrypted`) and `hls_parse_matches_shared_fixtures` (Rust,
same assertion, would have failed the same way). Both parsers
were written in this milestone and the bug was in the new code,
not a regression — the tests were written first against the
spec, then the parsers, and the test run caught it before any
ship.
Suspected cause: the `#EXT-X-KEY` attribute scan split the whole
line on commas and looked for an attribute whose key is exactly
`METHOD` — but the first attribute on the line is
`#EXT-X-KEY:METHOD=AES-128`, so its key reads
`#EXT-X-KEY:METHOD`, never `METHOD`, and the match silently
missed. The `METHOD=NONE` (not encrypted) case passed by
accident, because a missed match also reads as not encrypted.
Status: FIXED. Both parsers now strip the `#EXT-X-KEY:` tag
name before splitting the attributes, so `METHOD` is read
correctly; `METHOD=NONE` still reads as not encrypted.
Regression tests kept: the two tests above (JS + Rust, same
fixture), plus the end-to-end
`detect_page_videos_encrypted_playlist_is_not_downloadable`
(page links the encrypted playlist; the panel entry is
`downloadable: false` with an error naming encryption).
Files: `app/engine/src/video.rs` (`parse_m3u8`),
`app/extension/video-detect.js` (`parseM3u8`).

### BUG-022: NTLM stub proxy closed the connection on Basic auth instead of challenging

- Severity: S3 (test fixture only — no product code was wrong)
- Flow / case: S09-P / NTLM proxy auth (new cases in this milestone)
- Screen: none (engine test fixture)
- Build: 2026-11-15 (S09 Proxy finish, PARTIAL 4 of 4)

Steps
1. Run the new NTLM tests in `app/engine/tests/proxy_tests.rs`
   (`ntlm_proxy_authenticates_http_download`,
   `ntlm_proxy_wrong_password_is_rejected`,
   `ntlm_proxy_test_button_succeeds_and_wrong_password_fails`,
   `kerberos_only_proxy_is_refused_clearly`,
   `ntlm_proxy_https_test_url_is_refused_clearly`).
2. The engine probes through the stub proxy with a proxy
   credential configured, so reqwest sends
   `Proxy-Authorization: Basic ...` on the first request.

Expected: the stub answers 407 with `Proxy-Authenticate: NTLM`
(a real NTLM-only proxy challenges a Basic answer, it does not
go silent), and the engine then runs the Type 1/2/3 handshake.
Actual: the stub only answered when the header was absent or a
valid NTLM Type 1/3; any other value (including Basic) made the
handler return, closing the connection with no response at all.
The client failed with a bare hyper `IncompleteMessage` /
"error sending request" instead of ever reaching the handshake
— all 5 NTLM tests failed that way on their first run, while
all 10 FTP tests passed.
Evidence: `cargo test --test proxy_tests` first run: 10 passed,
5 failed, every failure an `IncompleteMessage` or network error
from the probe, none an auth error.
Suspected cause: the fixture's auth branch treated "not NTLM"
as "give up" (`return`) instead of "not yet authenticated"
(407 challenge). Real proxies challenge; the fixture did not.
Status: FIXED. The stub now treats any non-NTLM
`Proxy-Authorization` (Basic included) as unauthenticated and
re-issues the 407 challenge on the same connection; the 407
send was factored into a shared `send_407` helper. No engine
code changed for this bug.
Regression tests kept: all 5 NTLM tests above (they failed
before the fix, pass after).
Files: `app/engine/tests/proxy_tests.rs`
(`handle_ntlm_proxy` / `send_407`).
Note on numbering: checked `bugs.md` first — the highest
defined entry was BUG-021; the "no BUG-022" lines elsewhere in
this file are other workers' round notes, not entries.

## Known limitations, not bugs (round-9 additions)

- A master playlist's variant entries are probed (HEAD) but the
  variant playlist fetch can fail while the probe succeeds (e.g.
  a variant playlist that answers HEAD but not GET): the entry
  is then listed with the generic playlist note and `live:
  false`, which is honest about what was seen — the probe says
  it is downloadable, the note does not claim liveness either
  way.
- A variant playlist URL that is dead (probe fails) is listed as
  not downloadable rather than hidden, so the user sees why that
  quality is unavailable (pinned by
  `detect_page_videos_master_variant_list_is_capped`).
- The extension-side `parseM3u8` is exercised by the Node tests
  over the same playlist fixtures as the Rust parser; the
  content script itself does not fetch playlists (a cross-origin
  fetch from the page would usually be blocked), so variant
  expansion in the browser panel is the desktop panel's job —
  the extension lists the master playlist as one item with the
  honest note.
- No change to the HLS decision: the panel never claims a
  playlist is a merged video, and no segment is ever fetched by
  the scan (only the playlist text, capped at 256 KiB).

## To check (round-9 additions, not reproduced, needs the desktop / Windows)

- F05-M1/M2/M3: the floating button, the extension panel's new
  playlist notes, and the desktop panel's expanded variant list
  in a real browser / the real desktop app — all blocked on a
  real browser and the binary, as with every milestone.
- F05-T1: the Windows handshake for video (unchanged from
  Round 4).

## Round 10 — Finish site grabber (S06 / F06) — Milestone 4, 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus the S06 finish (per-project queue choice +
on-completion action reusing the existing queue completion
machinery). Engine: 248 total, 0 failed, 2 consecutive full
runs (24 unit + 111 integration + 25 extension + 28 grabber +
14 verifier + 15 sync + 31 video; was 235 — +13 new grabber
tests). `cargo check --tests` warning-free. `npm run build`
passes. **No new bug was found in this round: no BUG-022.**
BUG-001–021 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

No wrong test expectation needed correcting in this round: the
13 new tests passed on the first full run against the new code.

## Known limitations, not bugs (round-10 additions)

- A grabber-armed action and a queue-drain action for the SAME
  queue share one `PendingAction` slot (keyed by queue id): if
  both fire, the second overwrites the first. The slot is the
  reuse that makes the countdown shared, so this is a
  consequence of the design, not an oversight — documented in
  the build log and test plan (F06, Round 9); not tested (needs
  two runs racing, timing-dependent).
- A shutdown / hibernate action armed by a grabber run is never
  executed in a test: the cancel path is tested by dismissing
  the pending action and asserting a later execute fails, and
  executing a shutdown off Windows is covered by the existing
  queue test (`queue_action_shutdown_errors_off_windows`).
- The stopped-after-downloading test is timing-sensitive by
  nature (the stop must land mid-download-phase); it passed on
  the first run and twice in the full suite, with a 50 ms poll
  and ~3 s of download phase behind it. If it ever flakes under
  load, lengthen the phase rather than weakening the assertion.

## To check (round-10 additions, not reproduced, needs the desktop / Windows)

- F06-M3: the wizard's queue dropdown and completion-action
  picker, and the 60 s countdown dialog appearing for a
  grabber-armed action, in the real desktop app — blocked on
  the binary, as with every milestone (the Tauri
  `create_grabber_project` command changes are uncompiled).
- F06-T2: a crawl against a real website — never attempted, by
  rule (real-website crawling is forbidden as a test).

## Round 11 — Finish progress dialog (S03) — Milestone 5, 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus the S03 finish (60 s speed graph, engine-driven retry
countdown with Retry now, Open folder on completion). Engine: 255
total, 0 failed, 2 consecutive full runs (24 unit + 111
integration + 25 extension + 28 grabber + 14 verifier + 7
progress + 15 sync + 31 video; was 248 — +7 new progress tests).
Speed-graph sampler: 8/8 (`node --test`). Extension JS: 27/27
re-run green. `cargo check --tests` warning-free. `npm run
build` passes; Vite preview serves the app (index 200,
`Fetchline` title). **No new bug was found in this round: no
BUG-022.** BUG-001–021 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

One wrong test expectation was corrected during this round; it
was a design wart, not a product bug, so it gets no BUG entry:

- The first `speedGraph.test.mjs` cap test expected a 240-sample
  cap to hold after recording 250 samples. It got 121: the 60 s
  window prune (500 ms cadence) bounds the list at 121 entries,
  so the cap could never be reached. The cap was removed from
  `speedGraph.ts` instead of tested into place, and the test now
  pins the window bound (500 samples over 250 s → 121 kept).

Two test-first observations, neither a product bug:

- The progress tests were written against the new retry state
  (`ProgressEvent.retry`), so they cannot run against the old
  code at all — the field does not exist there. They passed on
  their first full run against the new code (7/7, three
  consecutive runs), which is the structural equivalent of the
  failing-test-first check: the feature is the state itself.
- The `retry_now_skips_the_backoff` timing bound (second wait
  inside 1.5 s; without the feature it would be 2 s) passed on
  all three runs with margin (the observed gap is ~0.3–0.6 s),
  but it is the test most likely to flake under load — if it
  ever does, lengthen the bound toward 1.8 s rather than
  weakening the assertion to "eventually errors".

## Known limitations, not bugs (round-11 additions)

- The `open_folder` success path (the actual shell-open spawn)
  has never executed anywhere: it needs a desktop app and a
  file manager, and there is neither on this Linux VM. Only the
  validation is tested (folder resolves / no longer exists /
  unknown download). The spawn reuses the queue-action opener
  (`open_path`), which is itself only proven by the
  `open_file_action_executes` test on Linux — the Windows
  `cmd /C start` branch has never run anywhere.
- A pause during a retry wait clears the wait before the
  snapshot (code review of the two pause paths in `run_inner`
  plus the `run()` cleanup); there is no dedicated test that
  pauses mid-backoff and reads the snapshot — see test-plan.md
  S03-T1.
- The speed graph renders only while the dialog is open: samples
  are kept in the dialog's own state and reset when it opens for
  another download. There is no per-download speed history
  persisted anywhere (the `could` row "Speed graph history per
  download" stays `no`).

## To check (round-11 additions, not reproduced, needs the desktop / Windows)

- S03-M1/M2/M3/M4: the graph, the ticking countdown, the Open
  folder button (including the missing-folder error shown in the
  dialog), and a keyboard walk-through of the dialog in each
  state — all blocked on the binary, as with every milestone
  (the `retry_now` / `open_folder` Tauri commands are
  uncompiled).
- S03-T1: pausing mid-backoff in the running app (the engine
  side is code-reviewed only).

## Round 12 — Finish periodic synchronisation (S05 / F04) — Milestone, PARTIAL 2 of 4, 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus the S05 finish (Last-Modified fixture, ETag-only /
size-only variants, interval boundaries, empty queue, mid-download
skip, shrunk file). Engine: first full run 297 total, 0 failed
(28 unit + 111 integration + 25 extension + 16 final-verify + 28
grabber + 14 verifier + 14 options + 7 progress + 23 sync + 31
video; was 271 — +8 new sync tests, +4 unit and +14 options tests
from the parallel Options-dialog worker's suite, which landed
during this run). Sync suite 23/23 in both full runs. `cargo check
--tests` warning-free (0 warnings). `npm run build` passes.
**No new bug was found in this round: no BUG-022.**
BUG-001–021 re-checked: all regression tests pass, no regression.
No open bugs remain in this file.

Two anomalies were observed while verifying; neither is a product
bug, so neither gets a BUG entry:

- The BUG-020 regression test
  (`sync_reports_failed_probe_and_retries_at_the_next_sync`)
  failed once on the first sync-suite run — its finished file was
  suddenly missing from disk — then passed 3/3 in isolation and
  on the full re-run (23/23). The failure mode (a file vanishing
  mid-test) is consistent with a concurrent run's
  `/tmp/fetchline-test-*` cleanup deleting a live test directory
  while three other workers shared the VM, not with a product
  fault: the re-download path never deletes the finished file
  (only its `.part`), and the test asserts exactly that. No code
  was changed for it.
- The second full run failed one extension test,
  `extension_manifest_is_mv3_and_minimal` ("must not request
  cookies"): the parallel browser-catch worker had added a
  `"cookies"` permission to `app/extension/manifest.json` while
  the run was in flight. That is the browser-catch worker's area,
  not this milestone's — no sync code or test touches the
  manifest, and the sync suite passed 23/23 in that same run.

## Known limitations, not bugs (round-12 additions)

- A sync run waits up to 120 s per re-downloaded file
  (`wait_for_download`); a slower file reports `failed` with "did
  not finish in time" and is retried at the next sync (unchanged
  from Round 7).
- A file the server describes with no size and no validator at all
  (no Content-Length, ETag or Last-Modified) is reported `skipped`
  with "cannot tell whether this file changed" — sync refuses to
  guess, in either direction (unchanged from Round 7; the
  size-only tests cover the case where Content-Length IS present
  but no validator is).
- Mid-download sync is SKIP by decision (see build-log.md): a
  downloading file is never probed, paused, reset or rewritten by
  a sync, and is re-checked at the next sync.
- Real-website sync is forbidden as a test (see test-plan.md):
  sync has never run against a real website, by rule.

## To check (round-12 additions, not reproduced, needs the desktop / Windows)

- F04-M4: the Scheduler dialog's periodic radio, interval field,
  last-sync status and "Sync now" button in the real desktop app
  — blocked on the binary, as with every milestone.
- The Windows handshake for the `sync_queue_now` command (same
  unproven status as every earlier milestone).

## Round 13 — Finish Options dialog (S09) — PARTIAL 3 of 4 — 2026-11-15

Build: the three missing Options tabs (Site logins, Dial-Up,
Sounds) on the 6 existing tabs. Engine: 14 new `options_tests`
+ 4 unit tests in `options.rs` (28 unit total); full suite 297
tests — the parallel sync worker's full run passed 297/297, 0
failed, and my per-binary re-runs totalled 296/297 (see below).
`cargo check --tests` warning-free. `npm run build` passes.
**No new bug was found in this round: no BUG-022.** BUG-001–021
re-checked: all regression tests pass, no regression. No open
bugs remain in this file.

Two out-of-scope observations from this round, neither a product
bug in this milestone, both already recorded by the parallel sync
worker in Round 12 above:

- The per-binary re-runs failed one extension test,
  `extension_manifest_is_mv3_and_minimal` ("must not request
  cookies"): the parallel browser-catch worker had added a
  `"cookies"` permission to `app/extension/manifest.json` while
  runs were in flight. That is the browser-catch worker's area
  — no Options code or test touches the manifest — so it gets
  no BUG entry here.
- A sync-suite run under parallel load failed
  `sync_reports_failed_probe_and_retries_at_the_next_sync`
  (BUG-020 regression test) with its finished file suddenly
  missing from disk; it passed in isolation and on the full
  re-run (23/23). Consistent with a concurrent run's
  `/tmp/fetchline-test-*` cleanup deleting a live test
  directory while four workers shared the VM — not a product
  fault, and not this milestone's code (this milestone does not
  touch the sync re-download path except through
  `resume_download`'s probe, which sends no credential when
  none is saved). No code was changed for it.

## Known limitations, not bugs (round-13 additions)

- Site-login passwords on this VM are base64-obfuscated only
  (`b64:` prefix) — NOT encryption — exactly like proxy
  passwords; DPAPI has never run on Windows here.
- Windows RAS execution (auto-dial, redial, hang-up via
  `rasdial`) is unproven: there is no dial-up connection on
  this VM, and auto-dial/redial are stored only, never executed
  here.
- Sound playback was never heard on this VM (no audio device);
  the Windows `SoundPlayer` branch has never run, and
  `play_sound` / `test_sound_file` honestly error "only
  available on Windows" here after validating the file.
- At verification time the periodic-sync re-probe and the
  page-scan probes (`fetch_page_links` / `detect_page_videos`)
  did not yet send site credentials — a gated link listed there
  showed its 401 error; the download itself (add/download
  path) did send the credential. Parallel workers have since
  reworked those probe paths in the same window (the sync
  re-probe now calls the credential-aware `probe_url`); that
  rework was not re-verified here.
- The 6 new Tauri commands are uncompiled on this Linux VM
  (`gdk-3.0`), as with every earlier milestone; the dialog is
  verified via `npm run build` + source inspection only.

## To check (round-13 additions, not reproduced, needs the desktop / Windows)

- S09-M6/M7/M8: the three new tabs in the running desktop app
  (add/edit/delete a login, masked password + Show toggle,
  dial-up fields, sounds enable + WAV path + Test) — blocked on
  the binary, as with every milestone.
- A real login-gated site end to end on Windows (DPAPI round
  trip + Basic auth against a real server, not the local
  fixture).
- A real dial-up connection: auto-dial on download start,
  redial after a drop, hang-up after a queue finishes
  (`rasdial <name> /disconnect`).
- A real WAV playing on Windows when each of the three events
  fires.

## Round 14 — Finish browser catch (S13 / F01) — PARTIAL 1 of 4 — 2026-11-15

Build: cookie forwarding for login-only links end to end
(extension `cookies` permission + `cookiesFor(url)`; host
validates, stores on the pending row, wipes on drain decision;
engine `probe_with_cookies` + per-download in-memory header,
dropped on restart), the download-watcher fix, and the Firefox
host manifest. Engine: 7 new extension tests (32 extension
total); full suite 326 tests, 0 failed, 2 consecutive full runs
(35 unit + 111 integration + 32 extension + 16 final-verify +
28 grabber + 14 verifier + 14 progress + 7 + 15 sync + 23 video
+ 31 proxy). `cargo check --tests` passes. `npm run build`
passes. Host smoke, rebuilt fresh: download-all 14/14, video
13/13, cookie 14/14 (new `cookie-smoke.py` C1–C7). Extension
JS tests 44/44 (video-detect 17, popup 6, background 4, watcher
9 new, speed-graph 8). **Two new bugs found and fixed in this
round: BUG-023 and BUG-024.** (Numbering note: BUG-022 was
taken by the parallel sync worker while this round was in
flight — the NTLM stub-proxy bug, Round 12 above — so this
round's bugs are BUG-023/024, the next free numbers.)
BUG-001–022 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

### BUG-023: README told Firefox to use the Chrome host manifest (S4, docs/install)

- Severity: S4 (docs/install — a Firefox user following the
  README would install a manifest Firefox rejects).
- Repro: `app/extension/README.md` §2 said Firefox takes the
  same host manifest in a different location. Verified against
  Chrome's native-messaging docs and Firefox/MDN: Firefox
  REJECTS `allowed_origins` — its key is `allowed_extensions`
  holding the add-on id. The keys are not interchangeable; two
  files are needed.
- Fix: new `app/extension/com.fetchline.host.firefox.json`
  (`allowed_extensions: ["fetchline@fetchline.app"]`);
  `manifest.json` gained
  `browser_specific_settings.gecko.id:
  "fetchline@fetchline.app"`; new validator
  `app/extension/tests/validate_host_manifest.py` checks both
  manifests (Chrome `allowed_origins` format + placeholder
  warning; Firefox `allowed_extensions` equals the gecko id,
  no `allowed_origins`); README install + checklist sections
  corrected.
- Kept test: `npm run test:extension` runs the new validator —
  it failed before the fix (missing Firefox file) and passes
  after.

### BUG-024: download watcher never fired — it fed a bare filename to the URL parser (S2)

- Severity: S2 (the watcher's cancel-before-handoff path was
  dead: every download that already had a filename was
  skipped, so a browser download could finish in the browser
  instead of being handed to Fetchline).
- Repro: `background.js` called
  `extensionOf(item.filename || item.url)`, but `item.filename`
  is a bare/local path, not a URL, so `new URL("file.zip")`
  throws, `extensionOf` returns `""`, and the watcher skipped
  the download. Found by the new watcher test: 4 of its 9
  cases failed before the fix.
- Fix: new `extensionOfFilename()` (strips directories, takes
  the suffix) with URL fallback; the watcher now uses
  `extensionOfFilename(item.filename) ||
  extensionOf(item.url)`.
- Kept test: `extension/tests/watcher.test.js` (9 cases:
  cancel+handoff of `.zip`; cookies attached; no cookies when
  none; full local-path filename — the BUG-024 regression;
  excluded site untouched; integration off untouched; `.txt`
  outside the catch list untouched; duplicate URL handed over
  twice; non-http(s)/finished downloads ignored) — 9/9 pass.

## Known limitations, not bugs (round-14 additions)

- Cookie forwarding is memory-only on the desktop side and
  host-scoped: the header is kept in
  `DownloadManager.cookies` for the run, removed on
  complete/error/cancel, kept on pause (pause/resume
  in-session works), dropped on restart — a resumed download
  then fails with a clear 401 (tested). Pending rows are
  wiped of cookies when the drain decides them. Cookies take
  precedence over a stored site login in probe (live session
  fresher); FTP cannot carry cookies (plain probe).
  `probe_with_cookies` is the plain HTTP(S) path — it does NOT
  go through proxy/NTLM machinery, so a cookie-gated link
  behind an NTLM proxy reports the proxy's 407 as a clear
  error, and the raw NTLM segment retry does not carry
  cookies (same documented limit).
- `download_all` page expansion carries no cookies (the host
  fetches the page itself); a login-gated page's links must be
  right-clicked individually.
- Pause/resume WITH cookies was not given an automated test
  (timing-sensitive); it is code-reviewed only — the header
  stays in the map across a pause and the segment GETs read
  it on every attempt.
- The Windows handshake checklist in `app/extension/README.md`
  is annotated per item as verified-by-inspection vs
  needs-Windows; nothing in it has run on Windows.
- The Tauri `drain_pending_links` command is still uncompiled
  on this Linux VM (`gdk-3.0`), as with every earlier
  milestone; the drain is verified through the engine
  `drain_pending_links` + the real host binary only.

## To check (round-14 additions, not reproduced, needs the desktop / Windows)

- The Windows handshake on Windows: registry install of the
  host manifest, the real extension id in `allowed_origins`,
  first live handoff against the real app-data folder
  (`%APPDATA%\com.fetchline.app\fetchline\app.db`).
- The extension running in a real browser: context-menu
  hand-off, download-watcher cancel-before-handoff, popup
  status — all Node fake-`chrome` tested here, never executed
  in a real browser on this VM.
- A real login-gated site end to end: the extension reading
  that site's real cookies and the gated download completing
  (here: local fixture server + test-only dummy cookie only).

## Round 15 — Final verification (all four partials) — 2026-11-15

Independent verifier pass after the four parallel finish
rounds (browser catch, periodic sync, Options dialog, proxy).
Full engine suite re-run twice after the fix below: **330
total, 0 failed** (35 unit + 111 integration + 32 extension +
4 final-verify2 + 16 final-verify + 28 grabber + 14 verifier +
14 options + 7 progress + 15 proxy + 23 sync + 31 video; was
326 — +4 new final-verify2 tests). `npm run build` passes;
extension JS 44/44 (36 extension tests + 8 speed-graph);
`node --check` passes on all four extension scripts; host
smoke against a freshly rebuilt `fetchline-host` binary:
video 13/13, download-all 14/14, cookie 14/14. One new bug
was found by the verifier's own adversarial tests, fixed
failing-test-first (BUG-025 below). BUG-001–024 re-checked:
all regression tests pass by name in the output (37 named
checks, incl. BUG-002/003/005/006/008/011/012/017/018/019/020
and the Last-Modified / size-only / interval / cookie /
NTLM / Kerberos cases); BUG-007 re-checked by source
(no `hidden` / `group-hover:block` left on the sidebar
buttons); BUG-013/016 covered by the host smoke / JS tests.
No open bugs remain in this file.

Two test-only cleanups in this round, neither a product bug,
so neither gets a BUG entry:

- `cargo check --tests` reported 3 warnings in
  `tests/proxy_tests.rs` (unused `Read` import, unnecessary
  `mut` on the Type 2 AV-pair closure, an `authed = true`
  assignment the harness never reads because no client reuses
  an authenticated stub connection) — the builders' claim of
  "warning-free" was stale. All three fixed in the test file
  (the `authed` flag stays, with an `#[allow(...)]` and a
  comment, because a real proxy keeps connections alive);
  `cargo check --tests` is now 0 warnings.
- The manifest test's comment cited "(BUG-022)" for the
  Firefox gecko id; the gecko id was BUG-023 (BUG-022 is the
  NTLM stub). Comment corrected, no behaviour change.

### BUG-025: Page-scan probes never sent a saved site login, so a login-gated link showed 401 in Download All / the video panel

- Severity: S3
- Flow / case: F07 / F05 / S09-L (page scan with a saved login)
- Screen: S07 (Download All dialog), S08 (VideoGrabberDialog)
- Build: 2026-11-15 (four partials finished)

Steps
1. Save a site login for a host (Options > Site logins).
2. Open Download All (or the video grabber) on a page from
   that host whose links need the login.
3. Look at the listed links.

Expected: each gated link probes OK with its real size and is
selectable/downloadable — the same credential the manual add
path already sends.
Actual: the page fetch itself failed with 401 when the page
was gated, and on an open page every gated link was listed
with a "401 Unauthorized" error and excluded — even though
the user had saved the credential and adding the same URL
manually (S02) downloaded it fine.
Evidence: new verifier tests in
`app/engine/tests/final_verify2_tests.rs` (own gated page +
file + playlist fixture server on 127.0.0.1, test-only dummy
credential "test-user" / "test-pass-123"):
`verify2_page_scan_sends_saved_site_login`,
`verify2_page_scan_login_not_sent_to_another_host` and
`verify2_hls_variant_scan_sends_saved_site_login` all failed
before the fix — the first with `Status(401, "401
Unauthorized while fetching the page")` even with the login
saved. The negative control
(`verify2_page_scan_without_login_fails_gated_page`) passed
before and after, proving the fixture really gates.
Suspected cause: `fetch_page_links` fetched the page with a
bare `self.client().get(page_url)` and probed every link with
`probe_with_proxy(..., None, ...)` — auth `None`, always.
The same gap was in `fetch_playlist_text` (no auth on the
playlist GET) and in the HLS variant probes in
`expand_hls_playlist` (auth `None`). The Options worker had
flagged exactly this as "reworked but not re-verified": the
sync re-probe DID move to the credential-aware `probe_url`
(verified in this pass, manager.rs sync path), but the
page-scan paths never did.
Status: FIXED. All four call sites now thread the saved
credential for the exact host (`site_auth_for_url`, the same
exact-host lookup the add/download path uses): the page GET,
each link probe (looked up per URL inside the parallel probe),
the playlist-text GET, and each HLS variant probe. A login
for another host is still never sent (pinned by the
not-sent-to-another-host test). Regression tests kept: all 4
in `final_verify2_tests.rs` (4/4 pass; one wrong test
expectation corrected during the fix — the HLS variant
quality is the honest dimension label "1280 × 720", not
"720p" — a test bug, not a product bug). Verified: full
suite 330/330, 0 failed, 2 consecutive runs. Files:
`app/engine/src/manager.rs` (`fetch_page_links`,
`fetch_playlist_text`, `expand_hls_playlist`).
Note: the site grabber crawl still does NOT send site
logins — login-required pages are reported, as documented in
the build log. That is a separate, deliberate scope decision,
not part of this fix.

## Known limitations, not bugs (round-15 additions)

- The proxy row was reverted `yes` → `partial` by this pass:
  Kerberos (SPNEGO/Negotiate) proxy auth is unbuilt, and recon
  F09 names it in scope — see `replica/parity.md` and the row
  note in `features.csv`. The only must-have not done.
- The grabber crawl does not send site logins (see BUG-025
  note above); the page scan now does.
- `cargo check --tests` was NOT warning-free at the start of
  this pass (3 warnings in `proxy_tests.rs`); it is 0 warnings
  after the test-only fixes above.
- The `cookies` manifest permission is genuinely used:
  `background.js` `cookiesFor(url)` reads cookies only for the
  exact handed-over URL and attaches them to `download_link` /
  `video_found`; the manifest test
  (`extension_manifest_is_mv3_and_minimal`) passes with the
  permission listed as allowed, and the Python validator lists
  it as allowed for exactly that use. No unused-permission
  finding.

## To check (round-15 additions, not reproduced, needs the desktop / Windows)

- Kerberos proxy auth on Windows (SSPI) — the remaining
  must-have gap; needs a Windows build to implement and prove.
- F01/F05 in a real browser and the Windows handshake (unchanged
  from every earlier round).
- The BUG-025 fix in the running desktop app: Download All /
  video panel listing a login-gated link with its size — the
  engine path is tested, the dialogs are verified via
  `npm run build` + source inspection only, as with every
  milestone.

## Round 16 — Kerberos proxy auth (SPNEGO / Negotiate) — 2026-11-15

First of the three sequential feature workers (Kerberos,
then update check, then download quota). Full engine suite
**354/354 pass, 0 failed** (44 unit + 112 integration + 32
extension + 4 final-verify2 + 16 final-verify + 28 grabber
+ 14 verifier + 14 options + 14 kerberos + 7 progress + 15
proxy + 23 sync + 31 video; was 330 — +9 unit, +1
integration, +14 kerberos). `npm run build` passes;
`cargo check --tests` 0 warnings. `/tmp/fetchline-test-*`
cleaned before and after. BUG-009/010 regression tests
pass. **No new bugs found in this pass — no BUG-026.**
BUG-001–025 re-checked: all regression tests pass in the
full suite.

One test-only fix in this round, not a product bug, so no
BUG entry: the new `kerberos_proxy_ranged_get_and_head_work`
test asserted `Content-Range` contains `"1024"`; the real
value is `bytes 0-1023/524288`. The assertion was wrong,
the engine reply was right — fixed in the test.

## Known limitations, not bugs (round-16 additions)

- No real KDC was contacted: the Kerberos success path is
  proven by a local stub proxy with its own hand-written
  DER code plus a scripted token source, not a domain login
  (see `replica/test-plan.md` S09-P34).
- The SSPI (Windows) Kerberos path is written, never
  compiled; Kerberos success through the manager's
  probe+segment paths is unproven without a KDC.
- https:// through a Kerberos proxy is refused (no TLS
  tunnel after the handshake); SOCKS5 GSSAPI is not built.
- The round-15 "to check" item "Kerberos proxy auth on
  Windows (SSPI) — the remaining must-have gap" is now the
  only part of that item still open: the auth itself is
  built, the Windows execution remains unproven.

## To check (round-16 additions, not reproduced, needs the desktop / Windows)

- Kerberos proxy auth on Windows (SSPI) against a real
  domain proxy — needs a Windows build and a KDC.
- The new Authentication select in the Proxy tab of the
  Settings dialog in the running desktop app (engine path
  is tested; the dialog is verified via `npm run build` +
  source inspection only, as with every milestone).

## Round 17 — Update check (S09, should-have) — 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic
sync plus S09 finish plus Kerberos, plus the update check.
Engine: 369 total (50 unit + 319 integration), 0 failed, 2
consecutive full runs. `npm run build` passes; `cargo check
--tests` 0 warnings. No new bug was found in this round — the
9 update tests + 6 update unit tests were written
failing-test-first and all passed once the implementation
landed, except one test-only fix (see below). BUG-001–025
re-checked: all regression tests pass, no regression. No open
bugs remain in this file.

One test-only fix in this round, not a product bug, so no BUG
entry: a manifest with no `version` field failed with a
generic JSON error ("missing field `version`") instead of the
explicit "no version" error the check is supposed to give.
`UpdateManifest.version` now has `#[serde(default)]`, so the
missing field reaches the explicit validation. The engine
behaviour was right (it errored); only the message was less
clear than intended — fixed in `update.rs`, test kept
(`malformed_manifest_is_a_clear_error_not_a_crash`).

## Known limitations, not bugs (round-17 additions)

- The Tauri updater plugin is not configured and there is no
  signed release pipeline: the check reports new versions, it
  does not install them. The dialog says so and offers the
  installer address to copy (see `replica/test-plan.md`
  UPD-T1).
- No real update server was ever contacted — all checks used
  a local fixture server only, and the default update URL is
  a placeholder that must be replaced with the real release
  server before shipping (see `replica/test-plan.md` UPD-T2).
- The new Tauri command is uncompiled on this Linux VM
  (`gdk-3.0`), as with every earlier milestone; the dialog
  and Help menu are verified via `npm run build` + source
  inspection only. No Windows `.exe` exists.

## To check (round-17 additions, not reproduced, needs the desktop / Windows)

- Help > Check for updates and the update dialog in the
  running desktop app (engine path is tested; the dialog is
  verified via `npm run build` + source inspection only).
- The update-server address field and last-checked time in
  the Settings dialog in the running desktop app.
- The in-app install once the updater plugin and signed
  releases exist (Windows).

## Round 18 — Download quota + per-download cap (S12) — 2026-11-15

Build: all earlier slices plus S12 quota + per-download cap.
Engine: 52 unit + 112 integration + 32 extension + 4
final-verify2 + 16 final-verify + 28 grabber + 14 verifier +
14 options + 14 kerberos + 9 update + 7 progress + 15 proxy +
19 quota + 23 sync + 31 video = 388, 0 failed. `npm run build`
passes; `cargo check --tests` warning-free. No new bug was
found in this round: the 19 quota tests were written
failing-test-first and all passed once the implementation
landed (the first run's 6 failures were test bugs, not product
bugs — see the build log: a hand-built migration database
missing a column a real pre-S12 database has, pause/resume
tests sized so the remainder could not fit the fresh window,
and a wrong rollover expectation in the user-pause test).
BUG-001–025 re-checked: all regression tests pass, no
regression. No open bugs remain in this file.

## Known limitations, not bugs (round-18 additions)

- A quota enabled mid-download applies to the next download,
  not the running one (like `max_connections`): the running
  download's bytes were never counted, so counting them from
  mid-run would under-count the window. Documented in
  `engine/src/quota.rs` and the build log.
- The quota window follows wall-clock Unix time: a user
  changing the system clock moves the window, as with the
  scheduler (see `replica/test-plan.md` S12-T1).
- The 4 new Tauri commands are uncompiled on this Linux VM
  (`gdk-3.0`), as with every earlier milestone; the quota UI
  is verified via `npm run build` + source inspection only.
  No Windows `.exe` exists.

## To check (round-18 additions, not reproduced, needs the desktop / Windows)

- Options > Connection quota section (enable, MB, hours, usage
  + window end, Reset button) and the status-bar quota state
  in the running desktop app.
- The S02 per-download cap field and the S03 cap display +
  quota-paused note in the running desktop app.

## Round 19 — Antivirus hook (S14) — 2026-11-15

Build: slices 1–5 plus S13 plus S08 plus S06 plus S05 periodic sync
plus Kerberos / update check / quota plus S14. One new bug was
found by the full-suite run after the S14 build, fixed
failing-test-first (BUG-026 below). BUG-001–025 re-checked: all
regression tests pass, no regression. No open bugs remain in this
file.

### BUG-026: `list_queue_files` missed the S14 scan columns, so every queue read failed

- Severity: S2
- Flow / case: F04 / F04-H1 (queue drain), S05 scheduler
- Screen: S01/S05 (engine: `Store::list_queue_files`)
- Build: 2026-11-15 (S14)

Steps
1. Build S14 (scan columns `scan_status` / `scan_detail` /
   `scan_scanned_at` added to `downloads`).
2. Add any download to a queue and list the queue's files (or
   start the queue, or run a scheduler tick).

Expected: the queue lists its files; the drain runs.
Actual: every queue read fails with
`Db(InvalidColumnIndex(20))` — queue drains, reorders, stops
and scheduler ticks all error out.
Evidence: full-suite run after the S14 build — 20 failures in
`engine_tests`, all queue/scheduler tests
(`add_download_with_queue_files_it_in_main_queue`,
`queue_drains_in_order_and_clears_queue_id`,
`scheduler_starts_queue_at_start_time`, …), each panicking
with `Db(InvalidColumnIndex(20))` at the `list_queue_files`
call. The single-test reproduction
`add_download_with_queue_files_it_in_main_queue` fails alone
with the same error.
Suspected cause: S14 added three columns to `row_to_download`
(indices 20–22) and updated the `get_download` and
`list_downloads` SELECTs, but missed the third `row_to_download`
caller, `list_queue_files`, which still selected the old 20
columns.
Status: FIXED. `list_queue_files` now selects
`scan_status, scan_detail, scan_scanned_at` like the other two
queries. Regression test kept:
`queue_files_read_back_with_scan_columns` in
`app/engine/tests/antivirus_tests.rs` (a queued file reads back
with `scan_status: None`). Verified: full suite re-run green —
410/410, 0 failed (see Round 19 summary in `replica/test-plan.md`). File:
`app/engine/src/store.rs` (`list_queue_files`).

## Known limitations, not bugs (round-19 additions)

- No real antivirus product was ever run: every scan in the
  tests used a fake scanner script fixture (exit codes and
  output simulated). Windows Defender / real AV behaviour on
  Windows remains unproven.
- The 5 new Tauri commands are uncompiled on this Linux VM
  (`gdk-3.0`), as with every earlier milestone; the Settings
  virus-check section and the S14 completion dialog are verified
  via `npm run build` + source inspection only. No Windows
  `.exe` exists.
- A scanner that spawns long-lived children keeps the pipes
  open after a timeout kill; the drain threads are left to
  finish on their own rather than joined (see `antivirus.rs`)
  — otherwise the timeout itself would hang until the child
  exits.
- Opening a file (`open_file`) spawns the system opener
  (`cmd /C start` on Windows, `xdg-open` on Linux): on this VM
  the spawn succeeds but there is no handler, so the file is
  not actually displayed — the refusal path (threat / missing
  file) is what the tests prove.

## To check (round-19 additions, not reproduced, needs the desktop / Windows)

- Options > Downloads virus-check section (enable, command,
  timeout, Test button) and the S14 completion dialog (file
  path, Open / Open folder, scan result, Quarantine / Delete
  on a threat) in the running desktop app.
- A real scanner (e.g. Windows Defender's MpCmdRun) against a
  real finished download on Windows: exit codes, output text
  and timing of a genuine scan.

## Round 20 — First Windows build failure (CI run 43674541575) — 2026-11-15

### BUG-027: Unix-only `FileExt::write_at` breaks the Windows build — 5 compile errors in `fetchline-engine`

- Severity: S2
- Flow / case: F01 (segmented download write path), CI Windows build
- Screen: S01/S03 (engine: `Manager::run_segment` and friends)
- Build: 2026-11-15 (first GitHub Actions Windows build, run 43674541575, job 132691688464)

Steps
1. Push the repo to GitHub; the `Build Windows installer` workflow
   runs on `windows-latest`.
2. The frontend builds fine ("✓ built in 3.87s").
3. Step 9 (`npx --yes @tauri-apps/cli@2 build`, working dir `app`)
   fails compiling `fetchline-engine`.

Expected: the engine compiles on Windows and the MSI + NSIS
installer is produced.
Actual: 5 errors, all in `app/engine/src/manager.rs`:
`error[E0433]: cannot find 'unix' in 'os'` at manager.rs:25
(`use std::os::unix::fs::FileExt;`, "the item is gated behind the
`unix` feature"), plus four `error[E0599]: no method named
'write_at' for 'MutexGuard<'_, File>'` at manager.rs:4461, 4525,
4621, 4696. Final line: "error: could not compile
`fetchline-engine` (lib) due to 5 previous errors".
Evidence: CI run 43674541575 step 9, exit code 1, 4m 22s; the
engine-tests step (step 8, `cargo test` in `app/engine`,
continue-on-error) also exited 1 on Windows — the same compile
failure, since `cargo test` compiles the same lib (it never got
to run any test).
Suspected cause: the segment writer used Unix-only positional
I/O (`std::os::unix::fs::FileExt::write_at`). The Tauri shell
never compiled on the Linux VM, so no Windows compile had ever
been attempted before this CI run — the Unix-only import was
invisible to every local check.
Status: FIXED. New cfg-gated helper `write_at_pos` in
`app/engine/src/manager.rs`: Unix keeps `FileExt::write_at`,
Windows uses `FileExt::seek_write` (same semantics — write `buf`
at absolute `offset`, returns bytes written); the top-level
Unix-only import is gone and all 4 call sites go through the
helper. Linux write semantics are unchanged (same offsets, same
partial-write handling — callers already assumed the full `take`
bytes, as before). A whole-workspace sweep found no other
Unix-only compile blocker: the only other `std::os::unix`
reference was this import; `open_path` (queue.rs),
`execute_action` (rasdial/shutdown/hibernate), `play_wav_file`
(PowerShell SoundPlayer) and the SSPI/DPAPI FFI are all
`#[cfg(target_os = "windows")]`-gated with honest errors off
Windows; `host` (`db_candidates`) checks `%APPDATA%` first and
`default_downloads_dir` checks `USERPROFILE` first. The
kerberos.rs:696 unused-assignment warning was also fixed
properly (the input `SecBuffer` is now built only when a token
exists — `Option<SecBuffer>` — no `#[allow]`).
Regression test kept: `manager::tests::write_at_pos_writes_out_of_order_byte_exact`
in `app/engine/src/manager.rs` (writes "world" at 6, "hello " at
0, "!" at 11, an overlapping rewrite at 5 and a zero-length write
at the end; reads back byte-exact `hello-world!`).
Verified: full suite 411/411 pass, 0 failed (was 410/410; +1 new
test); `cargo check --tests` 0 warnings; `npm run build` passes;
parity unchanged 85.4/100 (must-haves 17/17). Windows
cross-check attempted: `rustup target add x86_64-pc-windows-gnu`
succeeded, but `cargo check --target x86_64-pc-windows-gnu -p
fetchline-engine` is blocked on this VM by a missing MinGW C
compiler (`cc-rs: failed to find tool "x86_64-w64-mingw32-gcc"`;
`apt-get install -s gcc-mingw-w64-x86-64` cannot even locate the
package) — so instead every `cfg(windows)` block was audited by
hand (SSPI FFI signatures/arity vs Win32, DPAPI signatures,
`seek_write` signature, src-tauri has no cfg blocks). Windows
compilation is therefore NOT proven locally; the CI run after
upload is the proof. File: `app/engine/src/manager.rs`
(`write_at_pos`, 4 call sites, regression test),
`app/engine/src/kerberos.rs` (warning fix).

## Known limitations, not bugs (round-20 additions)

- The Windows GNU cross-check could not run on this VM (no
  MinGW gcc); Windows compilation of the fixed code is proven
  only by the CI run after the user uploads the fix.
- The engine-tests step also exited 1 on the failed CI run; the
  evidence points at the same compile failure (cargo test
  compiles the same lib), but Windows *runtime* test failures
  beyond compilation cannot be ruled out until a green CI run
  shows the step passing.

## Round 21 — Second Windows build failure (CI run 43676557208) — 2026-11-15

### BUG-028: `std::path::Result` typo in the Tauri shell breaks the Windows build — 1 compile error in `fetchline`

- Severity: S2
- Flow / case: app startup (`db_path` / `build_manager`), CI Windows build
- Screen: S01 (shell: `app/src-tauri/src/lib.rs`)
- Build: 2026-11-15 (GitHub Actions Windows build, run 43676557208, job 132697870091)

Steps
1. Push the repo to GitHub; the `Build Windows installer` workflow
   runs on `windows-latest`.
2. The frontend builds fine ("vite built in 4.02s") and the engine
   tests pass (step 8 green — BUG-027 is fixed).
3. Step 9 (`npx --yes @tauri-apps/cli@2 build`, working dir `app`)
   fails compiling the Tauri shell.

Expected: the Tauri shell compiles and the MSI + NSIS installer is
produced.
Actual: 1 error in `app/src-tauri/src/lib.rs:28`:
`error[E0425]: cannot find type 'Result' in module 'std::path'` —
`fn db_path(app: &AppHandle) -> std::path::Result<std::path::PathBuf>`.
There is no `Result` in `std::path`; `app_data_dir()` returns
`Result<PathBuf, tauri::Error>`. Packaging was never reached.
Evidence: CI run 43676557208 step 9 log tail, read from the user's
signed-in browser (exit code 1, 4m 21s); the log shows this single
Rust error — no other compile error, no packaging error.
Suspected cause: a typo written when the shell was first authored.
It survived because the Tauri lib never compiles on the Linux VM
(missing `gdk-3.0`; no apt fix attempted) — the shell had never
been compiled anywhere before CI run #4, and rustc only reports
the errors it reaches, so this one surfaced only after BUG-027
was fixed.
Status: FIXED. `db_path` now returns
`Result<std::path::PathBuf, tauri::Error>` — plain
`std::result::Result` with the `tauri::Error` error type, matching
what `app_data_dir()` returns. Behaviour is unchanged (same path,
same `.map_err(|e| e.to_string())?` in `build_manager`). The whole
Tauri shell (`lib.rs`, 779 lines, and `main.rs`) was audited: no
other wrong type, missing import or bad path — and the run-4 log
is the evidence, since rustc reports every error in the crate and
this was the only one. Two remaining Windows-only warnings from
the run-4 log were also cleaned, with no `#[allow]`: (a)
kerberos.rs `in_buf` still assigned-but-never-read — the BUG-027
restructure (`Option<SecBuffer>` initialised to `None`) still
warned, so the buffer is now built in a single assignment
(`input_token.map(...)`) and the descriptor is patched only when
the buffer exists; (b) `FreeContextBuffer` was declared in the
SSPI FFI block but never used — removed, because SSPI never
allocates a buffer for us (the output buffer is our own `Vec`),
so there is nothing for it to free; the module header comment was
corrected to say the same and to note the engine has now compiled
on the Windows CI runner.
Regression test kept: none for the one-line type fix itself (it
is a compile error — the CI build is the test). For the warning
fix, a standalone stub of the exact `step()` prologue was compiled
with `rustc -D warnings`: the old pattern fails with the same
`unused_assignments` error, the new pattern compiles clean and
runs correctly for `Some` and `None` tokens (`/tmp/bug028/`).
Verified: full suite 411/411 pass, 0 failed; `cargo check
--tests` 0 warnings; `npm run build` passes; parity unchanged
85.4/100 (must-haves 17/17). Windows cross-check attempted again
and still blocked on this VM (`cargo check --target
x86_64-pc-windows-gnu -p fetchline-engine` fails in `cc-rs`:
`failed to find tool "x86_64-w64-mingw32-gcc"` — no MinGW
compiler; the Tauri lib itself cannot be checked here at all
(`cargo check -p fetchline` fails on the missing `gdk-3.0`).
File: `app/src-tauri/src/lib.rs` (one-line signature fix),
`app/engine/src/kerberos.rs` (warning fixes + comment).

## Known limitations, not bugs (round-21 additions)

- The Tauri shell has still never compiled on this VM; the fix
  is proven only by the CI run after the push. If step 9 fails
  again, its log must come from the user's signed-in browser
  (the logs API returns 403 without sign-in).
- The kerberos.rs warning fixes are cfg(windows)-only, so they
  never compile on Linux; the restructure was verified with a
  standalone `rustc -D warnings` stub of the exact code pattern
  (old pattern reproduces the warning, new pattern is clean),
  and the real proof is a warning-free step 9 on the CI runner.

## Round 22 — First real Windows launch fails silently (CI run #5 installer) — 2026-11-15

### BUG-029: Installed app shows a blank white window for ~1-2 s, then closes — no error, no crash record

- Severity: S1
- Flow / case: app launch, first run on real Windows
- Screen: S01 (shell: `app/src-tauri/src/lib.rs`)
- Build: 2026-11-15 (CI run #5, 43678007493, commit f3736b84; MSI
  installed on the user's real Windows PC)

Steps
1. Install the MSI from the run-#5 artifact
   (`fetchline-windows-installer`) on a real Windows PC.
2. Launch Fetchline (Start menu or the installed exe).
3. A blank white window appears for ~1-2 s, then closes. No error
   dialog, no message of any kind.

Expected: the Fetchline window opens and stays open with the
download list.
Actual: the window closes by itself after ~1-2 s, every launch.
Evidence: `%APPDATA%\com.fetchline.app\fetchline\app.db` EXISTS
(156 KB, created 11/15/2026 6:59 AM, the first launch), so
`Store::open` succeeded and `setup` got past the database.
Event Viewer (Application log) shows no Application Error /
Windows Error Reporting entry for Fetchline at launch time —
only unrelated CAPI2 certificate errors (Event 4176). Code
already checked: no `process::exit`, no frontend `window.close`,
window config normal (1100x700, title Fetchline),
`scheduler_tick` does nothing on a fresh DB,
`DownloadManager::new` only panics if the reqwest fallback
client fails to build. The user sent a screen recording of the
launch (blank white window, then close).
Root cause: PROVEN by the user's `startup.log` (run #7
instrumented build, launched 07:47:21 EET, 2026-11-15) — the
log's last line is `PANIC at src\lib.rs:793:13: there is no
reactor running, must be called from the context of a Tokio
1.x runtime`. The `.setup` closure calls `tokio::spawn` six
times (lines 793/802/813/824/834/839) but setup runs
synchronously on the main thread OUTSIDE any Tokio runtime
context, so the very first `tokio::spawn` panics; the panic
unwinds out of setup and the process exits cleanly with code
101 — hence no WER / Application Error entry and the blank
flash (the webview never loads because the event loop never
starts). WebView2 is FINE: the log's second line is
`webview version: 157.0.4341.46` (recent) — the WebView2
theory is dead. The bug was latent since the first build:
the Tauri shell never ran anywhere before the user's PC, so
it was never caught; it is NOT a Windows-only quirk — it
would panic on any platform. Full log: `main entered`,
`webview version: 157.0.4341.46`, `setup start`,
`db_path ok: C:\Users\adham\AppData\Roaming\com.fetchline.app\fetchline\app.db`,
`store opened`, `manager built`, `settings applied`,
`PANIC at src\lib.rs:793:13: ...` — the log never reaches
"spawns done" or "setup ok".
Status: FIXED (Round 24). Fix: all six `tokio::spawn` calls
in the setup closure replaced with `tauri::async_runtime::spawn`
— Tauri's own runtime handle, the correct way to spawn
background tasks from setup (verified against vendored tauri
2.12.5: `pub fn spawn<F>(task: F) -> JoinHandle<F::Output>`
where `F: Future + Send + 'static`, `F::Output: Send +
'static`, `async_runtime.rs:300`). Each spawned body is
EXACTLY as before (same futures, same logic, no behaviour
change); the `tokio::time::interval` inside the tick task
stays as-is — it now runs inside a task on the runtime, so
it has reactor context. Audit of the rest of `lib.rs` and
`main.rs`: no other `tokio::spawn` / `tokio::task::spawn`
outside a runtime context — the only other `tokio` use in
the shell is that `interval` call. `tokio::spawn` inside
`async fn` Tauri commands is fine (commands run on Tauri's
async runtime) and was left alone. The startup.log
diagnostics stay in place (they proved their worth); after
the fix the log should reach "spawns done" and "setup ok",
plus "frontend reached backend: get_settings ok" once the
frontend mounts.
Fix (diagnostics only, no behaviour change): `app/src-tauri/src/lib.rs`
now writes an append-only `startup.log` next to the database
(`%APPDATA%\com.fetchline.app\fetchline\startup.log`) with
milestones — "main entered", "webview version: {v or ERROR}",
"setup start", "db_path ok: {path}", "store opened",
"manager built", "settings applied", "spawns done", "setup ok",
"run returned ok", and "frontend reached backend: get_settings
ok" (logged once, from the existing `get_settings` command the
frontend already calls on mount — no new command). A panic hook
installed at the very start of `run()` appends the panic payload
+ location to the same file (path resolved from the `APPDATA`
env var, temp-dir fallback; the hook itself cannot panic, and
also prints to stderr). A run error is now logged before the
same panic as before (`.expect` replaced with a match that logs
"run failed: {e}" first). All logging is best-effort std-only
(`OpenOptions` append + create, `create_dir_all`); a logging
failure never affects the app.
Regression test kept: the std-only helpers were verified with a
standalone `rustc -D warnings` stub (`/tmp/bug029/`): the exact
extracted functions compile clean, append lines with epoch
timestamps, log a caught test panic with payload + location,
and log nothing for a `None` path. The Tauri-side wiring
(`webview_version` re-export, `Builder::run -> Result<()>`,
setup closure `Result<(), Box<dyn Error>>`) was verified
against the vendored tauri 2.12.5 source.
Verified: full suite 411/411 pass, 0 failed; `cargo check
--tests` 0 warnings; `npm run build` passes; parity unchanged
85.4/100 (must-haves 17/17). Windows cross-check still blocked
on this VM (no MinGW gcc; Tauri lib uncheckable here) — the CI
run after the push is the proof. The next step is the user
installing the instrumented build, launching once, and sending
`%APPDATA%\com.fetchline.app\fetchline\startup.log`.
File: `app/src-tauri/src/lib.rs` (diagnostics only).

## Known limitations, not bugs (round-22 additions)

- (Resolved in Round 24 — see below.) The root cause of the
  launch failure was unknown when this round ran; it only
  instrumented the app. The `startup.log` from the user's
  next launch of the instrumented build proved the cause:
  `tokio::spawn` in the setup closure panics with no reactor
  running (BUG-029, fixed in Round 24).
- The user refuses command-line / PowerShell steps, so all
  further diagnosis must come from the app itself or from plain
  GUI steps (install, launch, send a file).
- The diagnostics code lives in the Tauri shell, which never
  compiles on this VM; its only proof is the CI run after the
  push.

## Round 23 — Diagnostics build fails CI run #6 (two E0308 type errors) — 2026-11-15

### BUG-030: Diagnostics rewrite broke the Windows build with two E0308 type errors in `app/src-tauri/src/lib.rs`

- Severity: S2
- Flow / case: CI build (GitHub Actions, windows-latest), step 9
  "Build Windows installer (MSI + NSIS)"
- Screen: S01 (shell: `app/src-tauri/src/lib.rs`)
- Build: 2026-11-15 (CI run #6, 43680534913, commit f51e6199 —
  the BUG-029 diagnostics round)

Steps
1. Push the BUG-029 diagnostics commit (f51e6199).
2. CI run #6: steps 1–8 success (including step 8 engine tests);
   step 9 fails; step 10 skipped; no artifact.

Expected: the diagnostics build compiles and produces the
instrumented installer.
Actual: step 9 fails with two Rust E0308 errors, both in the
diagnostics rewrite of `lib.rs` (frontend Vite build succeeded
first, "✓ built in 3.09s"):
1. lib.rs:117 — `Ok(DownloadManager::new(store, 8))`: expected
   `Arc<Store>`, found `Store`. `DownloadManager::new` is
   `pub fn new(store: Arc<Store>, max_connections: usize) ->
   Arc<Self>` (engine/src/manager.rs:335); the pre-diagnostics
   code wrapped the store in `Arc::new(...)` and the rewrite
   dropped it.
2. lib.rs:782 — inside the `.setup(|app| { ... })` closure,
   `return Err(e);`: expected `Box<dyn Error>`, found `String`.
   `build_manager` returns `Result<Arc<DownloadManager>,
   String>`; the setup closure returns `Result<(), Box<dyn
   Error>>`. The old code propagated with `?` (auto-converts
   String into Box<dyn Error>); the explicit `return Err(e)`
   does not convert.
Evidence: step-9 log tail read from the user's signed-in
browser (lines 25-75), two E0308 errors only — no other
compile error in the crate, so every other diagnostics symbol
(`tauri::webview_version()`, panic hook, log helpers, `run()`
match) resolved and type-checked.
Suspected cause: rewrite dropped the `Arc::new` wrapper and
replaced `?` with an explicit `return Err` without the
conversion.
Status: FIXED. Fix (two lines, `app/src-tauri/src/lib.rs`
only, no redesign): line 117 now
`Ok(DownloadManager::new(Arc::new(store), 8))` (`Arc` was
already imported); line 782 now `return Err(e.into());` —
the same `From<String> for Box<dyn Error>` conversion the
old `?` used, with the `log_startup(... "build_manager
failed: {e}")` line kept above it. Whole-file re-audit of the
diagnostics diff (milestones, panic hook, `webview_version`,
both log-path helpers, `log_startup`, `run()` match,
`get_settings` once-marker): no other Windows-only compile
risk found — the only other `return Err` sites in the file
already return `e.to_string()` into `Result<..., String>`,
and `Store::open` returns a plain `Store` (so `Arc::new` is
the right wrapper). BUG-029 stays OPEN — this round only
repairs the diagnostics build; its root cause is still
unknown until the user runs the instrumented build and sends
`startup.log`.
Regression test kept: none added — the failure is a
compile-time type error in the Tauri shell, which never
compiles on this VM; the CI run after the push is the proof.
The two signatures were re-verified by reading the code
(`manager.rs:335`, `store.rs:324`).
Verified: full suite 411/411 pass, 0 failed; `cargo check
--tests` 0 warnings; `npm run build` passes (✓ built in
3.88s); parity unchanged 85.4/100 (must-haves 17/17).
Windows cross-check still blocked on this VM (no MinGW gcc;
Tauri lib uncheckable here) — the CI run after the push is
the proof.
File: `app/src-tauri/src/lib.rs` (2 lines) + this log +
`replica/build-log.md`.

## Known limitations, not bugs (round-23 additions)

- The Tauri shell never compiles on this VM, so type errors
  like BUG-030 can only be caught by the CI run after the
  push — the Linux-side checks (engine suite, npm build)
  cannot see them. Both BUG-027/028/030 were the same class
  of failure: shell code that only Windows proves.
- BUG-029 is still open by design until the instrumented
  build is installed and its `startup.log` is read.

## Round 24 — BUG-029 root cause found in startup.log; fix: spawn on the Tauri async runtime — 2026-11-15

### BUG-029: RESOLVED — `tokio::spawn` in the setup closure panics: "there is no reactor running"

- Severity: S1
- Flow / case: app launch, first run on real Windows
- Screen: S01 (shell: `app/src-tauri/src/lib.rs`)
- Build: 2026-11-15 (CI run #7, 43682153394, commit bbd03779 —
  the instrumented diagnostics build)

Steps
1. Install the MSI from the run-#7 artifact
   (`fetchline-windows-installer`) on the user's real Windows
   PC, launch once, and send
   `%APPDATA%\com.fetchline.app\fetchline\startup.log`.
2. The log (epoch 1794721641, 07:47:21 EET) reads:
   `main entered`, `webview version: 157.0.4341.46`,
   `setup start`, `db_path ok: ...\app.db`, `store opened`,
   `manager built`, `settings applied`, then
   `PANIC at src\lib.rs:793:13: there is no reactor
   running, must be called from the context of a Tokio 1.x
   runtime`. It never reaches "spawns done" or "setup ok".

Expected: the Fetchline window opens and stays open.
Actual: blank white window for ~1-2 s, then the process
exits; no error dialog, no WER / Application Error entry.
Evidence: the startup.log above — the panic is at the very
first `tokio::spawn` in the setup closure (lib.rs:793). The
log also kills the WebView2 theory: version 157.0.4341.46
is recent and loaded fine.
Root cause: the `.setup` closure runs synchronously on the
main thread, outside any Tokio runtime context, but it calls
`tokio::spawn` six times (lines 793/802/813/824/834/839 —
progress, action and update forwarders, 30 s scheduler tick,
startup drain, startup queues). `tokio::spawn` requires a
reactor in the current context, so the first call panics;
the panic unwinds out of setup and the process exits
cleanly with code 101, before the event loop (and the
webview) ever starts. Latent since the first build — the
Tauri shell never ran anywhere before the user's PC. Not a
Windows-only quirk: it would panic on any platform.
Status: FIXED. Fix (six lines, `app/src-tauri/src/lib.rs`
only, spawn-site change only, no behaviour change): every
`tokio::spawn` in the setup closure is now
`tauri::async_runtime::spawn` — Tauri's own runtime handle,
the correct way to spawn background tasks from setup
(vendored tauri 2.12.5, `async_runtime.rs:300`:
`pub fn spawn<F>(task: F) -> JoinHandle<F::Output>`
where `F: Future + Send + 'static`, `F::Output: Send +
'static`). Each spawned body is byte-identical to before.
The `tokio::time::interval` inside the tick task is
unchanged — it now runs inside a task on the runtime.
Audit: no other `tokio::spawn` / `tokio::task::spawn`
outside a runtime context anywhere in `lib.rs` or
`main.rs` (main.rs is just `fetchline_lib::run()`); the
only other `tokio` use in the shell is that `interval`
call. `tokio::spawn` inside `async fn` Tauri commands is
fine (commands run on Tauri's async runtime) and was left
alone. The startup.log diagnostics stay in place — after
the fix the log should reach "spawns done" and "setup ok",
and "frontend reached backend: get_settings ok" once the
frontend mounts. BUG-030 stays FIXED.
Regression test kept: none added — the failure is a
runtime-context panic in the Tauri shell, which never runs
on this VM; the CI run after the push is the proof, and the
user's next `startup.log` is the launch proof.
Verified: full suite 411/411 pass, 0 failed (56 unit + 17
antivirus + 112 integration + 32 extension + 4
final-verify2 + 16 final-verify + 28 grabber + 14 verifier
+ 14 kerberos + 14 options + 7 progress + 15 proxy + 19
quota + 23 sync + 9 update + 31 video + 0 doc);
`cargo check --tests` 0 warnings; `npm run build` passes
(✓ built in 4.11s); parity unchanged 85.4/100 (must-haves
17/17). `tauri::async_runtime::spawn` signature verified
against vendored tauri 2.12.5 (`async_runtime.rs:300`).
Tauri lib still uncheckable on this VM — the CI run after
the push is the proof.
File: `app/src-tauri/src/lib.rs` (6 lines) + this log +
`replica/build-log.md`.

## Round 25 — BUG-031: engine test suite does not compile on Windows (Unix-only test fixtures) — 2026-11-15

### BUG-031: FIXED — `std::os::unix` in test fixtures breaks the Windows test build

- Severity: S2
- Flow / case: `cargo test` (full suite) on Windows
- Screen: engine tests (`app/engine/tests/`)
- Build: 2026-11-15 (found by the VPS test: `cargo test` on
  Windows Server 2025, repo head 7537cc9)

Steps
1. On Windows, run `cargo test` in `app/engine`.
2. The `engine_tests` target fails to compile:
   `error[E0433]: cannot find 'unix' in 'os'` at
   `tests/engine_tests.rs:490` (`use std::os::unix::fs::PermissionsExt;`)
   and `error[E0599]: no associated function or constant named
   'from_mode'` at line 491
   (`Permissions::from_mode(0o755)`). No test target runs at
   all in that invocation.

Expected: the full engine suite compiles and runs on Windows.
Actual: the suite never compiles on Windows. It was invisible
in CI because the workflow's "Engine tests" step has
`continue-on-error: true` — the step's failure never failed
the run, and the separate real-download step (Windows-clean)
passed, hiding it.

Root cause: three test fixtures used Unix-only APIs with no
platform gate — the run-program marker scripts in
`engine_tests.rs` (`queue_action_run_program_executes_and_dismiss_clears`)
and `grabber_tests.rs` (`marker_script`, used by
`grabber_completion_action_runs_when_files_downloaded`), and
the fake antivirus scanner in `antivirus_tests.rs`
(`fake_scanner`, used by all 17 antivirus tests). All three
wrote a `.sh` script and set +x with
`PermissionsExt::from_mode(0o755)`.

Fix: make each fixture platform-aware, test-only, no engine
change.
- `engine_tests.rs` / `grabber_tests.rs`: the marker script
  is now `on_done.bat` / `{name}.bat` on Windows
  (`@echo done > "<marker>"`, run directly by
  `Command::new` — CreateProcess handles .bat files) and
  the `.sh` + `#[cfg(unix)]` chmod on Unix. The Unix script
  is byte-identical to before.
- `antivirus_tests.rs`: `fake_scanner` writes a `.bat` on
  Windows (`.sh` name rewritten to `.bat`, `@echo off`
  header) by translating the body line by line
  (`translate_body_to_batch`, Windows-only): `exit N` →
  `exit /b N`; `sleep N` → `ping -n N+1 127.0.0.1 >nul`
  (`timeout` refuses redirected stdin — the engine runs the
  scanner with stdin nulled — while ping works);
  `echo ...` → same text with `$1` → `%1` and outer quotes
  dropped; the `printf '%s\n' "$1" >> <record>` recorder →
  `@echo %1>> "<record>"`. Any other shell line panics with
  a clear message instead of silently passing. The Unix
  path is unchanged (`#!/bin/sh` + `#[cfg(unix)]` chmod).
Regression tests kept: the three existing tests that use
these fixtures (they now run on both platforms instead of
only Unix).
Verified: `cargo check --tests` 0 warnings on Linux;
`cargo test --test engine_tests
queue_action_run_program_executes_and_dismiss_clears` 1/1
pass; `cargo test --test grabber_tests
grabber_completion_action_runs_when_files_downloaded` 1/1
pass; `cargo test --test antivirus_tests` 17/17 pass. The
batch translator was checked standalone against all six
body shapes used by the tests (exit, echo, sleep, threat
echo, found echo, printf recorder) — all six translate
exactly. Full Windows proof is the CI run after the push:
the `continue-on-error` on the "Engine tests" step must
also be removed (separate workflow change, with the
packaging fix) so a Windows compile break can never hide
again. A direct `cargo check --tests --target
x86_64-pc-windows-msvc` on this VM is blocked by the
`rusqlite` bundled build needing MSVC `lib.exe` (cc-rs
error, unrelated to this fix).
File: `app/engine/tests/engine_tests.rs`,
`app/engine/tests/grabber_tests.rs`,
`app/engine/tests/antivirus_tests.rs` + this log.

## Round 26 — BUG-032: `protect_round_trip` test fails on Windows whenever DPAPI works — 2026-11-15

### BUG-032: FIXED — secret-storage test asserted the Linux form on every platform

- Severity: S2
- Flow / case: `cargo test` (lib target) on Windows
- Screen: engine (`app/engine/src/secret.rs`)
- Build: 2026-11-15 (found by CI run 43708063352, head
  159dab06 — the user's manual workflow upload; the
  "Engine tests" step failed 55/56 with this one failure,
  now visible because `continue-on-error` was removed)

Steps
1. On Windows, run `cargo test` in `app/engine`.
2. The lib target fails one test:
   `secret::tests::protect_round_trip_off_windows` panics
   at `src/secret.rs:209` (`assert!(stored.starts_with("b64:"))`),
   the message being the stored value starting `dpapi:`.

Expected: the test passes on every platform.
Actual: it passes only when DPAPI happens to fail.

Root cause: the test was written for Linux — it pinned
the `b64:` obfuscated form. But `protect_secret` on
Windows returns `dpapi:...` whenever DPAPI succeeds, and
falls back to `b64:` only when DPAPI fails (locked
profile, service account). So on Windows the test is
environment-flaky: it passed in earlier CI runs because
DPAPI failed on the runner then, and failed in run
43708063352 because DPAPI worked. The failure is in the
test only — `protect_secret` / `unprotect_secret` behave
as designed on both paths.

Fix (test-only, no engine change): the test is renamed
`protect_round_trip` and now pins the round-trip plus the
two known prefixes, never which one —
`starts_with("b64:") || starts_with("dpapi:")`,
`assert_ne!(stored, "hunter2")`, round-trip equals
`"hunter2"`, legacy plain value reads back as-is. A
strict `dpapi:` assertion on Windows would have been
the same flakiness in reverse, so it was not used.
Regression test kept: the renamed test itself.
Verified: `cargo test --lib` 56/56 pass on Linux;
`cargo check --tests` 0 warnings. Windows proof is the
CI run after the push (Engine tests step now fails
visibly without `continue-on-error`).
File: `app/engine/src/secret.rs` + this log.

## Round 27 — BUG-033: shutdown test powers the machine off on Windows (and two more ungated tests) — 2026-11-15

### BUG-033: FIXED — `queue_action_shutdown_errors_off_windows` executes a real shutdown on Windows

- Severity: S1 (destructive on the machine it runs on)
- Flow / case: `cargo test` (full suite) on Windows
- Screen: engine tests (`app/engine/tests/engine_tests.rs:541`)
- Build: 2026-11-15 (found by the CI monitor: its
  `--no-fail-fast` run on the AWS VPS stopped the instance
  mid-suite; `describe-instances` reports the instance
  **stopped**, StateReason `Client.InstanceInitiatedShutdown`
  — a shutdown issued from inside Windows)

Steps
1. On Windows, run `cargo test` in `app/engine` with
   `--no-fail-fast` (or let the suite reach this test).
2. The test arms a `shutdown` completion action and calls
   `execute_queue_action`, expecting the Linux error
   "only available on Windows".
3. On Windows the action really runs `shutdown /s /t 0`
   (`app/engine/src/queue.rs`, Windows branch) — an
   immediate, undelayed shutdown of the machine. The
   GitHub runner is ephemeral so it just fails the step;
   the VPS stopped outright (public IP gone, SSM dead
   until the instance is started again).

Expected: the test passes on every platform.
Actual: on Windows it powers the machine off — the suite
can never go green there, and any machine running the
suite (VPS, runner, developer PC) shuts down.

Root cause: the same missing-gate pattern as BUG-031 /
BUG-032 — the test's name says "off_windows" but it has
no `#[cfg]` gate, and its assertion only holds where the
action refuses. The `shutdown` / `hibernate` actions are
Windows-only by design (`shutdown /s /t 0`, `shutdown /h`);
no test may execute them.

Fix (test-only, no engine change): the test is gated
`#[cfg(not(target_os = "windows"))]` with a comment
stating why it can never run on Windows. The same audit
found and fixed two more tests with the same flaw, which
would have failed the next CI run (harmless, no
shutdown): the dial-up hang-up test
(`options_tests.rs`, `dialup_hangup_arms_action_only_when_queue_has_no_action`)
expected the "only available on Windows" error, but on
Windows the action spawns `rasdial <name> /disconnect`
and reports Ok — now Ok on Windows, error elsewhere;
and the sound test
(`sound_disabled_event_records_nothing_and_playback_is_windows_only`)
expected playback to error "only available on Windows",
but on Windows the file really plays (PowerShell
SoundPlayer, spawn returns Ok) — now Ok on Windows,
error elsewhere. The audit also verified the other
`execute_queue_action` call sites are safe on Windows:
`run_program` (marker script), `open_file` (`cmd /C
start`), `none` (no pending action → error), and the
grabber shutdown case (dismissed before execute, so the
action never runs); `hibernate` is never executed by any
test.
Regression tests kept: all three fixed tests (they now
run correctly on both platforms, or only where safe).
Verified: on Linux the three tests pass 1/1 each (the
shutdown test still runs here — the gate preserves Linux
behaviour); `cargo check --tests` 0 warnings. Windows
proof is the CI run after the push — the engine-tests
step must now pass end to end, which also proves the
BUG-031 `.bat` fixtures and the BUG-032 fix. The VPS was
left **stopped** (the shutdown stopped it, which also
stops the hourly billing); restart with `aws ec2
start-instances` if SSM access is needed again.
File: `app/engine/tests/engine_tests.rs`,
`app/engine/tests/options_tests.rs` + this log.

## Round 28 — BUG-034: scanner command splitter strips Windows path backslashes — 2026-11-15

### BUG-034: FIXED — `split_command` mangled every unquoted Windows path

- Severity: S2 (real engine bug, not test-only)
- Flow / case: antivirus scan (S14) on Windows, any
  scanner command with an unquoted Windows path
- Screen: engine (`app/engine/src/antivirus.rs`,
  `split_command` + `build_command`)
- Build: 2026-11-15 (found by CI run 43710597692, head
  2d41441 — the Engine tests step failed in
  `antivirus_tests`: 8 passed, 9 failed; the log was
  read via the user's signed-in Chrome)

Steps
1. On Windows, run `cargo test` in `app/engine`.
2. The lib target passes 56/56. Then `antivirus_tests`
   fails 9 of 17 — every test that actually runs the
   fake scanner: `clean_scanner_reports_clean_and_scans_the_final_path`,
   `exit_zero_with_a_threat_in_the_output_is_still_infected`,
   `manual_rescan_after_completion_records_a_new_result`,
   `quarantine_is_refused_for_a_clean_file`,
   `run_scan_timeout_kills_a_sleeping_scanner_quickly`,
   `scan_does_not_block_completion_and_a_slow_scanner_times_out`,
   `scan_result_is_persisted_across_a_store_reopen`,
   `settings_round_trip_and_test_button_use_the_fake_scanner`,
   `threat_scanner_reports_infected_and_nothing_is_auto_deleted_or_opened`.
   All 9 carry the same detail: `Could not start the
   antivirus scanner: program not found. Check the
   command in Settings.` The 8 passing tests are the
   ones that never spawn a scanner (validation,
   not-configured, missing scanner, …).

Expected: the fake scanner runs and the tests see
`clean` / `infected` / `timed_out`.
Actual: the spawn fails before the scanner ever runs.

Root cause: `split_command` treated a backslash
OUTSIDE quotes as an escape for the next character and
dropped it. The tests set the scanner command
unquoted — `C:\Users\…\clean.bat {file}` — so every
backslash was stripped and the program token became
`C:Users…clean.bat`, with no path separator left.
Rust's Windows spawn then does a PATH search, finds
nothing, and fails with its own "program not found"
error. The existing unit test only covered a QUOTED
path (`"C:\Program Files\Scanner\scan.exe" …`),
where backslashes were already preserved, so the
unquoted case was never exercised. This is a real
product bug, not a test artifact: any user who typed
an unquoted Windows scanner path in Settings would
get the same "Could not start" failure (Linux paths
use `/`, so it never showed up there).

Fix (engine): the backslash arm now escapes the next
character only when that character needs escaping —
inside double quotes a quote or another backslash;
outside quotes a quote, another backslash, or a
space. Anything else keeps the backslash. The quoted
and single-quoted behaviour is unchanged. Regression
test kept: `split_command_handles_quotes_and_spaces`
now also asserts the exact failing shape —
`C:\Scanner\scan.exe {file}` splits to the intact
path plus `{file}`.
Verified: full engine suite **413/413 pass, 0 failed**
on Linux (lib 56, antivirus 17, engine_tests 112,
all other targets green), `cargo check --tests` 0
warnings. Windows proof is the CI run after the
push — the antivirus suite has never passed on
Windows before, so this run also proves the BUG-031
`.bat` fixtures end to end.
File: `app/engine/src/antivirus.rs` + this log.

## Round 29 — BUG-035: .bat `%1` keeps Rust's quotes, so the recorded path never matches — 2026-11-15

### BUG-035: FIXED — fake-scanner .bat records the quoted path

- Severity: S3 (test-only)
- Flow / case: `antivirus_tests` on Windows
- Screen: engine tests
  (`app/engine/tests/antivirus_tests.rs`,
  `translate_body_to_batch`)
- Build: 2026-11-15 (found by CI run 43711988153, head
  e16f8fb2 — the Engine tests step failed in
  `antivirus_tests`: 16 passed, 1 failed; the log was
  read via the user's signed-in Chrome)

Steps
1. On Windows, run `cargo test` in `app/engine`.
2. The lib target passes 56/56 and `antivirus_tests`
   passes 16/17. The one failure is
   `clean_scanner_reports_clean_and_scans_the_final_path`
   (line 166): the assertion compares the path the
   fake scanner recorded with the finished file's
   `save_path` — left `"C:\Users\RUNNER~1\…\clean.bin"`
   (quoted), right `C:\Users\RUNNER~1\…\clean.bin`
   (bare).

Expected: the recorded path equals the bare path.
Actual: the record holds the path in double quotes.

Root cause: when Rust invokes a .bat file it quotes
the argument, and in a batch file `%1` keeps those
quotes — only `%~1` strips them. The fake scanner
recorded `%1`, so on Windows the record always held
the quoted path while the test expected the bare one
(the Unix `$1` has no such quoting). The BUG-034
fix is what exposed this: before it, the scanner
never started at all (9 failures); with it, 16 of
17 pass and only this quoting detail remains. The
engine is correct here — a real .exe scanner gets
the bare path (the C runtime strips the quotes) —
so the fix belongs in the test fixture, not the
engine.

Fix (test-only, Windows-only code): the translator
now emits `%~1` instead of `%1` in both places that
carry the scanned path — the `printf` recorder line
(`@echo %~1>> "<record>"`) and the `echo` lines
(`$1` → `%~1`). `%~1` strips surrounding quotes
when present and is unchanged otherwise, so it
matches the Unix `$1` semantics exactly.
Regression test kept: the failing test itself (it
now asserts the bare path on both platforms).
Verified: the translator was checked standalone
with bare rustc (all 6 body shapes translate
exactly, `%~1` everywhere); `cargo test --test
antivirus_tests` 17/17 pass on Linux (unchanged
behaviour there — the changed code is
`#[cfg(windows)]` only); `cargo check --tests` 0
warnings. Windows proof is the CI run after the
push — if it goes green, the Engine tests step is
finally past and the host build, Tauri build, smoke
test and reworked capture job run for the first time
on this workflow.
File: `app/engine/tests/antivirus_tests.rs` + this log.

## Round 30 — BUG-036/037/038: the 42 Windows failures — accepted sockets inherit non-blocking mode, SSPI Negotiate falls back to NTLM, open-folder test deletes the open DB — 2026-11-15

### BUG-036: FIXED — every test stub's accepted socket is non-blocking on Windows, so multi-round-trip handlers fail instantly

- Severity: S2 (test-only; blocks the whole Windows suite)
- Flow / case: `cargo test --no-fail-fast` on Windows
  (VPS full-suite run, head 2697d0ec): 412 tests, 370
  passed, 42 failed, 10 of 18 binaries failed
- Screen: engine tests
  (`app/engine/tests/common/mock.rs`,
  `tests/proxy_tests.rs`, `tests/kerberos_tests.rs` and
  every other test file with a stub server)
- Build: 2026-11-15 (found by the VPS full-suite run
  2026-11-15 ~14:44 EET; isolated and proven by a
  raw-socket diagnostic the same day)

Steps
1. On Windows, run `cargo test --no-fail-fast` in
   `app/engine`.
2. 42 tests fail. The dominant family (~32) is WinSock
   10053 `WSAECONNABORTED` — Windows aborts connections
   to the hand-rolled TCP stub servers (FTP, SOCKS5,
   NTLM/Kerberos proxy stubs, sync tests) — while plain
   HTTP downloads through the same fixture server pass.
   One SOCKS5 test fails differently: the stub replies
   `REP=7 Command not supported`.

Expected: the stubs answer every round trip, on both
platforms.
Actual: on Windows the stubs close the connection
mid-handshake (FTP probe dies with 10053 in 0.09 s;
the SOCKS5 stub answers REP=7).

Root cause: every stub server sets its listener
non-blocking (`listener.set_nonblocking(true)`) so the
accept loop can poll the stop flag — and on Windows an
accepted socket INHERITS the listener's non-blocking
mode (on Linux accepted sockets are always blocking).
Every handler then does blocking-style reads
(`read_line`, `read_exact`, single `read`) on a
non-blocking socket: the read returns `WouldBlock`
the moment the peer's bytes have not arrived yet,
the handler treats any read error as "client gone"
and returns, dropping the socket while the client's
bytes are still unread in its receive buffer — so
the drop sends RST and the client sees 10053. The
pattern fits exactly: single-shot stubs (plain HTTP
mock, page server, grabber site) mostly work because
the client sends its whole request before the
handler's one read runs; multi-round-trip stubs
(FTP control connection, SOCKS5 handshake, NTLM /
Kerberos 407 loops) fail deterministically because
the server must read a second time, after its own
first reply, and that read races the client's
response. The SOCKS5 REP=7 is the same race: the
stub's `read_exact` for the CONNECT header hits
`WouldBlock` before the client's request arrives
and the stub misreads it as a bad command.
Proven, not guessed: a temporary raw-socket
diagnostic (`tests/raw_probe_tests.rs`, deleted
before pushing) ran on the VPS — the raw std SOCKS5
handshake PASSED (stub fine, engine client loses
the race under tokio scheduling), while the raw std
FTP handshake FAILED with 10053 right after the
greeting: the stub's first `read_line` returned
instantly instead of waiting for `USER`. On a
blocking socket that read would have waited.
Windows Defender (real-time + NIS on) was checked
and is NOT the cause: the failures are
deterministic in isolation, in 0.09 s, with no
load.

Fix (test-only): after every `accept()` from a
non-blocking listener, put the accepted stream back
in blocking mode first —
`stream.set_nonblocking(false).unwrap()` — 17 sites
in 10 files (5 in `tests/common/mock.rs`: MockServer,
PageServer, HttpProxyServer, Socks5Server,
GrabberSite; 2 in `tests/proxy_tests.rs`: FTP +
NTLM proxy; 2 in `tests/kerberos_tests.rs`; 1 each
in `extension_tests.rs`, `video_tests.rs`,
`grabber_verify_tests.rs`, `final_verify_tests.rs`,
`options_tests.rs`, `final_verify2_tests.rs` x2,
`update_tests.rs`). The listeners stay non-blocking
(the accept loops still poll); only the accepted
streams change, which makes Windows behave exactly
like Linux — the semantics every handler already
assumes. No engine code, no test assertions, no
timeouts changed.
Regression test kept: the 42 failing tests
themselves (they now exercise the same stubs on
both platforms).
Verified: `cargo check --tests` 0 warnings; full
engine suite on Linux 413/413 pass (lib 56,
antivirus 17, engine_tests 112, all other targets
green — the 112th engine test is the
`#[cfg(not(windows))]` shutdown test, so Windows
runs 412). Windows proof is the VPS full-suite
re-run after the push.
File: the 10 test files above + this log.

### BUG-037: FIXED — SSPI `Negotiate` package silently produces an NTLM token with no Kerberos credentials

- Severity: S2 (real engine behaviour)
- Flow / case: `kerberos_only_proxy_fails_clearly_with_real_gssapi_and_never_sends_basic`
- Screen: engine (`app/engine/src/kerberos.rs`,
  `SspiInitiator`)
- Build: 2026-11-15 (found by the VPS full-suite run,
  head 2697d0ec)

Steps
1. On Windows, run the Kerberos tests with the real
   SSPI initiator against a Negotiate-only proxy.
2. The test asserts no `Negotiate` token is sent when
   no Kerberos credentials exist — but a token IS sent.

Expected: with no Kerberos ticket, the initiator
fails cleanly and no token is ever sent (as on Linux,
where GSSAPI fails with no ticket cache).
Actual: SSPI produces a token and the engine sends
`Negotiate <token>` — the `Negotiate` package falls
back to NTLM inside the package when no Kerberos
ticket exists, so the "Kerberos" auth silently
downgrades to NTLM.

Root cause: `SspiInitiator::new` asked SSPI for the
`Negotiate` package. Negotiate is a meta-package: it
tries Kerberos, then NTLM, and returns whichever
works — with the SYSTEM / logon-session credentials
that always exist on Windows, it returns an NTLM
token. The setting is Kerberos and the fallback
order (Kerberos -> NTLM -> Basic) is handled at the
scheme level, so the silent in-package fallback is
wrong here: a Kerberos-only proxy must fail clearly,
and a Negotiate+NTLM proxy must fall back through
the explicit NTLM exchange, not inside Negotiate.

Fix (engine): ask SSPI for the `Kerberos` package
directly. With no ticket `InitializeSecurityContext`
fails and no token is sent; with a ticket it
produces a real Kerberos AP-REQ. Error strings and
module docs updated to name the Kerberos package;
the HTTP `Negotiate` scheme name is untouched.
Regression test kept: the failing test itself (it
now asserts the no-token property on both
platforms).
Verified: `cargo check --tests` 0 warnings; full
engine suite on Linux 413/413 pass (SSPI code is
`#[cfg(windows)]` — it never compiles here, as
before). Windows proof is the VPS full-suite re-run
after the push.
File: `app/engine/src/kerberos.rs` + this log.

### BUG-038: FIXED — `open_folder_validation` deletes the tmpdir while the SQLite DB inside it is still open

- Severity: S3 (test-only)
- Flow / case: `progress_tests::open_folder_validation`
  on Windows
- Screen: engine tests
  (`app/engine/tests/progress_tests.rs`)
- Build: 2026-11-15 (found by the VPS full-suite run,
  head 2697d0ec)

Steps
1. On Windows, run `open_folder_validation`: download
   a file into the tmpdir, wait for completion, then
   `std::fs::remove_dir_all(&dir)`.
2. The removal fails with Win32 error 32 ("The
   process cannot access the file because it is being
   used by another process").

Expected: the folder deletes cleanly, so the test
can then assert `folder_for_download` reports it
gone.
Actual: the removal fails — the SQLite database
(`app.db`) lives in the same tmpdir and the Store
holds it open for the whole test. Linux lets you
unlink open files, so the test passed there;
Windows refuses.

Root cause: the test used the tmpdir itself as the
download folder, so deleting "the download folder"
also deleted the open database. The downloaded
file's own handle is closed once the download
completes — only the DB was in the way.

Fix (test-only): the download folder is now a
`downloads` subdirectory of the tmpdir; the test
deletes that subfolder instead. Same assertions
(folder resolves, then "no longer exists" for both
entry points), same behaviour under test — only the
folder path moved out from under the open DB.
Regression test kept: the failing test itself.
Verified: `cargo check --tests` 0 warnings; full
engine suite on Linux 413/413 pass. Windows proof is
the VPS full-suite re-run after the push.
File: `app/engine/tests/progress_tests.rs` + this log.

## Round 32 — BUG-040: two grabber tests see downloads that do not belong to them — a stale-tmpdir cross-test leak — 2026-11-15

### BUG-040: FIXED — `tmpdir()` could return a directory left behind by an earlier test binary, so a test opened a stale `app.db`

- Severity: S3 (test-only; the Windows-runner failure)
- Flow / case: `grabber_tests::grabber_completion_action_not_armed_in_explore_only`
  (tests/grabber_tests.rs:675) and
  `grabber_tests::grabber_run_fails_when_its_queue_is_gone`
  (tests/grabber_tests.rs:580) on the GitHub runner
  (CI run 43722826150, head cd17d4f0): 262 passed, 2 failed
- Screen: engine tests
  (`app/engine/tests/grabber_tests.rs`, `tests/common/mock.rs`)
- Build: 2026-11-15 (found by CI run 43722826150; the same
  suite passed on the VPS at the same head, 412/412, and
  grabber 28/28 on the runner one run earlier at head
  2f02664a)

Steps
1. On the runner, run `cargo test` in `app/engine`.
2. Every binary passes except `grabber_tests`, where exactly
   two tests fail, both on the same assertion:
   `assert!(mgr.list_downloads().unwrap().is_empty())` —
   the list holds a download row although the test's own run
   added none (explore-only downloads nothing; the missing
   queue fails before crawling anything).

Expected: both tests see an empty download list.
Actual: on the runner the list is non-empty in both tests,
while every other assertion in both tests passes
(files_matched 8, files_downloaded 0, status failed with
"no longer exists", pages_explored 0).

Root cause: the rows are NOT the grabber's — no code path
in either test inserts a download (proven by audit: the
only `INSERT INTO downloads` is `Store::insert_download`,
reached only through `add_download_with_cookies`, which
probes first and inserts after; the explore path returns
before any add, the missing-queue path returns before any
crawl; `DownloadManager::new` spawns no background task;
each test has its own fixture site on a random port). The
rows can only come from ANOTHER test binary's database,
which means the test opened a database file it did not
create — i.e. its tmpdir collided with a directory left
behind by an earlier binary. The chain, each link proven
from the code:
- `tests/common/mock.rs::tmpdir()` built the directory as
  `fetchline-test-{pid}-{n}` with a per-process counter
  restarting at 0 in every binary. Windows reuses the PID
  of an exited process, so a later binary can compute the
  exact same path as an earlier binary's n-th directory.
- Stale directories DO happen on Windows: every test ends
  with `let _ = remove_dir_all(&dir)` while spawned tasks
  (grabber tasks, download tasks, the queue poll loop) may
  still hold the manager Arc and therefore the SQLite
  connection open — and Windows refuses to delete open
  files (error 32; the same mechanism as BUG-038), while
  Linux unlinks them happily. So earlier binaries leave
  `fetchline-test-*` directories behind, with populated
  `app.db` files.
- `create_dir_all` succeeds on an existing directory and
  `Store::open` opens the stale database without complaint,
  so the test silently inherits the earlier test's download
  rows. Project-level assertions still pass because the
  project and queue rows are freshly created in that same
  database — only the download list betrays the leak.
- This also explains why it never reproduces on Linux
  (no stale directories) and why it is intermittent on
  Windows (it needs a PID reuse landing on a stale
  directory whose downloads table is non-empty — the
  explore-only test at line 127, with the same assertion,
  passed in the very same run).

Fix (test-only): `tmpdir()` in `tests/common/mock.rs`
now builds `fetchline-test-{pid}-{n}-{nanos}` — the
nanosecond wall-clock timestamp makes the path unique
across processes even when the PID is reused (same
precedent as `update_tests.rs`' own tmpdir, which already
used PID + nanos); the counter still disambiguates two
calls in the same nanosecond inside one process. A stale
directory can now never be reopened by a later test. The
three `list_downloads().is_empty()` assertions in
`grabber_tests.rs` also print the offending rows on failure
now, so any future leak names itself instead of failing
bare. Nothing is weakened: the assertions still require an
empty list. Regression test kept: the two failing tests
themselves.
Verified: `cargo check --tests` 0 warnings; grabber_tests
5× 28/28; full engine suite 413/413 on Linux. VPS
verification (head 63bda459, confirmed by `rev-parse` on
the instance; --no-fail-fast): **412/412 PASSED, 0
failed**; both fixed tests ok in the log. Log:
~/workspace/vps-test-evidence/bug040-full-suite.log
(30,666 bytes) + S3
s3://fetchline-vps-evidence-ce1331eb/bug040-full-suite.log.
Mechanism checks on real Windows: the PID-reuse probe
spawned 300 short-lived processes and saw only 253
distinct PIDs — 47 PIDs reused — so PID reuse is real and
fast on Windows. Leftover `fetchline-test-*` directories
in TEMP after this run: 0 — the `remove_dir_all` race
does not lose every run, which is exactly why the CI
failure is intermittent (it needs a reuse landing on a
stale directory whose downloads table is non-empty).
Instance stopped after.


### BUG-039: FIXED — the test asserts the drain queue is empty the instant the status flips to "complete", before the runner removes the file

- Severity: S3 (test-only; the single remaining Windows failure)
- Flow / case: `sync_tests::sync_tick_does_not_fight_a_one_time_drain`
  on the GitHub runner (CI run 43717398945, head 2f02664a):
  371 passed, 1 failed
- Screen: engine tests
  (`app/engine/tests/sync_tests.rs`)
- Build: 2026-11-15 (found by CI run 43717398945; the same
  test passed on the VPS at the same head, 23/23 sync_tests)

Steps
1. On the runner, run `cargo test` in `app/engine`.
2. Every binary passes except `sync_tests`, where exactly
   one test fails:
   `sync_tick_does_not_fight_a_one_time_drain` panics at
   `tests\sync_tests.rs:469:5` — "the one-time drain still
   empties its queue", `assertion failed: queue.is_empty()`.

Expected: after the drain download reaches "complete", the
one-time queue is empty.
Actual: on the runner the queue still holds the file when the
assertion runs, even though the download is complete.

Root cause: "complete" and "removed from the queue" are two
separate steps done by two different tasks. The download
task flips the status with `mark_complete(id)`; the queue
membership is cleared only by the `run_queue` poll loop
(`src/manager.rs`, inner loop sleeps 200 ms per iteration),
which calls `remove_from_queue` after it observes
"complete". The test's `wait_for_status` polls every 100 ms
and returns the instant it sees "complete" — so on a loaded
2-vCPU runner the test's poller can win the race and assert
emptiness up to ~200 ms (plus scheduling delay) before the
runner's next tick removes the file. The scheduler tick is
exonerated: it skips queues that are already draining and
only spawns the sync for the periodic queue — it never
touches the drain queue. The first VPS run's 10053 symptom
on this same test was the BUG-036 stub issue, a different
mechanism already fixed.

Fix (test-only): the test now waits for the drain queue to
actually empty before asserting — new `wait_for_queue_empty`
helper (100 ms polls, 30 s deadline, same style as the
file's `wait_for_sync` / `wait_for_status`), then the
original assertion with its original message, unchanged.
This is the same pattern the codebase already uses for
exactly this situation (the finished-file drain wait in
`engine_tests.rs`). Nothing is weakened: the assertion
still requires an empty queue, and the test still fails if
the drain genuinely does not empty it within 30 s.
Regression test kept: the failing test itself.
Verified: `cargo check --tests` 0 warnings; sync_tests
run 5 times consecutively on Linux, all green; full engine
suite on Linux 413/413 pass. Windows proof is the VPS
full-suite re-run after the push.
File: `app/engine/tests/sync_tests.rs` + this log.

## Round 33 — BUG-041: update test fails `Store::open` with "database is locked" — no busy timeout, and `journal_mode=WAL` itself returns SQLITE_BUSY under a race — 2026-11-15

### BUG-041: FIXED — `Store::open` fails with SQLITE_BUSY when two connections open the same fresh database at once

- Severity: S3 (engine robustness + test-only; the Windows-runner failure)
- Flow / case: `update_tests::newer_version_available_shows_changelog_and_never_installs`
  on the GitHub runner (CI run 43729118128, head 63bda459):
  380 passed, 1 failed
- Screen: engine tests / store
  (`app/engine/src/store.rs`, `app/engine/tests/update_tests.rs`)
- Build: 2026-11-15 (found by CI run 43729118128; the same
  suite passed on the VPS at the same head, 412/412 —
  grabber 28/28 and sync 23/23 finally proven on the runner
  in this run; video_tests never ran, cargo stopped at
  update_tests)

Steps
1. On the runner, run `cargo test` in `app/engine`.
2. Every binary passes except `update_tests`, where exactly
   one test fails:
   `newer_version_available_shows_changelog_and_never_installs`
   panics at `tests\update_tests.rs:95:59` —
   `called Result::unwrap() on an Err value:
   Db(SqliteFailure(Error { code: DatabaseBusy, extended_code: 5 },
   Some("database is locked")))` — on
   `Store::open(...).unwrap()` in `test_mgr`, not on any
   changelog assertion.

Expected: the test opens its own fresh database and the
update check runs.
Actual: `Store::open` itself fails with SQLITE_BUSY
("database is locked").

Root cause: two holes, both proven from the code and by a
reproduction probe (temporary `tests/busy_probe_tests.rs`,
deleted after verifying the fix).
- Trigger (test harness): `update_tests.rs::tmpdir()`
  built `fetchline-test-update-{pid}-{nanos}` with NO
  disambiguating counter — unlike `common::tmpdir`
  (BUG-040). The 9 update tests run in parallel in one
  binary, share the PID, and can call `tmpdir()` in the
  same clock tick, so two tests open the SAME fresh
  `app.db` at the same time.
- Mechanism (engine): `Store::open` ran
  `execute_batch(SCHEMA_SQL)` — which WRITES (CREATE
  TABLE + INSERT OR IGNORE) — with NO busy timeout set
  (SQLite default 0), so the loser of the race failed
  instantly with SQLITE_BUSY. And the module header
  claimed "WAL mode" but no journal-mode pragma was ever
  set, so readers and writers contended in
  rollback-journal mode.
- The probe: 8 threads x 100 rounds of concurrent
  `Store::open` on one fresh path failed 11/800 pre-fix
  with the exact CI error text ("Database error: database
  is locked").
- Subtlety found by the probe: the first fix attempt —
  an unconditional `PRAGMA journal_mode=WAL` — made it
  WORSE. A per-step diagnostic (100 rounds x 8, tallying
  each step) proved `PRAGMA journal_mode=WAL` itself
  returns SQLITE_BUSY under a race: changing the journal
  mode needs an exclusive lock and SQLite does NOT consult
  the busy handler for it. The busy_timeout pragma itself
  never failed; execute_batch never failed once
  busy_timeout was set.

Fix:
- Engine (`src/store.rs::Store::open`):
  `PRAGMA busy_timeout = 5000` before any statement — a
  transient lock now waits instead of failing. 5 s is far
  longer than any schema/migration write, far shorter
  than a user would notice. `PRAGMA journal_mode=WAL` is
  now BEST EFFORT: on SQLITE_BUSY it is swallowed (WAL is
  an optimisation, not a requirement — the database keeps
  whatever mode it already has and the open still
  succeeds); any other pragma error still propagates.
  This also fixes the real production contention: the
  native host opens the same app.db while the app runs.
- Test (`tests/update_tests.rs::tmpdir`): a static
  AtomicUsize counter, same shape as `common::tmpdir` —
  `fetchline-test-update-{pid}-{n}-{nanos}` — so two
  tests can never share a directory.
- Nothing is weakened: no assertion changed, no test
  deleted or skipped. Probe re-run after the fix: 0/800
  staggered + 0/800 barrier-synchronised + 0/8
  single-round. Regression tests kept: the failing test
  itself (the probe was temporary; its numbers are
  recorded here).
Verified: `cargo check --tests` 0 warnings; update_tests
5x 9/9; full engine suite 413/413 on Linux. VPS
verification (2026-11-15 ~21:05 EET, Windows Server 2025,
instance i-0013aa5b18cec04b8, checkout verified at commit
2a75c089 "Fix BUG-041: busy_timeout + best-effort WAL in
Store::open, counter in update tmpdir"): full engine suite
**412/412 passed, 0 failed** (`cargo test --no-fail-fast`,
log C:\Windows\TEMP\bug041-full-suite.log, 30,666 bytes,
last written 17:04 UTC; copy in
~/workspace/vps-test-evidence/bug041-full-suite.log and
s3://fetchline-vps-evidence-ce1331eb/bug041-full-suite.log).
Per-binary: lib 56, antivirus 17, engine_tests 111,
extension 32, final_verify2 4, final_verify 16, grabber 28,
grabber_verify 14, kerberos 14, options 14, progress 7,
proxy 15, quota 19, real_download 2, sync 23, update 9,
video 31, doc-tests 0 — zero FAILED lines, zero panics. The
previously failing test
`newer_version_available_shows_changelog_and_never_installs`
passes (update_tests 9/9). 412 = Linux's 413 minus the one
#[cfg(not(windows))] shutdown test. **BUG-041 is proven
fixed on Windows.** (Retrieval was delayed by an expired AWS
session; the suite itself had finished before the expiry and
the instance was left running by the user's choice.)

## Round 34 — BUG-042: capture test Phase 2 takes Chrome's built-in Hangouts component extension for ours — 2026-11-15

Symptom (new repo xsonicx1998/fetchline, CI run #1
43739218909, head e591f95): build-windows SUCCESS,
smoke-test SUCCESS, extension-capture-test FAILED at step 6
"Run browser capture test" after 6 s, exit 1. Log line 20
verbatim: `FAIL: browser: discovered extension id
nkeimhogjdpnpccoofpliimaahmaaome is not the stable id
mioamdgogndabpcjbdekggcnlkkfpmbj derived from manifest.json's
key — the extension was not loaded with its shipped key`.
Phase 1 (host protocol) PASSED on the runner.

Root cause — test harness only, proven, not guessed:
`nkeimhogjdpnpccoofpliimaahmaaome` is Chrome's built-in
hidden "Google Hangouts" component extension
(`hangout_services`), shipped with every Chrome install —
confirmed by web search (Simon Willison's 2024 post on the
hidden extension; `chrome://system` extension listings show
the same id as "Google Hangouts"). `/json/list` returns
EVERY target, including component-extension service workers,
and the old `extension_id_from_targets` returned the FIRST
`chrome-extension://` URL in the list — which was Hangouts'
worker, not ours. The extension itself was never the problem:
the key-derived id is correct (`chrome_id_from_key` on the
repo manifest's key returns `mioamdgogndabpcjbdekggcnlkkfpmbj`,
verified locally), and on the user's real PC the extension
shows the stable id. The Preferences fallback (which matches
by extension-dir path) would have returned the right id, but
it never ran — the CDP discovery returned a (wrong) id first.

Fix (`scripts/capture-test.py`, test-only; no product code,
no assertion weakened):
- New `find_expected_target_id(port, expected_id)`: polls
  `/json/list` until a target carrying the EXPECTED id
  appears — the only sound way to identify our extension
  among the targets is to look for the expected id itself.
  It also collects every chrome-extension id seen, so the
  log now shows what was present (Hangouts is logged as
  ignored, not mistaken for ours).
- The Preferences fallback stays, with a short poll (it is
  written when Chrome registers the extension), and the
  strict comparison `discovered != expected -> FAIL` is
  unchanged: if Chrome ever really loaded our extension
  with a path-derived id, the Preferences entry would hold
  that id and the check would still fail — proven by a
  synthetic case (see below).
- The old first-match `extension_id_from_targets` is deleted.

Verified locally: `py_compile` OK; the discovery logic was
checked against 5 synthetic cases (Hangouts-first target
list -> correct id; our worker absent -> None with the seen
list; Preferences path-match -> correct id; a path-derived
id in Preferences still surfaces for the strict check; key
derivation -> stable id) — all pass. Phase 1 dry-run green
locally against the release host (ping/get_settings/
download_link + pending_links row). Phases 2–3 need the CI
run; the fixed test now actually exercises the real
Chrome -> host handshake on the runner for the first time.

Note: this does NOT explain the user's PC failure (his
extension shows the stable id; the spawn failure there is a
separate, still-open question). If the runner reproduces the
same "could not reach the host" failure at the popup step,
that gives a place to iterate on the real handshake with
logs, instead of guessing on his PC.

## Round 35 — BUG-042 follow-up: branded Chrome 137+ silently ignores --load-extension, so Phase 2 never installs the extension — 2026-11-15

Symptom (new repo xsonicx1998/fetchline, CI run #2
43745800633, head 57ab16a5): build-windows SUCCESS,
smoke-test SUCCESS, extension-capture-test FAILED at step 6
"Run browser capture test" after 36.0 s, exit 1. Log read
via the cloud browser as xsonicx1998: PHASE 1 PASSED; PHASE 2
FAILED in discovery. Exact FAIL line (only one): `FAIL:
browser: could not discover the extension ID (no
chrome-extension:// target with the expected id and no
Preferences entry)`. Line 19: expected id
mioamdgogndabpcjbdekggcnlkkfpmbj not among CDP targets (seen:
only the four Chrome component extensions, Hangouts among
them); the Preferences fallback found nothing either. PHASE 3
never ran; no popup step was reached. The 36 s is exactly the
discovery budget (6 s Phase 1 + 20 s CDP poll + 10 s
Preferences poll) — Path A, discovery exhaustion, NOT the
popup handshake.

Root cause — test harness only, proven from the log +
Chrome's own documented behaviour, not guessed: the test
launched headless Chrome with `--load-extension=<dir>`, and
branded Google Chrome IGNORES that flag. Chrome 126+ first
made it ephemeral (the extension is accessible as a
filesystem resource but is never registered in the profile:
no service worker starts, nothing is written to Preferences
/ Secure Preferences); Chrome 137 REMOVED it from branded
builds outright (Chromium RFC, March 2025 — the flag was
abused to push unwanted software; removal notice 4 April
2025). The flag is accepted and the extension is silently
never registered — which is precisely what run #2 logged:
the worker never appears in /json/list AND no Preferences
entry exists, while the component extensions do appear. The
runner's Chrome is the branded build at `C:\Program
Files\Google\Chrome\Application\chrome.exe`, so
--load-extension is dead there; the user's real Chrome loads
the same extension fine because he loads it through
chrome://extensions (a real install), not the flag. Adding
--enable-unsafe-extension-debugging alone does NOT restore
--load-extension on branded Chrome 137+ (verified against
Chrome 151; the --disable-features=DisableLoadExtension-
CommandLineSwitch workaround died in Chrome 142) — so the
one-flag fix was checked and rejected before betting the
iteration on it. The supported replacements are Chromium /
Chrome for Testing (where --load-extension still works) or
installing through CDP — and this test must run against the
branded Chrome the user actually has, so it uses the CDP
route.

Fix (`scripts/capture-test.py`, test-only; no product code,
no assertion weakened):
- New `PipeCDP` class: a minimal CDP client over Chrome's
  --remote-debugging-pipe. Messages are JSON separated by NUL
  bytes; the reader runs on a thread (select() cannot wait on
  a pipe on Windows). Transport setup follows Chromium's own
  pipe_builder.cc (ChromeDriver): POSIX uses fds 3/4; Windows
  passes the two pipe HANDLE values with
  --remote-debugging-io-pipes (inherited handles keep their
  value in the child). The POSIX fd placement moves the
  parent ends out of the way first — a naive dup2 order can
  close a pipe end still needed (dup2(wB, 4) closing wA when
  the pipes come back as 3,4,5,6); caught while writing the
  fake-peer test, fixed before pushing.
- New `install_extension_via_pipe()`: opens the pipe, probes
  `Browser.getVersion` (readiness), then calls
  `Extensions.loadUnpacked` with the extension dir. That CDP
  command exists ONLY on the pipe transport, gated on
  --remote-debugging-pipe + --enable-unsafe-extension-
  debugging (web-ext's approach since Chrome 126); it writes
  to Secure Preferences exactly like clicking "Load unpacked"
  in chrome://extensions, so the extension becomes a real
  install with a normal service-worker lifecycle. The
  returned id is checked against the stable id (strict
  `installed != expected -> FAIL`, same rule as discovery).
- `phase_browser` now installs via the pipe first, then
  launches the test Chrome WITHOUT --load-extension and WITH
  --enable-unsafe-extension-debugging, loading the extension
  from the profile it was just registered in. The rest of
  Phase 2 is unchanged: CDP worker poll, Preferences
  fallback, popup handshake, strict discovered == expected.
- The Preferences fallback now checks BOTH
  `Default/Preferences` and `Default/Secure Preferences`
  (registration is mirrored between them on different flush
  schedules). On discovery failure the test now stops Chrome
  cleanly, re-reads the profile (Chrome flushes on exit —
  distinguishes "registered but worker never appeared" from
  "never installed"), and appends Chrome's stderr tail to
  the report. Chrome's stderr is captured to a file instead
  of DEVNULL in both launches — the old code destroyed the
  only evidence of a silent load failure (the uploaded
  artifact only carries capture-test-report.txt, so the tail
  rides inside it).
- Pipe and port are never combined in one launch
  (Puppeteer's launcher asserts pipe XOR port): install in
  launch A (pipe), test in launch B (port 9222).

Verified locally: `py_compile` OK; 14 synthetic pipe tests
pass against a fake Chrome speaking \0-delimited CDP on
fds 3/4 (happy path probe + loadUnpacked; event
interleaving skipped; 1-byte chunked replies reassembled;
wrong id returned as-is for the caller to fail; error reply
-> RuntimeError; early exit -> RuntimeError; silence ->
TimeoutError; install fn returns the installed id; both
Preferences files path-match; no match; missing files;
stderr tail). Those tests validate the harness code only —
Chrome's side of the pipe is covered by Chromium's
documented behaviour, not by the fake. Phase 1 dry-run green
locally against the release host (ping/get_settings/
download_link + pending_links row). Full engine suite
413/413, 0 failed; `cargo check --tests` 0 warnings; `npm
run build` passes; extension validators pass. Phases 2–3
need the CI run.

Note: this still does NOT explain the user's PC failure
(his extension shows the stable id; the host runs
standalone; no crash in Event Viewer). If run #3 reaches
the popup and fails there with "could not reach the host",
that reproduces his failure on the runner for the first
time and the iteration continues from the popup's error,
not from discovery.

## Round 36 — BUG-042 follow-up: PipeCDP passes pass_fds on Windows, which CPython rejects outright — 2026-11-15

Symptom (new repo xsonicx1998/fetchline, CI run #3
43748778847, head 8f7641b6): build-windows SUCCESS,
smoke-test SUCCESS, extension-capture-test FAILED at step
"Run browser capture test" after only ~1 s, exit 1 — the
script died at startup, before Phase 2/3. Log read via the
cloud browser as xsonicx1998: PHASE 1 PASSED, then the
traceback (only one):
```
File ".../scripts/capture-test.py", line 527, in install_extension_via_pipe
    pipe = PipeCDP(chrome_exe, profile_dir, stderr_path)
File ".../scripts/capture-test.py", line 391, in __init__
    self._proc = subprocess.Popen(...)
File "C:\hostedtoolcache\windows\Python\3.12.10\x64\Lib\subprocess.py", line 1448, in _execute_child
    assert not pass_fds, "pass_fds not supported on Windows."
AssertionError: pass_fds not supported on Windows.
```
Chrome was never launched through the pipe; the extension
was never installed.

Root cause — test harness only, proven from the traceback
+ CPython's own source, not guessed: `PipeCDP.__init__`
built the Windows launch correctly (inheritable pipe
handles + `--remote-debugging-io-pipes=<h1>,<h2>`) but then
passed `pass_fds=child_fds` to `subprocess.Popen`
unconditionally. CPython's Windows `_execute_child`
(Lib/subprocess.py:1448) starts with `assert not pass_fds`
— `pass_fds` is POSIX-only by design (the docs say so; on
POSIX it forces close_fds True). The new pipe-install code
was written and synthetically tested on the POSIX path only
(the 14 fake-peer tests run on fds 3/4 under Linux), so this
Windows-only failure was never exercised before CI. The idea
itself (install via `Extensions.loadUnpacked` over
`--remote-debugging-pipe`) is unaffected — only the Windows
handle-passing was wrong.

Fix (`scripts/capture-test.py`, test-only; no product code,
no assertion weakened):
- Windows now launches with `close_fds=False` and NEVER
  `pass_fds`: the child inherits the two pipe HANDLEs
  because they are flagged inheritable (`os.set_inheritable`)
  and CreateProcess is called with bInheritHandles
  (`int(not close_fds)` — verified in CPython 3.12's
  Lib/subprocess.py `_execute_child`); inherited HANDLEs keep
  their numeric value in the child, which is exactly what
  `--remote-debugging-io-pipes` names (confirmed by
  chromedp's README: "On Windows the pipes are handles that
  the switch --remote-debugging-io-pipes names", and by the
  CPython docs: with close_fds false, "file descriptors obey
  their inheritable flag", and on Windows close_fds true
  inherits nothing except STARTUPINFO handle_list / standard
  redirection). POSIX is unchanged (fds 3/4 + pass_fds).
- Failure is loud and early on either platform: on Windows
  the handles are checked with `os.get_inheritable` right
  after flagging them — if they are not inheritable the
  constructor raises a RuntimeError naming the platform
  before launching anything; a Popen AssertionError is
  caught and re-raised as a RuntimeError naming the platform
  and the mechanism (`close_fds=False`, never pass_fds),
  which `install_extension_via_pipe` already catches and
  turns into a `FAIL: browser:` line — run #3's bare
  traceback can no longer escape the script.
- New committed test file `scripts/test_capture_pipe.py`
  (7 tests, run: `python3 scripts/test_capture_pipe.py`).
  The previous 14 synthetic tests were ephemeral (only
  their logs survived in /tmp); this file replaces them with
  a committed suite: 5 POSIX fake-peer tests (normal reply,
  event interleaved, chunked reply reassembled, error reply
  -> RuntimeError, early exit -> RuntimeError) plus 2
  Windows-branch tests that drive the real `PipeCDP.__init__`
  with `os.name` patched to `"nt"`, a fake `msvcrt`, and a
  Popen mock that raises exactly as CPython does on Windows
  whenever `pass_fds` is passed — asserting the call never
  carries `pass_fds`, carries `close_fds=False`, and names
  the io-pipes handles; and that non-inheritable handles fail
  before any launch. The Windows path itself can only be
  proven by the CI run (no Windows Python on this VM).

Verified locally: `py_compile` OK for both files; new test
file 7/7 pass; Phase 1 dry-run green locally against the
release host (ping/get_settings/download_link +
pending_links row). Full engine suite 413/413, 0 failed;
`cargo check --tests` 0 warnings; `npm run build` passes.
Phases 2–3 need the CI run.

Note: this still does NOT explain the user's PC failure
(his extension shows the stable id; the host runs
standalone; no crash in Event Viewer). If the CI run now
reaches the popup and fails there with "could not reach the
host", that reproduces his failure on the runner for the
first time and the iteration continues from the popup's
error, not from discovery.

## Round 37 — BUG-042 follow-up: the extension's icons are missing from the repo, so Chrome's loadUnpacked refuses to install it — 2026-11-15

Symptom (new repo xsonicx1998/fetchline, CI run #4
43750793531, head 5869dfb4): build-windows SUCCESS,
smoke-test SUCCESS, extension-capture-test FAILED at step
"Run browser capture test" after only ~7 s, exit 1. Log
read via the cloud browser as xsonicx1998; the exact FAIL
line, verbatim:
```
FAIL: browser: could not install the extension via CDP pipe: Extensions.loadUnpacked failed: {'code': -32600, 'message': "Could not load icon 'icons/icon16.png' specified in 'action'."}
```
Phase 1 PASSED; Phase 2 died at the install step
(`installing extension via CDP pipe
(Extensions.loadUnpacked)`); the popup handshake and Phase
3 never ran.

Root cause — proven, not guessed: `manifest.json`
`action.default_icon` names `icons/icon16.png`,
`icon32.png`, `icon48.png`, `icon128.png`, and those files
exist locally (174–847 bytes each) but are NOT in the repo
(raw.githubusercontent.com at head 5869dfb4 returns 404 for
`app/extension/icons/icon16.png` while manifest.json returns
200). `scripts/generate-icons.py` creates them from the
committed `app/extension/icons/icon.svg`, and the
`build-windows` job runs it — but the
`extension-capture-test` job never did (its dependency step
only installed `websocket-client`). Chrome's
`Extensions.loadUnpacked` strictly validates the icons a
manifest names, so the install fails before any extension
code runs. This also explains runs #1/#2: the extension
NEVER loaded in CI at all — the Hangouts first-match (run
#1) and the discovery exhaustion (run #2) are downstream of
that. The branded-Chrome 137 removal of `--load-extension`
is real but secondary here: even with the flag, an
extension with missing icons cannot load.

Why the fix is a workflow step and not committed PNGs:
the connector's `push-files` builds a local UTF-8 snapshot
— binary files fail its validation (the same reason a
`.pyc` had to be dropped from the new-repo upload), and the
MCP `create_or_update_file` / `push_files` tools carry
text content only ("Do not base64-encode it; this server
does that"), so no connector route can commit a PNG. The
icons stay generated, not committed, exactly as the
workflow header comment already states.

Fix (CI-only; `.github/workflows/build-windows.yml`,
`extension-capture-test` job, "Install capture-test
dependencies" step — no product code, no test code, no
assertion changed):
```
- name: Install capture-test dependencies
  run: |
    python -m pip install websocket-client pillow
    python scripts/generate-icons.py
```
with a comment naming run #4's exact error and why the
step exists. This mirrors the `build-windows` job, which
already runs the same two commands.

Verified locally: workflow YAML parses (jobs
build-windows / smoke-test / extension-capture-test; 346
lines); fresh-checkout simulation — a temp dir with the
repo contents MINUS the icons, running the new step's exact
commands, regenerates all four PNGs byte-identical to the
local ones (md5 icon16 5ae9c6dd…, icon32 ca6855ff…,
icon48 8a9b9e95…, icon128 f15a136e…) and every icon
reference in manifest.json resolves; `py_compile` OK;
full engine suite 413/413, 0 failed; `cargo check --tests`
0 warnings; `npm run build` passes. A real Chrome load
cannot run on this VM; the CI run is the proof for that.

Status, updated in Round 38: the workflow file WAS uploaded
via a browser task (signed in as xsonicx1998, Edit on the
EXISTING file page) at ~21:43 EET as commit 998b358
("Generate extension icons in capture-test job (BUG-042
follow-up)", 11 additions / 1 deletion), verified by
raw.githubusercontent read-back. It triggered run #5
(43753981942, head 998b358, completed ~21:55 EET):
build-windows SUCCESS + smoke-test SUCCESS,
extension-capture-test FAILED — but the step ran 38 s (vs
run #4's 7 s). Log read via the cloud browser as
xsonicx1998 (job 132936322990): the install NOW WORKS
("CDP install returned id
mioamdgogndabpcjbdekggcnlkkfpmbj" / "installed id matches
the stable id") — `Extensions.loadUnpacked` succeeded for
the first time in CI. Phase 1 PASSED. Then discovery
failed with the same message as run #2:
`FAIL: browser: could not discover the extension ID (no
chrome-extension:// target with the expected id and no
Preferences entry)` — the installed extension never
appeared in the test launch's CDP /json/list (only the 4
component extensions) and the Preferences fallback found
nothing. The popup handshake + Phase 3 never ran. The
icons fix is proven; the failure moved to the two-launch
design — see Round 38.

Note: this is CI-only and still does NOT explain the
user's PC failure (his extension ZIP was built from the
local dir, which HAS the icons). If run #5 reaches the
popup and fails there with "could not reach the host",
that reproduces his failure on the runner for the first
time. (Round 38: it did — and the cause turned out to be
BUG-043, a real product bug, not the test.)

## Round 38 — BUG-042 follow-up (two-launch design is unfixable) + BUG-043 (real product bug: the host exits on Chrome's launch arguments, so the popup handshake could never succeed anywhere) — 2026-11-15

Symptom (new repo xsonicx1998/fetchline, CI run #5
43753981942, head 998b358): build-windows SUCCESS,
smoke-test SUCCESS, extension-capture-test FAILED at step
"Run browser capture test" after 38 s (vs run #4's 7 s),
exit 1. Log read via the cloud browser as xsonicx1998
(job 132936322990); the exact FAIL line, verbatim:
```
FAIL: browser: could not discover the extension ID (no chrome-extension:// target with the expected id and no Preferences entry)
```
PROGRESS: the install now works — "CDP install returned
id mioamdgogndabpcjbdekggcnlkkfpmbj" / "installed id
matches the stable id" (`Extensions.loadUnpacked`
succeeded for the first time in CI; the Round 37 icons fix
is proven). Phase 1 PASSED. Then discovery failed: the
installed extension never appeared in the test launch's
CDP /json/list (only the 4 component extensions) and the
Preferences fallback found nothing. Popup handshake +
Phase 3 never ran.

Root cause 1 (test design, proven on the VPS) — the
two-launch design cannot work. Experiments on the VPS
(fresh clone at C:\fetchline2, head 998b358, branded
Chrome 157.0.8103.79, debug scripts
~/workspace/vps-test/dbg-profile.py, dbg2-profile.py):
- A (terminate, the old behaviour): `loadUnpacked`
  returns the stable id and the worker IS running in
  launch A itself (`Target.getTargets` shows
  `service_worker chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/background.js`),
  but while A is alive AND after terminate, `Default/
  Preferences` and `Default/Secure Preferences` DO NOT
  EXIST on disk — the registration never flushed.
  Launch B finds nothing; its popup target stays
  about:blank.
- B (clean shutdown via `Browser.close`): after A exits,
  Secure Preferences DOES contain the extension
  (`id=mioamdgogndabpcjbdekggcnlkkfpmbj`,
  `path='C:\fetchline2\app\extension'`, `location=4`) —
  the registration persists on clean shutdown. But
  launch B STILL finds nothing, and after B's clean stop
  the entry is GONE from Secure Preferences. Launch B
  itself removes it.
- C (B without `--enable-unsafe-extension-debugging`)
  and D (B with it): identical — the flag is innocent.
Two separate defects: (1) `pipe.close()` terminates
launch A, so Chrome never flushes the registration to
disk; (2) a fresh Chrome launch REMOVES a
`loadUnpacked`-installed extension (stored location=4,
the unpacked/load kind, tied to the installing session)
— so a clean shutdown alone is NOT sufficient either.
Chrome's stderr was clean in all launches. The fix is
not a better discovery check: there is nothing to
discover in a second launch. The whole phase must run in
ONE launch.

Root cause 2 (BUG-043, REAL PRODUCT BUG — found by the
single-launch experiment, and it explains the user's PC
failure too). The single-launch experiment (dbg3:
install via pipe in launch A, then `Target.createTarget`
popup.html + `attachToTarget` + `Runtime.evaluate` over
the pipe) proved the mechanism works — the popup loaded
(`document.title: 'Fetchline'`) — but its status went to
"Desktop app not reachable" and Chrome's stderr showed:
```
ERROR:chrome\browser\extensions\api\messaging\native_message_process_host.cc:378] Error when writing to Native Messaging host: -101
```
three times. The host itself answers fine when driven
directly (VPS dbg4: ping → `{"ok":true,"type":"pong"}`,
returncode 0; CI Phase 1 passes the same way). A
throwaway wrapper exe (~/workspace/vps-test/
host-wrapper.rs, logs every spawn then execs the real
host) proved why: Chrome spawns the host with TWO extra
arguments — the extension origin, then
`--parent-window=<hwnd>`:
```
args: ["...host-wrapper.exe", "chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/", "--parent-window=1966140"]
args: ["...host-wrapper.exe", "chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/", "--parent-window=0"]
```
and the host's `main()` rejected ANY unknown argument
with `exit(2)` — so it died instantly on every Chrome
spawn, before reading a single frame, and Chrome's write
failed with -101 (ERR_CONNECTION_RESET). Through the
wrapper (which drops the extra args) the popup reported
`Connected to the Fetchline desktop app` and all three
spawns exited 0. This was never caught because every
host test drives it directly (`--db-path` only) — the
Chrome spawn path was never exercised anywhere, on the
VPS, in CI, or on the user's PC. It is the same failure
the user sees on his PC ("Could not reach Fetchline:
Error when communicating with the native messaging
host") despite a correct install, correct registry key,
correct manifest, and a host that runs standalone.

Fix 1 (product, `app/host/src/main.rs` — BUG-043):
arg parsing extracted into `parse_host_args`, which
ignores the browser's launch arguments — any
`chrome-extension://…/` or `moz-extension://…/` origin,
and `--parent-window=<hwnd>` (plus the bare
`--parent-window` form, consuming a following numeric
handle only, so a following real flag is never eaten).
`--db-path`, `--help`/`-h` unchanged; genuinely unknown
arguments still exit(2) with the usage text. Regression
tests: 2 unit tests in main.rs (Chrome argv ignored in
both orders and mixed with `--db-path`; Firefox origin;
unknown args still rejected) + 1 integration test
(`app/host/tests/chrome_argv_spawn.rs`, process-level:
spawns the built host exactly as Chrome does and
requires a framed pong, then EOF and a clean exit 0).

Fix 2 (test-only, `scripts/capture-test.py` — the
two-launch design): `phase_browser` is now ONE launch
over the CDP pipe — install via `Extensions.loadUnpacked`,
then `Target.createTarget` popup.html +
`Target.attachToTarget` (flatten) + `Runtime.evaluate`
on that session, polling the status text up to 20 s.
`PipeCDP.call` gained an optional `session_id`
(sent as `sessionId`, replies matched on id AND
sessionId; a new session-mismatch test covers the skip).
`install_extension_via_pipe` became `install_extension`
(takes an open pipe, returns the id, does NOT close the
launch). Deleted dead code: `chrome_targets`,
`find_expected_target_id`, `extension_id_from_profile`
(the /json/list discovery + Preferences fallback can
never succeed — see root cause 1), the websocket popup
check, and the `re` / `urllib.request` imports. Phase 2
is stdlib-only now (websocket-client no longer needed;
the workflow still installs it — harmless, left alone
to avoid a workflow change this round). Clean shutdown
(`Browser.close` best-effort, then `pipe.close()`) at
the end of the phase. No assertion weakened: the
installed id must still equal the stable id, and the
popup must still report "Connected to the Fetchline
desktop app". The icon fix, pass_fds fix, and
--load-extension handling are untouched (all proven).

Verified locally: `py_compile` OK; pipe tests 9/9
(7 old + 2 new session tests,
scripts/test_capture_pipe.py); host tests 3/3
(`cargo test -p fetchline-host`); full engine suite
413/413, 0 failed; `cargo check --tests` 0 warnings;
`npm run build` passes. Fresh-checkout simulation =
the VPS run below (fresh clone + the synced files).

Verified on the VPS (real Windows, branded Chrome
157): host tests 3/3 pass on Windows (`cargo test -p
fetchline-host`, SSM command d806adce, 1m 31s). Then,
with the FIXED host (no wrapper) and the rewritten
script: dbg3 `RESULT PASS: popup reports Connected to
the Fetchline desktop app`, and the real
`capture-test.py` Phases 1+2 (`--skip-drain`):
`PHASE 1 PASSED` (ping / get_settings /
download_link / pending_links row confirmed) +
`PHASE 2 PASSED` (registry key + manifest verified,
CDP install returned the stable id, popup status text
'Connected to the Fetchline desktop app') →
`CAPTURE TEST PASSED (phases run: host, browser)`.
Phase 3 was skipped: it needs the desktop app's GUI,
which cannot run on the VPS (session 0) — CI run #6
is the proof for it. The user-facing consequence: the
next installer (run #6) contains the BUG-043 fix, so
installing it should make capture work on his PC too —
the same spawn that failed there now succeeds on the
VPS with the identical Chrome + host.

Status: pushed with the code fix in this round's
connector push to xsonicx1998/fetchline (files:
`app/host/src/main.rs`,
`app/host/tests/chrome_argv_spawn.rs`,
`scripts/capture-test.py`,
`scripts/test_capture_pipe.py`, `replica/bugs.md`
— this entry, plus the Round 37 status correction
above). No workflow change. Run #6 is the proof.

## Round 39 — BUG-044 (live `download-progress` events never reach the frontend: no Tauri capability grants `plugin:event|listen`) + progress window (S03) — 2026-11-15

Symptom: none visible — that is the bug. The main window's
`download-progress` listener (App.tsx `onProgress`) never fired
in the built app, so the old progress overlay's "live" speed, ETA
and 60 s speed graph never updated between the 1 s table polls;
the graph never got a sample in production and showed its "builds
as the download runs" placeholder forever. The 1 s `listDownloads`
poll masked it completely (rows carry speed, ETA and scan results),
so nothing ever looked broken.

Root cause (proven, not inferred): this app ships NO Tauri
capability files — no `src-tauri/capabilities/` directory, no
`app > security > capabilities` in `tauri.conf.json`, no
`src-tauri/permissions/` app manifest — and no generated
capability in the build output. Vendored tauri 2.12.5
(`src/webview/mod.rs`, `on_message`) checks the ACL exactly
when the command is a `plugin:` command, or the app defined an
ACL manifest, or the origin is remote; with an empty resolved
ACL every `plugin:` command is rejected. The installed
`@tauri-apps/api` 2.12.4 routes `listen` to
`invoke('plugin:event|listen', …)`, `WebviewWindow` creation to
`plugin:webview|create_webview_window`, and `setTitle` to
`plugin:window|set_title` — all denied. App commands (`invoke`
of a `#[tauri::command]`) run ungated: local origin, no app ACL
manifest, no `plugin:` prefix. The same denial also silences
the other two frontend listeners — `queue-action-pending`
(shutdown/hibernate countdown never appears mid-session; only
the mount-time `getPendingAction` sweep catches one) and
`update-available` (the weekly-check dialog never appears) —
those two are NOT fixed here; their fix is a capabilities file
or polling, a separate decision.

Fix (this round): the progress window is built around the ACL
instead of through it — created, focused, resized and titled
from Rust (`open_progress_window` / `set_progress_window_compact`
commands + title updates in the progress-event forwarder in
`app/src-tauri/src/lib.rs`), and its live feed is polling
(`list_downloads` + new `get_progress_snapshot`, backed by
`DownloadManager::last_progress`, stashed in `emit()`). The dead
`onProgress` listener is removed from App.tsx (with its `live`
state and the `onProgress` bridge helper) rather than left to
fail quietly; the 1 s poll is documented as the table's real
live path. No download behaviour changed: pause/resume/cancel/
retry reuse the existing commands, and the engine only gained
stored facts (`accept_ranges` on the row, `show_progress_window`
on settings) plus the snapshot accessor.

Status: fixed as described; engine tests 3 new (settings
round-trip, `accept_ranges` persistence, snapshot matches the
completion event). The window itself has never opened in the
running desktop app — no GUI test on this VM or the VPS — so a
human pass on real Windows is still needed (auto-open, title,
Hide details resize, close-does-not-cancel).

## Round 40 — BUG-044 resolution (capabilities file restores the two dead listeners) + Sonic Manager rebrand/retheme — 2026-11-15

BUG-044 (found in Round 39): the app shipped no Tauri capability
files, so every frontend `plugin:` command was denied by the ACL.
The progress window was built around it, but two listeners stayed
silently dead: `queue-action-pending` (the shutdown countdown
never appeared mid-session) and `update-available` (the weekly-
update dialog never appeared).

Fix (this round, approach (a) — capabilities file, the standard
Tauri fix): new `app/src-tauri/capabilities/default.json`,
granting exactly `core:event:allow-listen` +
`core:event:allow-unlisten` to the `main` window only (verified
against vendored Tauri 2.12.5: capability files in
`src-tauri/capabilities/` are auto-enabled by
`generate_context!()`; the main window's label is `main`). No
other permission is granted, and the progress windows (labels
`progress-<id>`) get nothing — they keep polling by design, and
the removed `download-progress` listener stays removed. The two
listeners in App.tsx (`onQueueAction`, `onUpdateAvailable`) now
work as written; the stale BUG-044 comments in App.tsx, api.ts
and ProgressWindow.tsx were updated to say so. The same file is
what CI's Windows build will validate — the Tauri shell cannot be
compiled on this Linux VM (gdk-3.0), so the first real proof is
the CI build passing and, on the user's PC, the shutdown countdown
and the weekly-update dialog appearing.

Rebrand/retheme in the same round (user's own images; display
name "Sonic Manager"; identifiers com.fetchline.app /
com.fetchline.host unchanged; repo rename is a separate step):
productName + window title, extension name/description/title,
README titles, all user-visible "Fetchline" copy in the app and
extension (comments and internal identifiers left alone; engine
error strings and the HTTP User-Agent still say "Fetchline" —
not in this round's scope, flagged for a follow-up decision).
Palette sampled from the logo/background/reference with PIL
(navy #0a0f24, neon red #f73244, neon blue #23a9ea; gold #d9b350
appears in the background art only, not as a UI token). Tokens
use neon red/blue on dark and deeper red/blue on light
(#c92a2a / #0b6fc4). Contrast re-checked on the tokens.json
pairs (24/24 pass, text 4.5 / UI 3.0); text-faint is
decorative-only (ring/icon strokes) and passes the 3.0 UI
threshold. New ProgressRing component (table rows 24 px,
progress-window header 56 px with the percent inside); primary
buttons get the glow shadow; extension popup restyled dark
Sonic. Icons from the logo: full logo for 1024/128/48, a head
crop (the red/blue curl) for 16/32 — the whole logo at 16 px
turns to mud (verified by eye). Background art as the main-window
backdrop under a dark scrim, dark theme only; the app root divs
carry no bg-bg class so the body backdrop shows through.

Sequencing (binary files): the logo/background JPGs cannot be
pushed via the connector, so they go to the repo through the
GitHub web UI first; only then do the adapted
scripts/generate-icons.py (reads logo.jpg, fails loudly if it is
missing) and app/src/background.css (+ its main.tsx import) land
in a follow-up push. This push keeps the old icon.svg-based
generator and no background import, so CI stays green meanwhile.
The workflow runs generate-icons.py in both the build-windows
and capture-test jobs, so the follow-up must land only after
logo.jpg is in the repo or both jobs fail at the icon step.

Status: fixed as described. Verified on Linux: full engine suite
**416/416 pass, 0 failed** (the manifest-name test was updated
to "Sonic Manager"); `cargo check --tests` 0 warnings (engine
and host; the Tauri app crate cannot be checked on this VM —
gdk-3.0); `npm run build` passes; icons generated locally at
every size with the 16 px legibility checked by eye; the
background build was verified with a temporary import (asset
emitted, then the import reverted). The new look has never been
seen on real Windows — a human pass on the user's PC is needed
for the final look, and the smoke-test screenshot from this
run's artifact is the launch proof.

## Round 41 — BUG-045 (rebrand fallout, test-only: the capture test still expected the old popup string) + theme finish (icons + background) — 2026-11-16

BUG-045 — test-only, found while watching run #10. Round 40 renamed
the extension popup's connected status to "Connected to the Sonic
Manager desktop app" (popup.js:94), but `scripts/capture-test.py`
still expected the old string: the Phase 2 poll at line 734 and the
strict check at line 742 both match on "Connected to the Fetchline
desktop app", and the module docstring (line 26) described the old
text. So even with a working popup and a reachable host, Phase 2
could never pass — the poll would exhaust its retries and fail with
"popup did not report a connection". Consistent with run #10
(a45e225): build-windows SUCCESS, smoke-test SUCCESS, but
extension-capture-test FAILED at "Run browser capture test". The
exact FAIL line from run #10's log was not read in this session
(no live-browser access here); the mismatch is proven from the
code alone and is a guaranteed Phase 2 failure either way.

Fix (test-only, this round): the three occurrences in
capture-test.py now expect "Connected to the Sonic Manager desktop
app" (docstring + both checks). No product code touched, no
assertion weakened — the check is the same strict match, against
the string the popup actually shows. `py_compile` OK; a repo-wide
grep confirms no "Connected to the Fetchline" string is left
anywhere the test or the popup reads.

Status: fixed as described. The fix rides in the same push as the
theme finish below, so the run it triggers is the proof.

## Round 42 — BUG-046 (MSI ships a host manifest pointing at the old install folder) — 2026-11-16

BUG-046 — product bug (packaging), proven from run #11's
capture-test-report.txt. Phase 1 PASSED (host protocol — BUG-043
fix holds); Phase 2 FAILED at the manifest check: `installed
manifest path is 'C:\Program Files\Fetchline\fetchline-host.exe',
expected the installed host 'C:\Program Files\Sonic Manager\
fetchline-host.exe'`. The registry key correctly points at
`C:\Program Files\Sonic Manager\com.fetchline.host.json`, but the
manifest FILE's `path` field is stale.

Root cause: `app/extension/com.fetchline.host.json:7` and
`app/extension/com.fetchline.host.firefox.json:7` still hardcode
`"path": "C:\\Program Files\\Fetchline\\fetchline-host.exe"`. The
MSI bundles these manifests as-is via bundle.resources; only the
NSIS installer rewrites the path at install time
(register-native-host.ps1). The Round 40 rename moved the install
folder to `C:\Program Files\Sonic Manager\`, so the MSI's manifest
points at a host that no longer exists there. Run #7 was green
pre-rename because the hardcoded path matched the install folder
then. Note the MSI caveat from the packaging fix still applies in
the other direction: a custom MSI install folder breaks the
manifest path again — the default install is what CI tests, and
the NSIS installer rewrites the path for any folder.

Fix (product, this round): both manifests' `path` now reads
`C:\Program Files\Sonic Manager\fetchline-host.exe`. The stale
old-path references in the comments of register-native-host.ps1,
fetchline-native-host.nsh, fetchline-native-host.wxs, and the
capture-test.py docstring example were updated to the new path
(cosmetic — the workflow passes the real paths explicitly). No
identifier changed (`com.fetchline.host`, `com.fetchline.app`
untouched); the workflow file was not touched (browser route
only).

Status: fixed as described. Verified on Linux: both manifests
parse as JSON with name `com.fetchline.host`, type `stdio`, and
unchanged allowed_origins/allowed_extensions; full engine suite
416/416 pass, 0 failed; `cargo check --tests` 0 warnings; `npm
run build` passes. A repo-wide grep finds no remaining
`C:\Program Files\Fetchline` in code — only historical
build-log.md entries, which are records of the past and are left
as-is. The run this push triggers is the proof — it also finally
exercises the BUG-045 popup-string fix, since Phase 2 previously
died before the popup handshake.

**Verdict (run #12, 43770074030, head de396268 — 2026-11-16): FULLY
GREEN.** build-windows SUCCESS, smoke-test SUCCESS,
extension-capture-test SUCCESS; installer 15,863,208 bytes. The
capture job passing proves both the BUG-046 manifest fix and the
BUG-045 popup-string fix (Phase 2's strict popup check — "Connected
to the Sonic Manager desktop app" — was reached and passed; the
report.txt text itself was not read, the job's success verdict is
the proof). BUG-046 is closed.

## Round 43 — dark theme by default + header logo — 2026-11-16

Not a bug: a user-requested look change. The user said the run #11
smoke screenshot (light theme) does not look like his dark reference
picture, the background was missing, and the header shows no logo —
only the tiny OS window icon next to the title. He approved dark as
the default theme.

Change (this round): the app now opens dark by default —
`App.tsx` initial theme state `'light'` → `'dark'`, and
`Settings::default` in `store.rs` now carries theme `"dark"`, so
fresh installs and fresh databases start dark (the stored setting
still wins for existing users, and the moon/sun toggle still
switches to light). The settings-defaults test was updated to
expect `"dark"`. The header now shows the logo next to the title:
`<img src={logoUrl} … className="h-8 w-8 rounded-md object-cover"
/>` importing `./assets/logo.jpg` (the user's logo, already in the
repo at `app/src/assets/logo.jpg`; the local tree needed the same
file copied in for the build — it is binary and was not pushed via
the connector). A new `app/src/vite-env.d.ts` (`/// <reference
types="vite/client" />`) was needed because `npm run build` runs
`tsc -b` first and TypeScript otherwise rejects the `.jpg` import
(TS2307); there was no such declaration before. The progress
window was deliberately left without a logo: its header IS the
download filename (truncated, with the title attribute), so a brand
mark there would compete with the file's identity — it still
applies the stored theme on mount, so it follows the dark default
automatically.

Status: changed as described. Verified on Linux: full engine suite
416/416 pass, 0 failed; `cargo check --tests` 0 warnings; `npm run
build` passes and emits both assets in dist/assets
(background-*.jpg and logo-*.jpg); the App root carries no `bg-bg`
class, so the body backdrop still shows through in dark mode. The
run this push triggers is the proof — its smoke screenshot should
show the dark theme with the background and the header logo.

## Round 46 — BUG-047: the Round 45 layout overflows at 1024×768 — 2026-11-16

Found in the run #19 smoke screenshot (1024×768), fixed in this
round. Two defects against the user's final reference image:

- **BUG-047a (toolbar overflow):** the toolbar's 96 px fixed
  columns × 11 buttons totalled ~1160 px, wider than the 1024 px
  window — "Stop Queue" and "Grabber" were cut off and a
  horizontal scrollbar appeared under the toolbar. Fix: buttons
  are `flex-auto` with `min-width: fit-content` (size to content,
  grow to fill the row) and the container is `overflow-hidden`
  instead of `overflow-x-auto`. Fit verified with real font
  metrics before building (DejaVu Sans Bold, wider than Inter:
  ~873 px of content against 1008 px available).
- **BUG-047b ("Main download queue" truncates):** the sidebar
  showed "Main download qu... 0 files" despite the 256 px width.
  Root cause: the hover-action buttons (sync/schedule) sat in the
  row's flow and took 24 px each even while invisible
  (opacity-0), leaving ~102 px for a ~140 px name. Fix: sidebar
  256 → 280 px, and the hover actions moved to an
  absolutely-positioned overlay at the row's right edge (count
  fades out under it on hover/focus; buttons stay
  keyboard-focusable). Same treatment for the category delete and
  grabber run buttons.

Status: fixed as described. Verified on Linux: full engine suite
417/417 pass, 0 failed; `cargo check --tests` 0 warnings; `npm
run build` passes. The run this push triggers is the proof — its
smoke screenshot should show all 11 toolbar buttons with no
scrollbar and "Main download queue 0 files" in full.

## Round 48 CI-fix — BUG-048: flaky `ftp_pause_then_resume_completes_byte_exact` — 2026-11-16

Test-only race, found in CI, not a product bug. The test failed
in run #8 (head ce293c5) and again in run #22 (head ab3af71f),
both times at exactly one test with "file finished before the
pause" (`app/engine/tests/proxy_tests.rs:716:5`); all other
suites passed (331 passed, 1 failed in run #22). The Round 48
change itself touched zero engine code, so the failure cannot be
caused by the round.

Root cause: the test downloaded a 2 MiB file from the fixture
FTP server (4 segments x 8 chunks x 20 ms chunk delay ≈ 200 ms
total) and then slept a fixed 200 ms before pausing. On a fast
runner the download was already finished when the pause landed:
the pause flag was set while the last segment finished its final
chunk, so the run loop paused with all bytes done and the
"paused" row showed the full size. The blind sleep raced the
download; the faster the runner, the more likely the failure.

Fix (test-only, `proxy_tests.rs`): wait for the run task to
report itself downloading (`wait_for_status(..., "downloading",
10)`) and pause immediately — the status flip and the download
start in the same task, so observing "downloading" bounds how
far the download can have got — and raise the fixture chunk
delay 20 ms → 50 ms, so the download takes ~400 ms and the pause
lands with most of the file still to go. The test still asserts
exactly what it asserted before (bytes > 0 and < full size at
pause, byte-exact file after resume). No engine source, no other
test, and no UI file changed.

Status: fixed as described. Verified on Linux: the proxy_tests
suite 15/15 three consecutive runs; full engine suite 417/417;
`cargo check --tests` 0 warnings; `npm run build` passes. The
run this push triggers is the proof — its build-windows job must
go green.

## BUG-048 follow-up (CI run #23): wait-for-status fires before any bytes — 2026-11-16

The BUG-048 fix above failed again in CI run #23 (43790293212,
head 9bca7f53) on the SAME test with the OPPOSITE timing
failure: `ftp_pause_then_resume_completes_byte_exact` panicked
at `proxy_tests.rs:726:5` with "pause landed before any bytes"
(331 passed, 1 failed; all other suites ok). The wait-for-status
gate was wrong: `set_status("downloading")` is the run task's
FIRST step, before any segment is spawned, so observing
"downloading" lets the pause land before a single byte is
written. The test's pause window is bounded on BOTH sides — it
needs bytes > 0 AND bytes < total at pause time.

Fix (test-only, `proxy_tests.rs`): replace the
wait-for-status gate with a wait for REAL PROGRESS — poll the
download's `.part` file until it holds a non-zero payload byte
(new helper `wait_for_part_bytes`, 5 ms poll, 10 s timeout that
fails loudly instead of pausing blind). The DB row cannot be
that gate: progress is checkpointed to SQLite only every 2 s
(`CHECKPOINT_INTERVAL`) while the whole 2 MiB download takes
~400 ms, so the row reads 0 for the entire download. The
`.part` file is pre-sized with `set_len` (all zeros) and the
fixture payload is `(i % 251)`, so the first non-zero byte
proves a segment has written. Pausing at that moment (~30–80 ms
after the segments spawn) is early enough that the run loop —
which notices the flag at most one 250 ms progress tick later —
still has ~1 MiB to go. The 50 ms fixture chunk delay is kept.
The test still asserts exactly what it asserted before (bytes >
0 and < full size at pause, byte-exact file after resume). No
engine source, no other test, and no UI file changed.

Status: fixed as described. Verified on Linux: the proxy_tests
suite 15/15 five consecutive runs; full engine suite 417/417;
`cargo check --tests` 0 warnings; `npm run build` passes. The
run this push triggers is the proof — its build-windows job must
go green so the user can see the Round 48 look.

## Round 50 — BUG-049 (real product bug: the downloads watcher never catches anything — `chrome.downloads.cancel` takes a bare id, and the code passed an object) — 2026-11-16

The user installed the run #25 MSI and reported on his own PC:
a real mp4 download in Chrome was NOT intercepted (it downloaded
in the browser), right-click → "Download with Fetchline" did
NOTHING visible, and nothing appeared in the app — while the
extension popup said "Connected". He then closed his browser and
asked for the testing to be done without him, so the popup's
last-handoff line (the decisive evidence) was never provided.

Reproduction on the VPS (headless Chrome 157, current extension,
freshly built host, fresh app.db, local fixture server): the
failure reproduced exactly, 3 runs in a row, slow fixture and
fast fixture alike — popup "Connected to the Sonic Manager
desktop app", catch toggle ON, pending "No links waiting", the
mp4 landed in the browser's download dir, no pending_links row,
no hand-off recorded. The worker was alive and the listener was
registered (`chrome.downloads.onCreated.hasListeners()` true, and
a probe listener added in the live worker DID fire with
`state=in_progress`), so the event was delivered — the handler
itself was exiting silently.

Root cause (proven with an instrumented background.js on the VPS
copy only, console trace via CDP): the handler ran cleanly
through every gate — `catch=true desktop=true excl=[]/[]`,
`ext=mp4 host=127.0.0.1`, "cancelling id=1" — and then
`chrome.downloads.cancel({ id: item.id })` threw:
"Error in invocation of downloads.cancel(integer downloadId,
optional function callback): No matching signature."
`chrome.downloads.cancel` takes the BARE download id —
`cancel(downloadId)` — not a query object (only `erase`/`search`
take query objects). The empty `catch {}` swallowed the throw,
so every browser download sailed past the watcher uncaught with
no trace anywhere. Never caught before because the Node fake in
`watcher.test.js` destructured an object
(`cancel: async ({ id }) => ...`), encoding the same wrong
assumption — the 9-case suite passed while every real download
failed. The popup handshake (Phase 2) never exercises cancel,
and the right-click path never calls it either, so nothing in CI
could see it. Note the user's right-click doing nothing is a
SEPARATE symptom this bug does not explain by itself (the menu
path never calls cancel): on his streaming site the download was
most likely a blob:/HLS flow the watcher ignores by design, or
the hand-off landed but was never drained visibly — still open,
needs his PC to confirm now that the watcher works.

Fix (product, `app/extension/background.js`, the watcher only):
`cancel({ id: item.id })` → `cancel(item.id)`, and a cancel
failure is now non-fatal for the hand-off — a small file can
complete before this async handler runs, and then there is
nothing left to cancel; the URL is handed over anyway so it
still lands in Sonic Manager instead of vanishing silently (the
browser's finished copy stays where it is; only a cancelled
download is erased from the browser's list). `erase({ id })` is
unchanged: erase really does take a query object.

Tests:
- `app/extension/tests/watcher.test.js`: the fake's cancel now
  enforces Chrome's real signature (throws "No matching
  signature." on a non-number — the old call fails 4 of the 11
  tests against this fake, proven by running the suite against
  the reverted call in /tmp), plus two new tests: cancel is
  called with the bare id, and a cancel failure (download
  already finished) still hands the URL over with nothing
  erased. Suite 11/11.
- `scripts/capture-test.py` gains Phase 4 (watcher): a second
  headless Chrome launch installs the extension, clicks a real
  download link for a SLOW fixture mp4 (512 KiB at ~80 KiB/s,
  in_progress ~6 s), and asserts a pending_links row
  (source=browser, status=pending) AND no file in the download
  dir. Runs after the drain phase, with `--skip-watcher` for
  local debugging. The module docstring now says four phases,
  and the honest-limits note now records that the watcher IS
  driven (only the human right-click still cannot be).
- Rebrand leftovers found while verifying (pre-existing since
  Round 40, NOT this bug): `validate_manifest.py` still demanded
  the name "Fetchline" and all 6 `popup.test.js` cases expected
  "Fetchline" strings, so `npm run test:extension` and the popup
  suite failed locally while CI stayed green (the workflow never
  runs them). Fixed in this round by updating the expectations
  to "Sonic Manager" (dummy input messages and the
  `FetchlinePopup` identifier untouched). Extension JS suite now
  38/38 (11 watcher + 4 background + 6 popup + 17 video).

Status: fixed as described. Verified on the VPS with the fixed
background.js: slow fixture INTERCEPTED (pending row
`http://127.0.0.1:63555/test.mp4`, source=browser,
status=pending; no file in the download dir; popup "Last sent
4:20:41 AM: Link queued — Fetchline will pick it up", "1 link
waiting in Sonic Manager") and fast fixture INTERCEPTED the same
way. Verified on Linux: full engine suite 417/417; `cargo
check --tests` 0 warnings; `npm run build` passes; extension JS
38/38. The run this push triggers is the proof — its
extension-capture-test job must go green with Phase 4 passing.
The progress window still has never been seen opening in the
running app (session 0 has no GUI), and the user's own-PC
confirmation is still outstanding.

CI verdict (run #26, 43804413258, head 266968a7, 2026-11-16) —
FULLY GREEN: build-windows success, smoke-test success,
extension-capture-test success (Phase 4 passed in CI: a real
browser download of the slow fixture mp4 was intercepted —
cancelled + erased, pending row landed). Installer 15,516,734
bytes (~14.8 MB); smoke artifact 375,151 bytes; capture artifact
3,796 bytes.

## Round 51 — Refresh download address (step 2 of the user's agreed sequence) — 2026-11-16

The user approved executing the sequence step by step (build →
test → next step): after the halo (Round 49) and the watcher fix
(Round 50), this round adds "Refresh download address" — a paused
or failed download can get a new link without losing progress.
The plan was presented to him on 2026-11-16 and he approved it
("نفذ وبعدين اختبر تمام نفذ إلي بعده وهكذا").

How it works (product, engine + UI):
- Right-click a paused or failed download → "Refresh download
  address" (the menu entry is disabled for any other status) →
  dialog shows the file, the progress so far and the current link,
  with a field for the new link.
- The engine (`DownloadManager::refresh_download_address`, new
  Tauri command `refresh_download_address(id, url, force)`)
  re-probes the NEW link FIRST through the same probe path
  `add_download` uses (saved site login for the new host, active
  proxy, the download's browser cookies only when the new link is
  on the same host — another host never receives this site's
  cookies; excluded sites refused before any request, BUG-011):
  - probe fails → error, and the old URL and all progress are kept
    untouched;
  - same file (size and ETag — the same rule `resume_download`
    uses) → the row is pointed at the new address (new store
    method `update_download_address`: URL, final URL, size, ETag,
    Last-Modified, range support; old error cleared) and the
    download starts again from the downloaded bytes;
  - different file → a `Mismatch` result (new size, ETag,
    filename) and NOTHING is changed; the dialog warns and only
    switches when the user confirms, and then the download restarts
    from zero — never stitching old bytes onto new content.
- Guards: only paused/failed rows can be refreshed (running or
  finished refused); the quota gate is checked before anything is
  switched, so a refresh never strands a switched row that cannot
  start; a bounded wait covers the gap between the row flipping to
  paused and the run task leaving the running map.
- The refreshed download auto-starts, so its progress window
  auto-opens like any resumed download (the main window's
  1 s poll shows the new state).

Tests (5 new, in `engine_tests.rs`, two mock servers serving the
identical file — the second one is the "new link"):
- same file switches and resumes from the downloaded bytes, then
  completes byte-identical;
- probe failure (404 server) keeps the old URL, the progress and
  the paused status;
- changed file (bumped version) reports a mismatch with the new
  size/ETag and changes nothing;
- confirmed mismatch switches and restarts from zero, then
  completes with the NEW file's bytes;
- running and complete downloads are refused.
- Caught during verification: the forced-restart path deleted the
  segments and reset the row but never recreated segments, so the
  restarted download never completed (the first run of the new
  test failed on the 30 s wait). Fixed by laying out fresh
  segments from the new probe (`initial_ranges`) after the reset
  — the same step `resume_download` takes when the remote file
  changes. All 5 tests pass after the fix.

Status: implemented as described. Verified on Linux: full engine
suite **422/422 pass, 0 failed** (417 + 5 new); `cargo check
--tests` 0 warnings; `npm run build` passes.

CI verdict (run #27, 43811435149, head 2d770731, 2026-11-16) —
FULLY GREEN: build-windows success, smoke-test success,
extension-capture-test success. Installer 15,561,674 bytes
(~14.8 MB); smoke artifact 375,982 bytes; capture artifact 3,799
bytes. The parent fetches the screenshot from the smoke artifact.
Honest limit: the smoke screenshot shows the main window only;
the dialog itself needs the user's own PC test to be seen in
action (like the progress window, it has never opened in the
running desktop app).

## Round 52 — Clipboard monitoring (step 3 of the user's agreed sequence) — 2026-11-16

The user approved the plan presented to him ("تمام، نفّذ مراقبة
الحافظة"): copying a download link shows a small offer instead
of downloading silently.

How it works (product, engine + shell + UI):
- The shell polls the system clipboard once a second (arboard,
  new dependency in `app/src-tauri/Cargo.toml`) and hands the
  text to the engine; the engine decides (new
  `app/engine/src/clipboard.rs` +
  `DownloadManager::check_clipboard_offer`):
  - the text must be a single link: trimmed, no whitespace (a
    multi-line paste is not one link), at most 2048 chars,
    scheme http/https/ftp, non-empty host, and the last path
    segment must carry an extension from the capture list (the
    same 27 extensions as `DEFAULT_CATCH_EXTENSIONS` in
    `app/extension/background.js` — the two lists must stay in
    sync);
  - the offer fires once per link: the engine remembers offered
    and dismissed URLs for the session (capped at 64) and ignores
    unchanged clipboard text;
  - excluded sites are never offered (BUG-011 rule); a link
    already in the downloads table (downloading or finished) is
    never offered;
  - the Settings toggle `clipboard_monitoring` (default on) gates
    everything — off means no offer, ever.
- The offer reaches the UI as a `clipboard-link-found` event (the
  capabilities file already grants `core:event:allow-listen` to
  the main window); App.tsx shows `ClipboardOfferDialog`
  (filename, URL, Download / Dismiss) and never stacks offers —
  a new event is ignored while one prompt is open. Download adds
  the link through the existing `add_download` path; Dismiss
  calls the new `dismiss_clipboard_offer` command, and the engine
  never offers that URL again.
- The clipboard is never stored: the engine keeps only a hash of
  the last seen text plus the offered/dismissed URL list for the
  session; a clipboard that cannot be opened disables monitoring
  for the session with one startup-log line, never a panic.

Tests (15 new, in `app/engine/tests/clipboard_tests.rs`,
modelled on the Round 51 refresh tests):
- detection: .mp4/.zip links offered with the right filename
  (percent-decoded); plain text, non-file URLs, multi-line
  pastes and over-long text refused; ftp + file extension
  offered; uppercase extension offered;
- dedupe: same text twice offers once; same URL re-copied after
  other text offers nothing; dismiss → never offered again;
  dismiss idempotent;
- exclusion: excluded site and subdomain refused; toggle off →
  no offer; already-downloaded URL refused.
- Settings default-on and round-trip through `update_settings`
  covered.

Status: implemented as described. Verified on Linux: full engine
suite **437/437 pass, 0 failed** (422 + 15 new); `cargo check
--tests` 0 warnings; `npm run build` passes.

CI verdict (run #28, 43815862108, head e3b3b853, 2026-11-16) —
FULLY GREEN: build-windows success, smoke-test success,
extension-capture-test success. Installer 15,570,771 bytes
(~14.8 MB); smoke artifact 375,182 bytes; capture artifact 3,804
bytes. The parent fetches the screenshot from the smoke artifact.
Honest limit: the smoke screenshot shows the main window only;
the offer dialog itself needs the user's own PC test to be seen
in action (like the progress window, it has never opened in the
running desktop app).

## Round 53 — Download history (step 4 of the user's agreed sequence) — 2026-11-16

The user approved the plan presented to him (tapped "تمام، نفّذ
سجل التحميلات"): a History section in the sidebar recording
every finished download, surviving removal from the main table,
searchable, with open-folder / re-download from history and a
clear-history option. This is the last step of the agreed
sequence (refresh address → clipboard monitoring → history).

How it works (product, engine half — the frontend worker builds
the sidebar section in parallel against the same contract):

- New `download_history` table (`replica/schema.sql` +
  `Store::migrate`, same pattern as `pending_links`):
  `id`, `download_id UNIQUE`, `filename`, `url`, `final_url`,
  `size_bytes`, `folder`, `category_id`, `completed_at`, plus
  an index on `completed_at DESC`. `download_id` is
  deliberately NOT a foreign key (no REFERENCES, no ON DELETE
  CASCADE): deleting a download must never delete its history
  row. `UNIQUE(download_id)` makes recording idempotent.
- The only recording hook is inside `Store::mark_complete` —
  the single completion call (`manager.rs:4200`) — so finished
  downloads ONLY: failed / cancelled / paused rows never land
  in history. The row is read with the already-locked
  connection (`get_download` would lock the same mutex and
  deadlock); `record_history` (`INSERT OR REPLACE`, folder =
  parent directory of `save_path`, `completed_at` = the row's
  own value or now) is also public for tests. Deleting a
  download (`set_status 'deleted'`) touches no history row —
  verified by test, no cleanup was added to any delete path.
- Store methods `list_history(query)` (empty query = all;
  otherwise filename OR url `LIKE`, with `%` and `_` escaped
  so they match literally; newest first),
  `get_history_entry(id)`, `clear_history()` (history only —
  the downloads table is untouched). New serializable
  `HistoryRow` (`id, download_id, filename, url, final_url,
  size_bytes, folder, category_id, completed_at`).
- Manager methods `list_history`, `clear_history`,
  `redownload_from_history(history_id)` (through the REAL
  `add_download` path — probe, excluded-sites refusal,
  duplicate handling — from `final_url` or `url`, same
  filename / folder / category; the history row is untouched,
  so the same entry can be re-downloaded again) and
  `open_history_folder(history_id)` (same platform opener
  `open_folder` uses — `crate::queue::open_path`; a folder
  deleted after the download finished is a clear error, not a
  silent no-op).
- New Tauri commands in `app/src-tauri/src/lib.rs`, registered
  in `generate_handler` (app commands run ungated, no
  capability change): `list_download_history(query)`,
  `clear_download_history()`,
  `redownload_from_history(id) -> (String, ProbeInfo)`,
  `open_history_folder(id)`.

Tests (8 new, in `app/engine/tests/history_tests.rs`):
- a real download driven to completion through the manager is
  recorded with the right filename / URL / final URL / size /
  folder (and the finished bytes match);
- recording the same download twice more leaves exactly one
  history row;
- deleting the download from the main table keeps its history
  row;
- search matches by filename and by URL, returns nothing for a
  non-match, and `%` / `_` match literally;
- clear history empties the table and leaves the download
  untouched;
- re-download from a history entry creates a new download from
  the stored URL and really completes (history then has 2
  rows); unknown history id errors for both re-download and
  open-folder; open-folder errors when the folder is gone.

Status: implemented as described. Verified on Linux: full engine
suite **445/445 pass, 0 failed** (437 + 8 new); `cargo check
--tests` 0 warnings. **CI run #29 FULLY GREEN 2026-11-16
~07:05 EET (run 43819643263, head 0928b120): build-windows ✅,
smoke-test ✅, extension-capture-test ✅; installer 15,616,779
bytes; smoke artifact 380,078 bytes.** Honest limits: the new Tauri commands are
uncompiled on this VM (gdk-3.0, same as every round); the
History sidebar section is the frontend worker's half and has
never been seen in the running desktop app.

## Round 54 — Real E2E GUI tests (tauri-driver + WebDriverIO) — 2026-11-16

**User order (2026-11-16 ~16:44 EET):** "try everything" — build a
comprehensive End-to-End test that really drives the program on a
real Windows server in GitHub Actions, and deliver no new version
until it passes and the logs + screenshots are inspected. (He asked
first whether Wine/Xvfb could do it — no: WebView2 does not run
under Wine, and apt is broken on this VM — then ordered the
tauri-driver path.)

**What was built (no product behaviour changed in this round):**
- `scripts/e2e/e2e-test.mjs` — plain-Node WebdriverIO `remote()`
  script (no wdio runner) driving the real debug binary through
  tauri-driver on 127.0.0.1:4444. 11 steps, one screenshot after
  EVERY step into `test-screenshots/` (+ results.json/results.md,
  startup.log, tauri-driver.log): 01 launch (window renders, no
  white screen), 02 stale-drain, 03 progress-window, 04 progress
  moves, 05 stop/pause, 06 resume, 07 add-url, 08 delete, 09
  sidebar, 10 options, 11 stop-all.
- `scripts/e2e/fixture-server.mjs` — local slow HTTP server
  (127.0.0.1, ephemeral port): 20 MiB at 32 KiB/s per connection,
  real Range support (206) + HEAD, so a download lasts ~160 s and
  is still running through every pause/resume/delete step (the
  BUG-048 timing trap: a fast fixture finishes before Pause lands).
  Locally verified: HEAD 200 with Content-Length 20971520 +
  Accept-Ranges; Range 0-99999 → 206, exactly 100000 bytes; 128 KiB
  in 3.0 s.
- `scripts/e2e/package.json` (+ lockfile) — webdriverio 10.2.0,
  own dependency tree so the app's package.json is untouched.
- `.github/workflows/e2e-windows.yml` — new workflow (push +
  workflow_dispatch, windows-latest, 30 min timeout): Node 20,
  Rust MSVC, npm ci, icons, host build, `tauri build --debug
  --no-bundle`, `cargo install tauri-driver --locked`,
  msedgedriver-tool (the Edge driver must EXACTLY match the
  installed Edge build or the session hangs forever at startup),
  `npm ci` in scripts/e2e, run the suite, upload
  `fetchline-e2e-screenshots` (if: always()).
- **The 4 bugs from the user's own-PC test of run #29 are the
  suite's designed reproductions, asserted as correct behaviour
  (a failure is a FINDING, never a reason to weaken the test):**
  (1) step 02 seeds a stale pending link through the REAL
  fetchline-host binary (Native Messaging frames, like
  capture-test.py) BEFORE launch and asserts it stays Waiting
  instead of auto-starting; (2) step 03 asserts a separate OS
  progress window exists for it (checked via user32 EnumWindows
  on the fetchline.exe process — authoritative — plus WebDriver
  window handles); (3) step 05 asserts toolbar Stop is usable
  with nothing selected, then pauses the selected row; (4) step 10
  asserts the Settings dialog renders all 9 tabs with real values
  (no "could not be loaded" alert; progress-window checkbox ON).
  Correct-behaviour note recorded in the test: the default
  download folder is EMPTY by default (`Settings::default` is
  `String::new()`), so an empty folder field is NORMAL and is not
  asserted.
- Selectors verified against the real .tsx source (aria-labels on
  the toolbar, #add-url, tab roles in SettingsDialog, sidebar nav
  label, StatusBadge text Waiting/Downloading/Paused/Finished/
  Failed, confirm dialog "Remove this download?" / "Remove").
  No data-testid attributes were needed; none added. No product
  file was changed.

**Local verification:** `node --check` on both .mjs files; fixture
server exercised with curl (see above); `npm run build` passes;
full engine suite 445/445 (ran locally before the push).

**Workflow-file route:** `.github/workflows/e2e-windows.yml`
cannot be pushed via the connector (no Workflows permission — the
403 is a permission limit, not a broken connection). It was written
locally and must be uploaded via the GitHub web UI signed in as
xsonicx1998, then the run it triggers is watched to completion and
its screenshots artifact is fetched and inspected. CI verdict:
PENDING the browser upload (record here when it lands).

**CI verdict — E2E run #1 FAILED before any test ran (run
43892347330, head 0ccfb124, 2026-11-16 ~17:09 EET, 6 min):**
step 9 "Build debug app (no bundle)" (`npx --yes
@tauri-apps/cli@2 build --debug --no-bundle`) failed after 2m46s;
steps 10–13 (tauri-driver, msedgedriver, E2E deps, Run E2E GUI
tests) were SKIPPED — the suite never ran, so steps 01–11 have NO
result and there are NO screenshots (the evidence artifact uploaded
empty under if-no-files-found: warn). Job logs are unreadable from
here (Actions API returns 403 "Must have admin rights" and is
rate-limited for the shared egress IP) — the log was NOT read.
- **Likely cause (harness/workflow setup, NOT a product bug):**
  the e2e workflow's host step builds fetchline-host.exe but never
  copies it into `app/src-tauri/binaries/` — build-windows.yml does
  (`Copy-Item target\release\fetchline-host.exe
  ..\src-tauri\binaries\fetchline-host-x86_64-pc-windows-msvc.exe`)
  because tauri.conf.json declares `bundle.externalBin:
  ["binaries/fetchline-host"]` and `tauri build` fails when that
  file is missing. The debug build is otherwise identical to the
  proven release build (same beforeBuildCommand, same shell code —
  no debug-gated code except the windows_subsystem attr).
  Confidence: high but NOT log-proven.
- **Fix prepared (harness-only, local file only):** the copy lines
  were added to the local `.github/workflows/e2e-windows.yml` host
  step, mirroring build-windows.yml exactly. The file must be
  re-uploaded via the GitHub web UI (xsonicx1998) — the connector
  cannot write workflow files — then the workflow re-run is the
  proof. No product code changed.
- **CI verdict — E2E run #2 FAILED (run 43894346292, head 9df2324,
  2026-11-16 ~17:23 EET, ~8 min):** the host-copy fix WORKED —
  step 9 "Build debug app (no bundle)" now SUCCEEDED (3m13s) and
  steps 1–12 all passed, but step 13 "Run E2E GUI tests" FAILED
  after ~1 s (`node scripts/e2e/e2e-test.mjs` exited almost
  instantly). The suite never ran: NO per-scenario results, NO
  screenshots, NO artifact at all (`fetchline-e2e-screenshots`
  missing — the upload step found nothing). Diagnosis: the script
  died before `main()` ran — `main()` first does
  `mkdirSync(test-screenshots)` + writes `results.json`, so a run
  that reached `main()` would have left an artifact. Most likely
  a load-time crash (module resolution in CI), but NOT proven:
  both imports resolve fine locally (`webdriverio` remote is a
  function; fixture-server imports fine; `node --check` passes;
  lockfile contains node_modules/webdriverio). The job log is the
  decisive evidence and it is UNREADABLE via the API (logs
  endpoint: 403 "Must have admin rights to Repository" —
  unauthenticated access cannot download job logs, confirmed
  3×; plus the shared-egress rate limit keeps 403ing on rotating
  IPs). Next step: read the run #2 job log via the signed-in
  browser (xsonicx1998 — user explicitly authorised this), fix
  the exact error, then run #3. Do NOT re-run blind. No product
  code changed; no product finding can be drawn from run #2.
- **Run #2 cause FOUND, log-proven (parent read the job log via
  the signed-in browser, 2026-11-16 ~17:57 EET):** step 13 died
  on `TypeError: Cannot read properties of undefined (reading
  'bind')` at
  `node_modules/webdriver/node_modules/undici/lib/interceptor/decompress.js:78`
  — `zstd: createZstdDecompress.bind(null, { params: {
  [constants.ZSTD_d_windowLogMax]: 23 } })` — on Node v20.20.2.
  Requiring `webdriver` loads its bundled undici, and undici's
  decompress interceptor needs Node's zlib zstd API
  (`createZstdDecompress`), which does not exist in Node 20 (it
  exists in Node 22+). The log also warned Node 20 is deprecated.
  Harness-dependency failure, NOT an app failure, NOT a test
  failure — the suite still never ran anywhere.
- **Fix (harness-only):** `.github/workflows/e2e-windows.yml`
  now sets up Node 22 (`node-version: "20"` → `"22"`, step
  renamed "Set up Node.js 22", comment records why). No
  `engines` field in scripts/e2e/package.json (nothing to
  change there); webdriverio/undici versions unchanged.
  Locally proven mechanism: the same undici line 78 exists in
  the local tree; on this VM's Node v24.22.1
  `createZstdDecompress` is a function and
  `import('webdriverio')` succeeds (`remote` is a function).
  No local Node 22 binary exists, so run #3 on CI is the
  verification of the exact Node 22 setup — not claimed as
  locally proven. `build-windows.yml` untouched (still Node 20,
  out of scope). The workflow file must be re-uploaded via the
  GitHub web UI (connector cannot write workflow files), then
  run #3 is the proof. No product code changed.
- **CI verdict — E2E run #3 FAILED (run 43902745102, head
  d3ee9a0, 2026-11-16 ~18:08 EET, ~7m46s):** steps 1–12 ALL
  passed, including "Set up Node.js 22" and "Build debug app"
  (2m33s) — but step 13 "Run E2E GUI tests" FAILED,
  16:07:15 → 16:08:26 UTC (1m11s). Progress vs run #2: the
  Node 22 fix WORKED — the undici import crash is gone; the
  script got past module loading and ran for 71 seconds instead
  of dying in ~1 s. **An artifact EXISTS this time:**
  `fetchline-e2e-screenshots`, created 16:08:27Z, **1,720
  bytes** — far too small for 11 screenshots, so it most
  likely holds only results.json/results.md (or a subset).
  Contents NOT inspected yet: the artifact download and the
  job log are parent browser jobs (API logs: 403 "Must have
  admin rights"; artifact download via API: 401/403; this
  coordinator has no browser route). Per-scenario results
  (steps 01–11) are UNKNOWN until the artifact is read — NO
  product finding can be drawn from run #3 yet. Next step:
  parent reads the job log + fetches the artifact via the
  signed-in browser (xsonicx1998), unzips, views the
  screenshots, reads results.json/results.md; then the failing
  step and any product findings are identified, the exact
  error is fixed, and run #4 follows. Do NOT re-run blind.
  No product code changed.

## Round 55 — Fix round for the 4 run #29 live defects (frozen progress, blank progress window, Add silent failure / 1 GB hang, Resume no-op) — 2026-11-16

  The run #29 VPS test (2026-11-16 evening) found the engine
  downloading but the UI frozen at 0%, the progress window blank
  white, Add URL sometimes failing silently, and a 1 GB add
  hanging for minutes. The investigation pinned the causes in
  code; this round fixes them. Approved by the user 22:34 EET
  ("نفذ الخطة") on the plan presented 21:48 EET.

  - **Frozen progress (defect 1):** the store row was checkpointed
    only every 2 s while a 10 MB download finishes in ~1.3 s, so
    the store never learned mid-download progress — and a stalled
    body stream waited forever, because the reqwest client had
    only a connect timeout. Fixed with a 1 s checkpoint interval
    plus a checkpoint on the first progress tick; a 60 s total
    timeout on the short-request client (`build_client`); a
    separate no-total-timeout segment client; a 30 s per-chunk
    stall watchdog on every segment request; and a 180 s run
    watchdog (suppressed during retry backoff) that fails a
    wedged run with a clear "stalled" message instead of leaving
    status `downloading` forever.
  - **Resume no-op (defect 4):** a wedged run left status
    `downloading`, and Resume refused it as "already running"
    while Pause set a flag no run ever read. Resume now recovers
    a stale `downloading` row (no run in the `running` map) and a
    genuinely running download still refuses. Resume/Pause
    errors are now sticky in the banner: the 1 s poll used to
    clear the error on every successful refresh, so a failure
    flashed for under a second and looked like a silent no-op.
  - **Add silent failure / 1 GB hang (defects 3a/3b):** the new
    row was hidden by the sidebar category filter; Escape /
    backdrop closed the dialog mid-submit so the probe error
    landed on an unmounted component; and the probe had no total
    timeout, so the 1 GB add stalled for minutes. Fixed: the
    table switches to "All downloads" and selects the new row;
    no close while busy; a 60 s submit timeout with a clear
    error; the submit promise outlives the dialog and reports
    late failures in the main banner.
  - **Blank progress window (defect 2):** root cause UNPROVEN —
    the code path is correct on inspection, so this round adds
    diagnostics only, no blind render change: creation/failure
    logging to startup.log, devtools on the progress window,
    a static loading shell in `index.html` (first paint never
    depends on JS), a plain-DOM fallback in `main.tsx` if React
    fails to mount within 2 s, and a poll-failure state in the
    progress window instead of the wrong "Download removed".
    The next live test (VPS, devtools + startup.log) must prove
    the cause before any render fix.
  - **Latent hazards fixed along the way:** `try_split` never
    splits an open-ended segment (its midpoint landed near 2^63
    and only the schema CHECKs refused it) and validates the
    midpoint before splitting; `Store::reconcile_stuck_downloads`
    resets rows left `downloading` by a previous run to `paused`
    (bytes kept), called once at startup.
  - **Also fixed:** the stale BUG-044 comment in `lib.rs`
    claiming no Tauri capability files ship —
    `capabilities/default.json` exists since Round 40.
  - **Tests:** 8 new engine tests (round55_tests.rs), suite
    453/453 on Linux; `npm run build` passes. The Tauri shell
    never compiles on this VM, so `lib.rs` is verified by reading
    only — CI is its first compile. The progress window's root
    cause and the whole round still need the VPS retest.

## Round 56 — Fix round for the r55 live defects (invoke path wedged after completion, both webviews blank white) — 2026-11-17

  The r55 VPS test (2026-11-17 ~01:57 EET) proved the engine
  completes downloads — the DB row was complete at 23:29:41 —
  but the UI stayed at "Downloading 0%" forever and both
  webviews ended up blank white. Server evidence (02:36 EET)
  overturned the first diagnosis: the run did not wedge and
  the store did not deadlock — `mark_complete` ran, and the
  checkpoint wrote. The failure is in the invoke path / tokio
  runtime after the completion write, and the wedge mechanism
  itself was NOT found in code. Step-0 live checks mostly
  failed to run (02:50 EET): the browser agent could not drive
  the RDP session, the main window is a blank white rectangle
  that does not respond to maximize, DevTools was never opened,
  and the app was deliberately NOT restarted (wedged state
  preserved, PID 5860). This round implements the code-alone
  parts of the §7 plan in `round56-investigation-rev.md`
  (approved by the user 02:48 EET, "نفذ خطة الإصلاح");
  the targeted runtime-wedge and webview-blank trigger fixes
  wait for the post-rebuild retest, where the §7.4 A/B test
  (plain index.html) is now the primary discriminator.

  - **Observability that does not depend on the runtime
    (§7.1):** `build_manager` now logs "reconciled 0
    download(s)" on Ok(0) instead of silence (the r55 log's
    silence was read as "the diagnostics never reached the
    log"); a plain `std::thread` heartbeat appends
    "heartbeat: startup" + "heartbeat: alive" to startup.log
    every 30 s, plus a "clean shutdown" line after `app.run()`
    returns (replacing "run returned ok" — a log ending
    without it means the process died, not exited); the
    store's single mutex is acquired only through
    `Store::lock_conn`, which logs a > 1 s wait with its call
    site (`#[track_caller]`); the store's diagnostics sink
    (`Store::set_log_path`, pointed at startup.log by the
    shell) receives probe durations and checkpoint failures
    instead of `let _ =`; `list_downloads` logs a sampled
    entry/exit trace (every 100th call, or any call slower
    than 1 s) and `add_download` logs every call; and
    `open_progress_window` logs BEFORE `build()` and logs the
    window's resolved `url()` after creation (the r55 log had
    no creation line at all).
  - **Independent run watchdog (§7.2):** a separate task per
    run watches a shared last-progress timestamp
    (`Running.last_progress_ms`, stamped by the supervisor
    loop whenever the total grows) and aborts the run after
    180 s of no progress — it keeps running even when the
    supervisor loop itself wedges, which is exactly what it
    watches for. It reads the CURRENT map entry, so a stale
    watchdog from an earlier run can never abort a newer run
    that is making progress; it is suppressed during retry
    backoff; the abort mirrors `cancel_download` (entry
    removed first — that is what unblocks Resume — then
    handle aborted, error status, snapshot), so a later
    Resume can restart the download. Timeout is a test knob
    (`set_run_watchdog_timeout_secs`, clamped >= 1 s,
    default 180 s); the existing 30 s per-chunk watchdog is
    untouched.
  - **Frontend in-flight guard + "backend not responding"
    (§7.3):** App.tsx's 1 s poll and ProgressWindow's 500 ms
    poll skip a tick while a refresh is still in flight, and
    the poll invoke itself is wrapped in `withTimeout`
    (5 s) so a hung invoke REJECTS instead of pending
    forever. After ~10 s with no poll response the window
    stops issuing invokes and shows a sticky "Backend not
    responding — restart the app" banner with Retry connection
    (resumes polling) and Reload window. A successful poll
    clears the dead state.
  - **Heartbeat-detection mechanism (§7.4):** each window's
    JS pings `webview_heartbeat` every 5 s with its label
    (`main`, `progress-<id>`, guarded so a hung backend never
    piles up pings); a runtime-independent monitor thread logs
    "webview <label> heartbeat lost" when pings stop for 30 s
    (and "resumed" on recovery), once per outage.
    `reload_webview_window` is the recovery action: the main
    window reloads in place (`window.location.reload()`), a
    progress window is closed and rebuilt from scratch (new
    renderer process) via the extracted `build_progress_window`
    helper. Devtools are now enabled on the MAIN window too
    (`tauri.conf.json`, verified against vendored Tauri 2.12.5
    + wry 0.57.2: the builder flag is not feature-gated and
    maps to WebView2 `AreDevToolsEnabled`, so F12 works in a
    release build — the `devtools` cargo feature only gates
    the `open_devtools()` API).
  - **Side bugs + probe timeout (§7.5):** the Add dialog's
    "Save to folder" placeholder no longer shows the made-up
    `C:\Users\you\Downloads` — it shows the configured
    default folder (new `defaultFolder` prop from Settings)
    or "Leave empty for Downloads"; the speed-cap input is
    `type="text"` + `inputMode="numeric"` with select-all on
    focus (a number input ignores Ctrl+A, so typing appended
    → "0512") and non-digits stripped on input; the probe
    total timeout is 60 s → 30 s (the dialog's own 60 s
    submit timeout is unchanged).
  - **Tests:** 4 new engine tests (round56_tests.rs): the
    independent watchdog aborts a stalled run in ~2 s with
    its message (not after the 30 s chunk watchdog); a slow
    4 MiB download at ~50 ms per chunk completes with the
    watchdog at 2 s; the timeout defaults to 180 s and clamps
    to >= 1 s; the probe duration is logged with its URL.
    Suite 457/457 on Linux (453 + 4 new); `npm run build`
    passes. The Tauri shell never compiles on this VM, so
    `lib.rs` is verified by reading only + `rustfmt` parse —
    the VPS build is its first compile. Deliberately NOT in
    this round: the targeted runtime-wedge fix and the
    webview-blank trigger fix (plain-index.html A/B test,
    fragment removal) — those wait for the step-0 / retest
    evidence.

## Round 57 — Fix round for the r56 live defects (7–8% stall with no watchdog line, both webviews blank white) — 2026-11-17

  User approval (2026-11-17 ~12:19 EET): "نفذ خطة الجولة 57" on
  the §9 plan in `~/workspace/round57-investigation.md`. The r56
  VPS retest (07:00 EET) failed all 4 checks in the UI: 20 MB
  stalled at 19.1/20 MB then 7%, 10 MB stalled at 8% on a fresh
  engine, Stop/Resume dead, 1 GB never started, progress window
  blank white — and the log showed "webview main heartbeat lost"
  twice with NO watchdog line at all. The investigation (12:03
  EET) corrected two things told to the user before: "ALL 4
  FAILED" is a UI verdict, not an engine verdict (the DB/disk
  check was never done, so the engine state is unknown), and
  "renderer death PROVEN in the log" is overstated (the heartbeat
  ping is an invoke, so it also stops on a runtime wedge; one hung
  ping silences all later pings). It also found the reload command
  used the wrong primitive (`eval("window.location.reload()")`
  needs live JS + IPC — exactly what's dead when a reload is
  needed) and that the 180 s watchdog is a tokio task on the same
  runtime it watches. This round implements §9.1–§9.4 only.

  - **§9.1 Renderer recovery (shell + frontend):** the reload
    command now uses the native `WebviewWindow::reload()` (Tauri
    2.12.5 → wry `ICoreWebView2::Reload()`), no JS, no tokio;
    progress windows still close + rebuild. New `reload_webview_now`
    (the shared primitive) + `maybe_auto_reload` (cooldown-gated).
    The heartbeat monitor thread now AUTO-RELOADS a window whose
    pings stop for 30 s — once per outage, max one auto-reload per
    5 min per window (`AUTO_RELOAD_COOLDOWN_MS`), every decision
    logged; the lock is released before the reload runs (the reload
    takes the same lock for its cooldown). `ProcessFailed` is
    registered directly on every webview via wry's
    `WebViewExtWindows::webview()` + `webview2-com 0.39`
    (`ProcessFailedEventHandler::create` +
    `ICoreWebView2::add_ProcessFailed`, mirroring wry's own
    `PermissionRequested` registration) — the binding was verified
    from the crate's published docs (docs.rs) + vendored wry
    0.57.2, NOT skipped. On the event it logs `webview <label>
    renderer process failed (<kind>)` and triggers the same
    cooldown-gated native reload. Heartbeat entries are now
    registered at window creation (main in `setup`, progress in
    `build_progress_window`), so a window whose JS never runs at
    all is watched from the start instead of never appearing in
    the map. The ping flaw is fixed in `main.tsx`: the ping races
    a 5 s timeout so `pingInFlight` always clears.
  - **§9.2 Runtime-wedge hardening (engine + shell):** the
    clipboard read runs on `spawn_blocking` behind an
    `Arc<StdMutex<Clipboard>>` with `try_lock` (a stuck RDP
    clipboard channel skips the tick instead of queuing) +
    `MissedTickBehavior::Delay`; the supervisor's `checkpoint()`
    store calls run on `spawn_blocking` (snapshot in memory
    first, then one blocking write); the Tauri runtime is built
    with 4 workers via `tauri::async_runtime::set` (mitigation
    only, commented as such — the Runtime is leaked
    deliberately, because `set` keeps only the handle and
    dropping it would shut the runtime down). `refreshAll` in
    App.tsx is complete: `refreshQueues`,
    `refreshGrabberProjects` and the new `refreshQueueFiles`
    all have the 5 s `withTimeout` + a shared in-flight guard.
    `list_downloads` logs its entry (sampled, same as the exit)
    so a hung call leaves an entry-without-exit trace.
  - **§9.2f Truly-independent watchdog (engine):** `Running.handle`
    is now `Arc<JoinHandle<()>>`, shared with a new
    `ThreadWatchEntry` per run in `DownloadManager::
    thread_watchdogs`. A plain `std::thread` per run watches the
    same last-progress timestamp (same 1–10 s cadence, same
    retry-backoff suppression) and on a stall logs FIRST
    ("thread watchdog aborted … (tokio watchdog did not fire)"),
    then aborts the run and marks the row error, mirroring the
    tokio watchdog — it never restarts the process. The thread
    exits when its registry entry is removed (run ended,
    cancelled, tokio watchdog fired) or replaced (newer run —
    `Arc::ptr_eq`, re-checked immediately before acting). No
    progress event is emitted: `emit_snapshot` is async and
    needs the very runtime that is wedged — the DB status is the
    source of truth.
  - **§9.3 Progress-window A/B test (shell + frontend):** the
    progress window now loads plain `index.html` with the id via
    `WebviewWindowBuilder::initialization_script`
    (`window.__PROGRESS_ID__`); `main.tsx` reads it, falling back
    to the `#progress=<id>` fragment. The fragment path stays
    working.
  - **§9.4 Diagnostics (engine):** the in-loop watchdog logs
    "in-loop watchdog fired …" explicitly before returning its
    error.
  - **Tests:** 2 new engine tests (round57_tests.rs): the thread
    watchdog aborts a stalled run while the tokio runtime is
    wedged (2-worker runtime, both workers blocked by sync
    sleeps; the log shows the THREAD watchdog fired and the
    tokio one did not); a slow 4 MiB download completes with no
    watchdog line at all. Suite 459/459 on Linux (457 + 2 new);
    `npm run build` passes. The Tauri shell never compiles on
    this VM, so `lib.rs` is verified by reading only + `rustfmt`
    parse — the VPS build is its first compile. Deliberately NOT
    in this round: the §9.5 engine-stall fix (it depends on the
    live DB/disk check results, not yet available when this was
    written — do not write it blind) and auto-restart (a pending
    user decision — the watchdogs log + abort only). No pushes,
    no GitHub/CI, no workflow files touched.

## Round 58 — Fix round for the r57 live defects (invoke wedge persists, progress window still blank, Start dead / 1 GB add 60 s timeout, "Backend did not respond" banner) — 2026-11-17

Investigation (`~/workspace/round58-investigation.md`, read-only): the diagnosis moves
from a tokio-runtime wedge to a **main-thread wedge in WebView2 window creation**.
`WebviewWindowBuilder::build()` blocks the calling worker in `rx.recv()` until the
main thread finishes; on the main thread wry's `InnerWebView::new` creates the
container HWND, then calls `create_environment` and `create_controller`, each ending
in `webview2_com::wait_with_pump(rx)` with **no timeout**. Tauri shares the WebView2
environment only for new-window requests, so every progress window pays a full
environment creation. If the browser process never starts a controller, the main
thread never returns: no invoke is dispatched or answered, the window cannot close,
and the worker is lost. One mechanism fits all six retest observations (banner =
IPC outage not backend outage; Start dead vs Save works = timing of an
intermittent wedge; 1 GB = invoke never reached the probe, which is bounded 30 s
and size-independent; blank window = empty container HWND, title updates via
native calls without a renderer; unclosable window = event loop stuck).
**Correction to the Round 57 report:** "renderer death PROVEN in the log" was
overstated (heartbeat ping is an invoke — it also stops on a runtime wedge).
**Correction to this round's own investigation (§5.3/§9.4):** the probe-on-typing
invoke (`probeBusy` / `getDownloadInfo`) does NOT exist in this codebase — grep
finds zero occurrences. The Add dialog has no probe-on-typing effect; both buttons
share the `busy` flag and the same `add_download` invoke (differing only by
`queue: startNow ? null : 'queue-main'`), so the Start-vs-Save asymmetry has no
dialog-level explanation — it is the intermittent wedge's timing. The §8 log
checks (`add_download` entry/exit) decide the 1 GB story instead.
Fix (§9.1–§9.5, `lib.rs` + `AddDownloadDialog.tsx`): progress windows reuse the
main window's WebView2 environment via `WebviewWindowBuilder::with_environment`
(verified public API, tauri 2.12.5; env from main window `with_webview` →
`PlatformWebview::environment()`, 5 s bounded `recv_timeout`, fallback logged);
auto-reload guarded — no rebuild of a never-created window (`created_ok` flag),
cap 2 attempts per outage then `auto-reload abandoned`, rebuild on a detached
`std::thread` so a hung rebuild cannot silence the monitor; `window.url()`
getter deleted from `build_progress_window` (builder's URL logged instead, plus
`build()` elapsed time with a >5 s slow-creation warning); main-thread watchdog
(`std::thread`, `is_visible()` round-trip, logs `main thread stalled for 10s`
once per outage + resume); stale comment fixed (dialog 60 s is deliberately
longer than the engine's 30 s probe bound, not "matching" it). Deliberately NOT
in this round: engine-stall fix (engine never stalled), auto-restart (pending
user decision), probe change, worker increase. `npm run build` passes; `cargo
fmt` clean in the edited regions. Tauri shell never compiles on this VM — the
VPS build is the first compile. Decisive log check (§8.1): `progress window
progress-<id> created: <url>` present = creation completed (renderer branch),
absent = main-thread wedge confirmed.

**E0277 fix (2026-11-17 ~15:46 EET, same round):** the first VPS build failed
with exactly one error — `NonNull<c_void>` cannot be sent between threads
safely (lib.rs:614): `main_webview_environment` sent the environment through
`mpsc::Sender<ICoreWebView2Environment>`, but `with_webview` requires a `Send`
closure. Fix: a Windows-only newtype `SendableWebView2Environment` with
`unsafe impl Send`; the channel now carries the wrapper. Safety (documented
in the code, no agility claimed — Microsoft's WebView2 threading doc does NOT
state the environment is agile): move-only use — the worker never calls a
method on it, only stores it in the builder; the only COM call
(`CreateCoreWebView2Controller`) runs on the main thread; clone/drop is only
`AddRef`/`Release` (thread-safe by COM rules). This is the same transfer
Tauri itself performs: `WebviewAttributes` (holding this exact type) is
`unsafe impl Send` (tauri-runtime-2.12.2/src/webview.rs:434). 5 s
`recv_timeout` unchanged. `npm run build` passes; `cargo fmt` clean in the
edited region. VPS build is the real compile check.

## Known limitations, not bugs (round-24 additions)

- The Tauri shell never compiles or runs on this VM, so a
  bug like BUG-029 — a panic that only happens when the
  shell actually runs — can only be caught by launching the
  built app on real Windows. BUG-027/028/030 were compile
  errors CI caught; BUG-029 was a runtime error only a
  launch could catch, and the startup.log diagnostics are
  what caught it.
- The fix is a spawn-site change only; no download behaviour
  changed. The next proof is the user's `startup.log` from
  the fixed build ending with "setup ok" and "frontend
  reached backend: get_settings ok".

**Round 58 retest (2026-11-17 16:48 EET, build a26a1027, exe 17,873,920 bytes):**
BUILD OK (3m 59s). (a) Progress window ×10 FAIL — still blank white, will not
close (X ×2, Task View X ×2), red "Backend did not respond" banner after each
creation. (b) 20 MB via Start PASS on a healthy instance (Start never visibly
disabled). (c) Slow Stop/Resume FAIL — froze at exactly 2,097,152 bytes
(2 MiB) on disk, UI "Downloading 0%", Resume did not move it. (d) 1 GB PASS
by criterion — probe error in ~15 s, no 60 s dialog timeout. Log: progress
heartbeat STARTED then LOST (33s); main heartbeat lost (34s); main
auto-reload done; progress auto-reload skipped (creation never completed).
NO "build took ... slow creation" or "main thread stalled" lines — the
main-thread wedge hypothesis is now weakened. NEW: slow-a.mp4 listed as
Finished with only 2 MiB on disk (false Finished). Fixture server listens on
an EPHEMERAL port (this run: 55140), not fixed 18099.

- **Round 59 investigation (2026-11-17 ~17:02 EET, report `~/workspace/round59-investigation.md`):** the r58 fix removed only 1 of 2 unbounded waits — `create_controller` also ends in an unbounded `wait_with_pump` (wry-0.57.2 mod.rs:382-436), hit on every progress-window creation. "Heartbeat started" + `created_ok` false = hung `build()` signature (started is a Rust-side entry, not a JS ping). Watchdog verified a genuine discriminator (tao buffers events during a wedge, so a stall line MUST fire within 10 s). No engine path can mark a partial row complete (single `mark_complete` call site behind a `total == size` guard). 1 GB probe error = transport failure, not a timeout. Fix plan §9 gated on live checks (A1 persistent window; A2 surface FAILED + close leaked window; A3/A4 ProcessFailed gap + decouple windows).
- **Round 59 live checks (2026-11-17 17:44 EET, task 37b76396, 44 min, read-only):** full-log grep (1,322 lines): "created:" = 0, "main thread stalled" = 0, "PANIC" = 0, "build took" = 0, no "FAILED to create"; heartbeat lost 7 (main 5, progress 2), resumed 0; auto-reload 8. **Wedge RULED OUT — all four §8 branches ruled out; §9.2–§9.4 suspended.** **CORRECTIONS: (1) slow-a.mp4 did NOT freeze at 2 MiB and was NOT falsely Finished — DB + disk agree: complete, 20,971,520/20,971,520, 4 segments done, ran 14:39:51→14:42:30 at exactly the fixture rate (128.8 KiB/s); the 2,097,152 reading was a misread by the retest task. Stop was pressed ~14:44, after completion. (2) 1 GB probe error = REAL DNS failure: curl "Could not resolve host: speed.hetzner.de" in ~18 ms — not a code bug. (3) Last add_download pair exited 150 ms (err) — no ~15 s vs 18 ms gap; "~15 s" was a loose observation bound.** Mechanism now: build() returns neither Ok nor Err while the main thread stays healthy; leading candidate by elimination = the CreateWindow message never reached the handler (unproven; no silent-drop path in vendored code). Unclosable window: wedge explanation gone (RDP miss or unregistered window id remain).
- **Round 59 §11 follow-up (2026-11-17 18:06 EET):** all four branches ruled out; revised plan §11.4 = instrument first, fix second (6 logging-only changes + one instrumented VPS run). **Incident: the follow-up agent overwrote round59-investigation.md while appending §11 and rebuilt it condensed (532 lines vs 922 original); §8/§9.1/§9.5/§9.6/§9.7/§10 verbatim, §0–§7/§9.2–§9.4 faithful reconstructions; §11 intact, backed up to /tmp/section11-backup.md.**
- **Round 60 (approved 2026-11-17 18:10 EET):** §11.4 items 1–3 implemented, logging only — monitor/watchdog started lines, watchdog tick every 60 s, build() entered with thread id, thread id on created:/FAILED lines, watchdog probe-failed line. Engine suite 459/459; npm build passes. No behavior change; §9.5 engine latent fixes NOT in this round. VPS build + instrumented test pending.

- **Round 60 instrumented test (2026-11-17 18:26–19:24 EET, task 106d3b65, 58 min):** BUILD SUCCESS (exe 17,903,104 bytes, 4:43:57 PM; host 1m 40s, Tauri 3m 45s). Full-log greps (1,776 lines): "created:" 0, "FAILED" 0, "main thread stalled" 0, "renderer process failed" 0, "heartbeat monitor started" 3, "main thread watchdog started" 3, "watchdog tick" 35, "build() entered on thread" 3, "watchdog probe failed" 0, "heartbeat lost" 13, "heartbeat resumed" 0. **Mechanism (b) CONFIRMED: the CreateWindow message is lost in transit (watchdog runs, ticks continue, build() entered and never returned).** Blank window reproduced; X and system-menu Close both fail; only killing the app closes it (RDP miss ruled out; Tauri-map-missing stands). Stop pressed in-flight (~5:10:30 PM) did NOT stop the download (completed 17:11:09, full bytes) — invoke channel was dead (banner), so the click never reached the engine; not a separate Stop defect. Resume untestable. Cloudflare 100MB add NOT confirmed (session expired). Fix plan (§11.4 outcome 3): persistent progress window built once at startup (preferred), or detached-thread build, or webview-less window + async attach — presented to user for approval.

- **§13 CORRECTION (2026-11-17 19:26–19:30 EET, second-AI review + parent verification in vendored code): the Round 60 verdict "Mechanism (b) CONFIRMED — CreateWindow message lost" is WRONG.** Sync `#[tauri::command]` fns run INLINE on the main thread (tauri-macros-2.7.4 wrapper.rs `body_blocking` calls the fn directly; `body_async` spawns) and `send_user_message` handles inline on the main thread (tauri-runtime-wry-2.12.3 lib.rs:263) — no CreateWindow message exists to be lost. `build()` runs inside the main webview's WebView2 callback and blocks in `create_controller`'s `wait_with_pump`; the nested pump answers the watchdog, hence no stall line. Both `open_progress_window` (lib.rs:810) and `reload_webview_window` (:904) are sync fn. UI bugs verified: `window.close()` ×5 in ProgressWindow.tsx (no `getCurrentWindow` anywhere); capabilities/default.json grants only `["main"]`. **Correction to the second AI:** `main_webview_environment` CAN succeed on the main thread (log has "sharing" lines) — do NOT remove sharing on that premise.
- **Round 61 (approved 2026-11-17 19:33 EET):** async fix + UI bugs. `open_progress_window` and `reload_webview_window` → `async fn` (keyword only, bodies unchanged); setup logs `main thread id: {:?}` (§13 confirm line). ProgressWindow.tsx: all 5 `window.close()` → `getCurrentWindow().close()` (import from '@tauri-apps/api/window'). capabilities/default.json: windows `["main", "progress-*"]` + `core:window:allow-close`. Neither command is called from Rust (only `generate_handler!` + JS `invoke`), so no call-site adaptation needed. Engine suite 459/459; npm build passes. App crate cannot compile on this VM (no gdk-3.0) — VPS build is the real compile check. Push pending → VPS build + retest.

- **Round 61 build + retest (2026-11-17 19:43–21:14 EET, task 4360d730, 1h 30m): BUILD SUCCESS (exe 18,780,672 bytes, 6:16 PM; host 1,192,960 bytes; head 6358fe54 verified; log C:\fetchline-r55\build-r61.log). (a) Progress window PASS — rendered real content on 5+ openings, never blank white; closed via title-bar X, in-page Close button, and grey X glyph. Caveat: twice a close click needed a repeat (first click only focused the window). (b) Fresh download PASS — fixture slow-a.mp4, live progress 5%→100%, finished 20.0 MB on disk. (c) In-flight Stop/Resume PASS — Stop at ~16% (row Paused at 35%, bytes stopped), Resume 35%→100% full bytes. (d) Log checks PASS — "created:" 5, "FAILED" 0, "main thread stalled" 0, "renderer process failed" 0, "watchdog tick" 108, "build() entered" 8, "main thread id:" 1 = ThreadId(1); Round 61 build() lines run on ThreadId(3/5/4) — all DIFFER from main, as required. (The 3 ThreadId(1) build() lines are pre-18:16 Round 60 instance.) "heartbeat lost" 18, all progress-window pings (~34s), "heartbeat resumed" 1 — no main stalls. (e) Large download FAIL — Cloudflare 100MB.bin 404 (file gone), __down 403; OVH 100Mb.dat reached 75 MB then "Connection lost — retrying" then Failed ("Network error: error sending request"), while curl HEAD to the same URL returns 200 + Content-Length 104857600; Tele2 100MB.zip address check failed 3× ("error sending request"). NEW FINDING: Toolbar Resume did not restart the failed download and the right-click menu offers no Resume/Retry on a failed row. Extra feature pass (Settings/History/etc.) NOT run — stopped per instruction because (e) failed. App left RUNNING (PID 6100), 14 downloads (13 finished, 1 failed). RDP session expired once ~18:45 UTC; user reconnected; 32 min remained at end.**

- **Round 62 (approved 2026-11-17 23:07 EET — "نفذ التصميم"):** settled redesign implemented, local only. ProgressWindow.tsx rewritten (frameless 660×480, live "<percent>% - <filename>" title bar, Download status + merged Options tab, IDM parts table with derived vocabulary — Done cyan / "No" red bold / "Disconnect" pink / "Send GET..." amber / "Receiving data..." purple — speed graph + Hide details + completion tab + scan UI removed; shutdown = 30 s countdown with Cancel, close-after exits the app). New shared TitleBar.tsx (drag region = icon+title only, double-click maximizes), MenuBar.tsx (all 6 menus, no Registration / Tell a Friend), ToolsStrip.tsx (Queue, Scheduler, History, Video Grabber, Site Grabber, Refresh Address, Clipboard ON/OFF toggle). Main window: slim toolbar (gradient Add URL, Resume, Pause, Stop, Delete, search), 5-column table with inline gradient bars, status bar with live counts + drive free space, NO settings gear. New Rust commands: set/get_download_completion_options, exit_app, shutdown_computer, get_free_disk_space (GetDiskFreeSpaceExW FFI); `set_progress_window_compact` deleted; main window `decorations: false`; capabilities add minimize/maximize/toggle-maximize/unmaximize/is-maximized/start-dragging/set-size/set-focus. Round 61 constraints kept (async open/reload, getCurrentWindow().close()). New icon: chosen logo → `icons/logo.png` (new generate-icons.py source) + `app/public/icon.png`; icon set regenerated via `tauri icon` (128x128 verified by eye). Engine 459/459; npm build passes; cargo fmt 40 pre-existing hunks, none in edited regions. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; VPS build + live test pending (resize-from-edges, rounded corners, title-bar behavior unverified until then).

- **Round 63 (user feedback 2026-11-18 ~00:32 EET on the real Round 62 app, implemented local only):** (1) Progress-window tabs moved to the TOP under the title bar (pinned above the scrollable content). (2) Footer Cancel button DELETED — Pause only while downloading; title-bar X still closes. (3) On completion the footer shows Open file + Open folder (existing `open_file`/`open_folder` commands, no new Rust). (4) Neon-waves background REMOVED — `background.css` import deleted, `background.css` + `sonic-neon-waves.svg` deleted; flat dark #0B0F24 in both windows. (5) Theme scrollbars (index.css) — purple thumb, dark track, cyan on hover, thin, everywhere scrollable. (6) History REMOVED from the TOOLS strip — stays in the sidebar. Picture-matching (agreed designs sent 00:38 EET): parts header "Parts (N connections)" + "X/N completed" counter; Downloaded stat "downloaded / size"; folder path moved into the footer row. Engine 459/459; npm build passes. No Rust edited. NOT pushed; VPS build + retest pending (old RDP test task tests the pre-fix build).

- **Round 63b (user decision 2026-11-18 ~01:29 EET — "هنرجع cancel تآني", reverses the Round 63 removal):** Cancel button RESTORED in the progress window footer, exactly as Round 62 — `variant="danger"`, `onClick={handleCancel}` (same `cancelDownload` call), shown while `downloading` or `paused`, next to Pause/Resume, before Close. It does NOT show on completion (footer shows Open file / Open folder instead). Retry-box "Cancel download" ghost button unchanged. Everything else from Round 63 unchanged (tabs on top, flat dark background, theme scrollbars, no History in the TOOLS strip). Engine suite 459/459; npm build passes. No Rust edited. NOT pushed; VPS build + retest pending.

- **Round 63 build + retest (2026-11-18 00:46–01:35 EET, task c4566d1c, 48 min):** BUILD SUCCESS (exe 18,332,672 bytes; head 92a88a0 verified; log C:\fetchline-r55\build-r63.log; host 1m 54s, Tauri 4m 10s). MAIN WINDOW PASS — new logo, no History in the TOOLS strip, flat dark background, purple scrollbars. PROGRESS WINDOW NOT VERIFIED — it never opened despite double-clicking the row, right-clicking it, clicking the progress bar and the status-bar "1 downloading" text, and trying the menus (none opened a dropdown) and Task View. LARGE DOWNLOAD FAIL — OVH 100Mb.dat resumed from the failed row, reached 94%, then stalled ~12 min (rate "—"), Pause had no effect; Add URL "Start download" did nothing (dialog stayed open; Cancel and Save for later also dead; only the dialog X closed it); then "+ Add URL" stopped opening the dialog at all and the window X would not close the app. App left RUNNING hung at 94%. The Round 61 retest PASSED all of this (progress window 5+ openings, Stop/Resume, fixture download) — the hang is a Round 62/63 regression, frontend/main-thread/invoke-channel suspect, engine exonerated before. Next: Round 64 investigation, then re-test on the Cancel-restored (63b) build.

- **Round 63b build + retest (2026-11-18 01:56–02:34 EET, task 8529c233, 37 min):** BUILD SUCCESS (head 3d4c4a9 verified; old hung app killed with Stop-Process -Force, old exe deleted, app data untouched; host 6,310,912 bytes; exe 18,332,072 bytes; log C:\fetchline-r55\build-r63b.log). MAIN WINDOW PASS (logo, menu bar, TOOLS strip no History, flat dark, purple scrollbars). PROGRESS WINDOW PASS — opened with real content on the fixture download: tabs on top, Pause + Cancel in footer (Cancel is back), parts table 4 rows, stats + path in footer. FIXTURE DOWNLOAD COMPLETED (slow-a.mp4 20.0 MB, 10%→100%, Pause at 74% → Resume → 100%, 4/4 parts Done, Finished 15→16; app stayed responsive, never stalled). USER LINKS NOT TESTED — both testfile.org links (1.3GBiconpng, files-5GB) returned 403 Forbidden in the app AND via curl from the VPS even with a browser User-Agent (Cloudflare block) — site-side, not an app failure. Notes: Pause/Resume each needed a second click (likely RDP coordinate scaling, not app); progress-window Speed/Time-left intermittently showed "—". App left RUNNING (16 finished, 0 downloading). **The Round 63 hang did NOT reproduce on this build.** Large-download test still needs links from a host that does not block the VPS.

- **Round 64 fixes (user feedback 2026-11-18 ~02:37 EET, local only):** (1) Speed / Time-left flicker — cause: the engine reports an instantaneous per-tick speed (250 ms ticks, manager.rs), so any tick with no new bytes carries speed 0 and eta null; the checkpoint (1 s) writes those into the DB row and each progress event carries them, so the 500 ms poll flipped the stats row between a value and '—'. Fix (frontend only, no engine change): while downloading, ProgressWindow.tsx and DownloadTable.tsx hold the last positive speed / ETA and keep showing them for up to 5 s; only a sustained idle shows '0 B/s' (speed) / '—' (ETA). The hold clears when the status leaves 'downloading', so a later run never shows a stale value. `formatSpeed` gained an optional zero label (default '—', existing call sites unchanged). (2) Footer Close button REMOVED from the progress window (Round 63b Cancel stays; Open file / Open folder on completion stay); the title-bar X is the way to close the window — the empty-state Close (shown when the download is gone) was kept. Engine suite 459/459; npm build passes. No Rust edited. NOT pushed; VPS build + retest pending (plus the user's full-app + extension test pass).

- **Round 65 — system tray + IDM right-click menu (2026-11-18, local only):** (1) System tray, like IDM: the tray icon (the app logo via `default_window_icon`) lives by the clock while the app runs. Minimize (main window AND progress windows) and the main-window X hide the window to the tray (`window.hide()`); the progress-window X still closes just that window and the download keeps running. Left-click the tray icon shows the main window again; right-click opens a menu with Show / Exit (Exit quits through the existing `exit_app` path). Rust: `TrayIconBuilder` in `build_system_tray()` (built best-effort in setup — a failed tray never blocks startup), `tray-icon` feature enabled in Cargo.toml, `core:window:allow-hide` / `allow-show` added to capabilities, `.on_window_event` intercepts main-window CloseRequested → hide + prevent_close; `open_progress_window` now calls `show()` before `set_focus()` on the already-open branch. (2) IDM-like right-click menu on download rows: Open (complete only), Open with... (new `open_file_with` command — `rundll32 shell32.dll,OpenAs_RunDLL`, file must exist on disk), Open folder, Redownload (cancel if running, delete row + file, re-add the same URL/folder/category/queue), Resume Download (paused/error only — this also fixes the Round 61 finding that failed rows had no Resume/Retry), Stop Download (downloading only, pauses), Refresh download address (paused/error only), Remove, Move to queue (inline click-to-expand section, only when queues exist), Delete from queue (only when the download is in a queue), Properties (new `DownloadPropertiesDialog` — filename, URL, status + %, size, downloaded, speed, save path, category, queue, added, completed). "Open progress window" and "Move to category" kept. Every item is enabled/disabled by the row's state, like IDM (user rule 2026-11-18 ~02:44 EET). (3) Refresh Address REMOVED from the TOOLS strip (ToolsStrip.tsx prop + button + App.tsx call site) — now only in the right-click menu. Round 64 fixes kept (speed/ETA hold, no footer Close). Caught during the fmt check: inserting `build_system_tray` had stranded the `#[cfg_attr(mobile, tauri::mobile_entry_point)]` attribute on it instead of `run()` — moved back to `run()` (desktop no-op, but wrong on mobile). Engine suite 459/459; npm build passes (4.73 s after a `MutableRefObject` type fix on the menu ref). No Rust compile on this VM (no gdk-3.0) — VPS build is the first real compile. NOT pushed; VPS build + retest pending (plus the user's full-app + extension test pass).

- **Round 66 — "Download complete" dialog + notification sound (2026-11-18, local only):** (1) On completion the progress window closes and a separate "Download complete" dialog opens (approved mockup `workspace/download-complete-preview.html`): cyan check, FILENAME heading (not "Download complete" twice), "Downloaded X MB (N Bytes)", Address, "The file saved as", Open (purple→pink gradient) / Open with... / Open folder / Close, "Don't show this dialog again" checkbox persisted in settings (`show_download_complete_dialog`, default on; re-enable in Settings, next to the progress-window switch). New `DownloadCompleteDialog.tsx`; new async `open_complete_dialog` + `build_complete_dialog_window` in lib.rs (label `complete-<id>`, frameless 560x440 fixed, `__COMPLETE_ID__` init script, shared WebView2 env, no heartbeat/auto-reload); `main.tsx` renders it and skips the heartbeat for it; "complete-*" added to capabilities. Trigger is the main window's 1 s poll (flip into 'complete', deduped, 30 s `completed_at` recency for first-seen rows); close-after / shut-down-after take precedence (no dialog); the progress window closes itself when the dialog is enabled. (2) Notification sound: no binary asset — the chime is synthesized at runtime in Rust (`builtin_complete_wav()`: A5 880 Hz 0.22 s + D6 1174.66 Hz 0.32 s, 22050 Hz 16-bit mono, attack + decay), written to the app data dir as `complete.wav` and played via the PowerShell SoundPlayer; a user-configured `download_complete` sound wins (engine already played it), Windows default Asterisk sound is the last resort. TitleBar gained `showMinimize`/`showMaximize` props (dialog hides both — minimize-to-tray would strand it). Engine suite 459/459; npm build passes (4.89 s); my Rust regions fmt-clean. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; VPS build + retest pending (plus the user's full-app + extension test pass).

- **Round 67 — Add URL dialog redesign + extension capture opens the dialog (2026-11-18, local only):** (1) Dialog redesigned to the approved mockup (`workspace/add-url-preview.html`): dark title bar with the real Sonic logo + "Add URL" + X, purple→pink gradient divider, IDM field order (URL, Category + "+", Save As as one editable full path, "Remember this path" + cyan file size on the same row, read-only folder-path field, Description), Download Later / Start Download / Cancel with the default-action button as the gradient primary (with the default setting that is Start Download, as the mockup shows). Debounced probe prefills the filename and the cyan size; picking a category autofills the folder from the category's default folder; "..." opens the native folder picker (`@tauri-apps/plugin-dialog` 2.8.4); "Remember this path" persists the folder as the category's default folder. Round 55 submit machinery kept (60 s timeout race, late-outcome handler). (2) Description is new end to end (`description TEXT` + migration + `DownloadRow.description` + `set_download_description`, written after the add, non-fatal). (3) Extension captures now open this dialog instead of auto-starting: the drain skips `browser`/`video` rows (only `download_all` drains); the app polls `list_pending_links` and pops one capture at a time into the dialog (never stacked), with the row's cookies riding the probe and the add via new `confirm_pending_link` (re-checks integration + excluded sites, applies the speed cap before starting, double confirm is an error). New Tauri commands `list_pending_links`, `resolve_pending_link`, `probe_url`, `set_download_description`, `confirm_pending_link`; `"dialog:allow-open"` in capabilities. Clipboard flow, Download All flow and video-panel prefill unchanged. Engine suite 464/464 (5 new tests); npm build passes. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; VPS build + retest pending (plus the user's full-app + extension test pass).

- **Round 68 — Download failed dialog + sounds + cap 32 + extension v2 + taskbar progress (2026-11-18, local only):** (1) On failure a separate "Download failed" dialog opens over the progress window (approved mockup `workspace/download-failed-preview.html`, IDM-style): yellow warning triangle + "An error occurred during download. Please try again." + "Details:" with the ACTUAL engine `error_message` for that case (connection lost, server not found / DNS, 404, 403, no disk space, timeout — never a fixed string) + one OK button; OK/X closes it and returns to the progress window. New `DownloadErrorDialog.tsx`; new async `open_error_dialog` + `build_error_dialog_window` in lib.rs (label `error-<id>`, frameless 520x360, `__ERROR_ID__` init script, shared WebView2 env, no heartbeat/auto-reload); `main.tsx` renders it and skips the heartbeat for it; "error-*" added to capabilities. Trigger is the main window's 1 s poll (flip into 'error'/'failed' from a previously-seen downloading/paused/queued status, deduped per id — the completion dialog is untouched). Progress-window failed state (mockup `workspace/download-failed-progress-state.html`): Error badge (pink-red), red error line under the URL, parts Disconnect/No, footer Resume (primary gradient, continues from where it stopped) + Cancel (Cancel now also shows for 'error' status). (2) Sounds: completion sound is now the real Windows "tada", BUNDLED as `app/src-tauri/assets/tada.wav` (285,228 bytes, `tauri.conf.json` resources `"assets/tada.wav": "tada.wav"` — the GitHub connector cannot push binaries, so it goes up via the browser route at push time) and played on completion; Round 66's synthesized chime (A5 880 Hz → D6 1174.66 Hz) is the fallback if the asset is missing, and a user-configured WAV still wins. Error alert is synthesized in Rust exactly like the chime (`builtin_error_wav()`: 740 Hz 0.16 s + 494 Hz 0.32 s, 22050 Hz 16-bit mono, attack + decay) and plays once when the error dialog mounts, via the shared `play_notification_sound()` helper (master switch → user file → bundled asset → synthesized built-in → Windows Asterisk); new `play_error_sound` command. (3) NEW setting `sounds_enabled` (default on, `sounds_enabled INTEGER NOT NULL DEFAULT 1` in schema.sql + migration + Default + get/update_settings; engine `emit_sound` fails open on read error): a master "Play notification sounds" checkbox at the top of the Sounds tab — when off, neither sound plays. (4) Segment cap 16 → 32 (user order 03:21 EET, IDM parity: default 8, max 32 — ours keeps default 4): engine validation `1..=32` + updated error string, Settings input/label/validation 1–32. The Add URL dialog has no per-download segments input, so there was no second UI max to change; a grep confirms no other 16-cap remains. (5) Extension popup v2 (approved mockup `workspace/extension-popup-preview.html`): only the logo header + "Connected" pill (ping on load) + Developer card (avatar "M", "Developer: Mohand Kandil") + blue Facebook "f" circle linking to https://www.facebook.com/mohand.kandil/ + one note line about auto-capture + version footer; all old switches/buttons (catch toggle, opt-out, Send this page, Find videos, pending count, last hand-off) removed — catching is governed by the desktop app, and an earlier extension-local opt-out stays in force with no popup UI to change it. Floating video Download button restyled to Sonic identity (dark #0B0F24 panel, purple→pink gradient button, top-right of the video, quality tags Full HD for 1080p / HD for 720p, sizes only when known, "Sonic Manager" naming); the "Find videos on this page" trigger moved from the deleted popup button to a right-click context-menu item (`fetchline-find-videos`), sharing the new `injectVideoDetector()` helper with the kept `scan_page_for_videos` message handler; manifest v3, stable extension ID and host protocol unchanged. README updated (popup v2, menu trigger, install steps). (6) Taskbar progress (approved mockup `workspace/taskbar-progress-preview.html`): new `set_taskbar_progress` command (`Window::set_progress_bar` + native window title for the tooltip) driven from App.tsx — aggregate percent over downloading rows with known size (single active = its own percent), all-paused = paused state, nothing active clears the bar and restores the "Sonic Manager" title; invokes only when the rounded percent/title changes. Verification: engine suite **464/464 (0 failed, 22 binaries)** — `settings_reject_invalid_values` updated (33 rejected, 32 accepted) and its final assertion fixed in this round (the valid 32-save legitimately leaves the row at 32, not the base 8); defaults test asserts `sounds_enabled` + `show_download_complete_dialog`; round-trip test covers the sounds toggle. npm build passes (tsc + vite). Extension Node tests: popup 3/3 (rewritten for v2), background 4/4, video 17/17, watcher 11/11 + manifest validators OK. `cargo fmt --check`: my Rust regions clean (two nits fixed — `play_error_sound` signature, `sounds_enabled` tuple); the remaining ~43 hunks in lib.rs are pre-existing version drift, untouched. App crate cannot compile on this VM (no gdk-3.0) — VPS build is the first real compile. NOT pushed; VPS build + full retest pending (plus the user's full-app + extension test pass).

- **Round 69 — six Round 68 retest failures, investigated first, then fixed (2026-11-18, local only, NOT pushed):** Phase 1 investigation (read-only) found the root cause of items 1, 2, 4, 5 from code; item 3 (OVH) has no deterministic code cause. (1) EXTENSION NOT CONNECTED — root cause: the repo manifest hardcodes `C:\Program Files\Sonic Manager\fetchline-host.exe` (MSI install path) and NOTHING registers the host except the installer, which never ran on the VPS dev build — Chrome had no `HKCU\...\NativeMessagingHosts\com.fetchline.host` key at all. The host binary itself is PROVEN fine: built on Linux in 33 s, answered `{"ok":true,"type":"pong"}` to a framed ping with Chrome's exact argv against a fresh DB, and `handle_host_message` returns pong unconditionally (native_host.rs) — ping never touches the store, so a missing reply means the host never runs. Also proven: the manual VPS run of the host while the app ran rules out a DB-lock startup failure; the popup/background code is correct; the manifest has the nativeMessaging permission. Fix: the app self-registers the host on startup (Windows only, best-effort, never fatal) — `register_native_host_best_effort` in lib.rs writes the manifest (real host-exe path, UTF-8 no BOM) into the app data dir and points the Chrome + Edge (+ Firefox) registry keys at it via std-only `reg.exe` (CREATE_NO_WINDOW), idempotent via `reg query` first; host exe found next to the app exe (installed) or under `src-tauri/binaries` (dev build). PLUS the safety rule: the watcher used to cancel the Chrome download FIRST and hand over second — a failed hand-off silently swallowed the file. Now it pauses, hands over, cancels ONLY on confirmed receipt, and resumes the browser download on any hand-off failure. `sendToHost` (background + popup) is bounded at 15 s so a wedged host cannot stall a paused download forever. The popup now shows Chrome's real `lastError.message` under the pill instead of a bare "Not connected". Video path: injection/scan failures were swallowed by an empty catch in the menu handler — they are now recorded to `lastVideoScan` and the popup shows the last scan outcome (found / none / failed + reason). The detector itself was NOT reworked: the button only appears on hover of a video with variants found at injection time, and `isMediaUrl` drops extension-less/blob URLs — unproven as the VPS cause, left for the retest to diagnose with the new scan line. (2) CLIPBOARD "WAITING" STUCK — root cause: the offer called only `addDownload({url})`; engine `add_download` creates the row `queued` and never starts it, and the toolbar + menu gated Resume to paused/error only, so a queued row had NO start path anywhere (toolbar Resume "did nothing" = button disabled; Redownload re-added a queued row and also never started it). Fix: the offer now starts the download after adding; Resume is gated to paused/error/queued in the toolbar AND the menu (the engine's `resume_download` accepts any non-complete, non-downloading row); Redownload starts the new row after adding. Download Later rows stay queued by design but are now resumable. (3) OVH START DOES NOTHING — NOT proven from code: the submit path is sound (probe bounded 30 s, errors surface inline, buttons wired `submit(true)`/`submit(false)` correctly, no stuck promise, quota off by default). Most likely the second probe failed on the VPS network (or the tester clicked during an in-flight submit, when Cancel/X are disabled by design). No code change for OVH itself — the Round 69 (d) error dialog makes any such failure unmistakable, and the retest must check `startup.log` probe lines for the OVH attempt. (4) ERROR DIALOG DEVIATION — two root causes: (a) the Round 68 dialog fires only when a download ROW flips to `error`; an add-time probe failure (404) creates no row, so the dialog could never appear — fix: the Add dialog reports submit failures via new `onSubmitError` to App, which opens the standalone `DownloadErrorDialog` as an overlay (message variant: no polling, `title` + `message` props, `onClose` dismisses the overlay; `TitleBar` gained an optional `onClose` so its X dismisses the overlay instead of closing the main window); the inline pink text stays too. (b) UNCLOSABLE DIALOG — the capture-queue effect reopens the Add dialog instantly while `captureQueue` is non-empty, and stale pending browser/video rows sat `pending` forever, so the dialog reopened with a stale capture on every close. Fix: `Store::expire_stale_pending_links` rejects interactive rows older than 24 h (reason "Expired…"), called at the top of `drain_pending_links` (startup + every 30 s); `download_all` rows are NOT expired. (5) SETTINGS — (a) "Connections shows 8, not 4": NOT a bug — schema DEFAULT 8, `Settings::default()` 8, manager constructed with 8; the "default 4" was `INITIAL_SEGMENTS`, the initial segment count, not the setting. (b) "4 connections used despite 8": REAL BUG — `initial_ranges` capped at `INITIAL_SEGMENTS.min(max_connections)`, so the setting could only reduce segments below 4, never raise them. Fix: the setting IS the segment count now (`max_connections.min(size / MIN_SEGMENT_BYTES)`, 256 KiB floor so a small file with a high setting still splits sanely; `INITIAL_SEGMENTS` removed). Effective default is now 8 segments, matching the setting default. (c) "Completion-dialog checkbox appeared UNCHECKED yet the dialog showed": NOT a bug — verified: migration adds the column `DEFAULT 1`, `get_settings` indices 30/31 are correct, the SettingsDialog checkbox binds `settings.show_download_complete_dialog`, App checks the same setting before showing — the dialog appearing proves the setting is true; the observation was a test artifact (pre-load state or misread). No code change. (6) "DELETE FROM QUEUE" MISSING — the item existed but rendered only when `d.queue_id !== null`, and every row the retest checked was in no queue. Fix: always rendered, disabled when the row is in no queue (with a title explaining why), keeping IDM order (Remove → Move to queue → Delete from queue). Verification: engine suite **466/466 (0 failed, 22 binaries)** — was 464, +2 new tests (`max_connections_setting_raises_segment_count`: 8-setting 2 MiB file → 8 segments, 32-setting → 8 by the 256 KiB floor; `stale_browser_capture_expires_in_drain`: 25 h-old browser row rejected with "Expired", fresh video row stays pending). npm build passes (tsc + vite, 4.87 s). Extension Node tests: video 17/17, popup 4/4 (was 3/3, +1 scan-line test), background 4/4, watcher 12/12 (was 11/11, +1 resume-on-failed-handoff test), manifest validators OK. `cargo fmt --check`: my regions clean in lib.rs, manager.rs, store.rs (fixed one long log line in the new function); remaining hunks are pre-existing drift. App crate cannot compile on this VM (no gdk-3.0) — the VPS build is the first real compile of the new Rust (`register_native_host_best_effort`: `app.path().app_data_dir()` + `reg.exe` + `CommandExt::creation_flags` — std-only, no new dependency; `serde_json::json!` already a dependency). NOT pushed; rides with the next push. RETEST MUST CHECK (not verifiable locally): popup shows Connected after the app runs (and the popup's detail line shows Chrome's real error if it still fails); auto-capture hands a Chrome download to the app AND the Add URL dialog opens; a failed hand-off leaves the file downloading in Chrome; the floating video button appears on a real video page and the popup's scan line reports the outcome; clipboard offer starts the download; OVH 100Mb.dat starts (check `startup.log` probe lines if it does not); a 404 shows the standalone error dialog with Details; stale captures from previous sessions no longer reopen the Add dialog; a download with the setting at 8 uses 8 segments (parts table) and at 32 uses 32 on a large file; "Delete from queue" is visible (disabled) on a row in no queue and enabled on a Download Later row.

- **Round 70 — user's own-PC failures + Round 69 retest failures (2026-11-18, local only, NOT pushed):** (a) HOST NO-WINDOW + LAZY DB — host built as GUI-subsystem (`windows_subsystem = "windows"` in `app/host/src/main.rs`) so Chrome spawns show no console; Store opened lazily (Ping answered without the DB; open only when a message needs it; open failure → framed error, never `exit(1)`); startup/message errors logged to `host.log` next to `startup.log`, never stdout. New spawn test `host_answers_ping_when_database_is_unusable`. CORRECTION from user evidence: he DELETED the extension and the cmd window STILL appears — the host is NOT the cause of the flashing he still sees; the PowerShell sound spawns are the prime suspect (below). The host fix stays (a console host would flash whenever the extension IS installed). (b) POWERSHELL CONSOLE FLASH — `CREATE_NO_WINDOW` added to both PowerShell sound spawns in lib.rs (`play_wav_with_sound_player`, `play_default_system_sound`), `shutdown_computer`, `open_file_with` (rundll32); engine `play_wav_file` (`manager.rs`); `queue.rs` rasdial, shutdown ×2, `cmd /C start` (`run_program` and the user-configured antivirus scanner deliberately left alone). A failed download fires TWO unflagged PowerShell consoles (engine `emit_sound("download_failed")` + the error dialog mount → `play_error_sound`), matching "appears, disappears, appears again". Full `Command::new` sweep of lib.rs + engine found no other console-spawning call reachable from a failed download. (c) REGISTRATION — same-dir `fetchline-host-x86_64-pc-windows-msvc.exe` candidate added to `register_native_host_best_effort`, so the shipped ZIP works with the host named EITHER `fetchline-host.exe` OR `fetchline-host-x86_64-pc-windows-msvc.exe` next to the app exe. (d) PROBE HEAD→GET FALLBACK — `probe.rs`: transport-level HEAD failure falls back to ranged GET (bytes=0-0) in `probe_with_proxy` + `probe_with_cookies`; +2 tests (`head_reset` mock mode). REFUTED as the downet fix (see below) — kept only as zero-risk defence. (e) DOWNET / OVH "error sending request" — ROOT CAUSE PROVEN: the engine used reqwest with `rustls-tls`; downet serves an INCOMPLETE chain (leaf only, Sectigo intermediate missing, AIA present) and rustls-webpki does NOT fetch missing intermediates via AIA while Chrome/SChannel do — `openssl s_client` from this VM reproduces the exact failure ("unable to get local issuer certificate") and plain curl fails the same way. OVH's chain is COMPLETE (3 certs), so OVH's mid-download failure is a DIFFERENT, still-unproven cause. Fix: reqwest feature `rustls-tls` → `native-tls` (one line in `app/engine/Cargo.toml`; SChannel on Windows, OpenSSL on Linux). (1) OLD "WAITING" ROW RESUME — investigated, NOT an engine bug: `resume_download` sets the row to `error` with the real message on probe failure before returning, and every other outcome also moves the row out of `queued`; nothing ever sets a row back to `queued`. The observed "stays Waiting" cannot be produced by the engine — retest must re-check with the filter visible. (2) OVH 11 CONNECTIONS WITH SETTING 8 — explained, not a bug: `try_split` splits the largest remaining segment whenever a connection frees up, so parts grow past the setting (8 + 3 splits = 11) while concurrent connections never exceed 8; the progress window label "Parts (N connections)" conflated the two — relabelled "Parts (N parts)" (`ProgressWindow.tsx`). (3) MAIN-LIST SPEED "0 B/s" WHILE PROGRESS WINDOW LIVE — real bug, FIXED: the 1 s checkpoint persisted the single last 250 ms tick's instantaneous speed, so bursty flow wrote 0 to the DB for >5 s (expiring the Round 64 hold) while the progress window's 250 ms emits stayed live. The checkpoint now carries the average over the checkpoint interval (`checkpoint_total`/`checkpoint_at` in `run_inner`); the live emit keeps the instantaneous value. (4) 17 MISSING ROWS — investigated, most probably a list filter, NOT data loss: `list_downloads` returns all rows with `status != 'deleted'` and only user Remove/Cancel write `deleted`; the frontend `visible` memo filters by queue / unfinished / finished / grabber / category / search — 22 mostly-complete rows → "Unfinished" shows ~5, new adds raise it to 6, 7, exactly as observed. Retest must confirm via the sidebar filter state and `SELECT COUNT(*) FROM downloads` on the VPS. (5a) ERROR DIALOG HARD TO DISMISS — close paths were correct in code; the failure is most plausibly the stacked dialogs (error overlay over the still-open Add dialog, both showing the error) + no keyboard support (no focus, no Escape handler). Fix: `autoFocus` on the OK button + a window Escape handler in `DownloadErrorDialog.tsx` (both variants). (5b) SILENT START FAILURE ON URL 2 — submit path correct in code; engine cause is the rustls chain issue above. But the debounced probe failed SILENTLY — fix: `AddDownloadDialog.tsx` shows the probe failure as muted text next to the size ("Could not check this link — Start will try again", full reason in the tooltip). Verification: engine suite **468/468 (0 failed)** with native-tls + checkpoint change (first run aborted at 370 passed when the 512 MB /tmp tmpfs filled with leftover test dirs — quota_tests failed with SQLite DiskFull, an environment artifact; cleaned and re-ran green); host tests unit 2/2 + spawn 2/2; npm build passes (4.78 s); extension Node tests video 17/17, popup 4/4, background 4/4, watcher 12/12, manifest validators OK. `cargo fmt --check`: my regions clean; remaining diffs are pre-existing drift. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; rides with the next push. RETEST MUST CHECK: both downet URLs + OVH 100Mb.dat downloading in the app (the TLS proof); popup Connected on a real PC layout; no console flash on host spawn or on a failed download; old "Waiting" row Resume flips it to error with the 404; main-list speed stays live during a bursty download; row count vs app.db; error dialog dismisses with OK / Enter / X / Escape; a dead link shows the muted probe note before Start.

- **Round 71 — second-AI audit #2: extension/host, OVH 38% stall, white flash, minor regressions (2026-11-18, local only, NOT pushed; user approved "اه" ~18:43 EET):** (A) HOST/EXTENSION — host back to CONSOLE: `#![windows_subsystem = "windows"]` removed from `app/host/src/main.rs` (a GUI binary detaches from stdio pipes, so Chrome can spawn it and never talk to it — the Round 70 "Error when communicating..." failure; Chrome spawns hosts hidden anyway, so a console host does NOT flash). Registration strips the `\\?\` prefix (`clean_windows_path` in lib.rs) before writing the manifest path + registry, and logs the final path; the `reg` calls already had CREATE_NO_WINDOW. Extension pings the host FIRST (3 s bound) — if the ping fails the browser download proceeds natively, never paused, never swallowed. Video button shows immediately for the first video with variants + document-level `mouseover` delegation (mouseenter doesn't bubble); verified content.js has NO link-click interception / `preventDefault` anywhere. BUILD SAFEGUARD (procedure): every build/retest must rebuild the host crate first and copy it as the sidecar `app/src-tauri/binaries/fetchline-host-x86_64-pc-windows-msvc.exe`, printing its LastWriteTime — the Tauri build does NOT rebuild the host (separate workspace). The stale-host idea is REFUTED by timestamps (Round 70 host exe 11/18 1:36:24 PM, newer than the commit) — the safeguard is procedure, not the cause; the "Chrome launches via cmd.exe" mechanism is UNCERTAIN (actions still right). (B) OVH 38% = 3 OF 8 SEGMENTS — connect timeout capped at 8 s (`MAX_CONNECT_TIMEOUT`, both client builders) so dropped SYNs fail fast instead of hanging for the 21 s Windows TCP SYN timeout. Per-segment retry + non-fatal degradation: `SegOutcome::{Done, Deferred}`; a segment that fails to START (reqwest connect/timeout, status 429/503, or a SEND-phase stall — no response headers at all) with zero bytes gets 3 fast retries (500 ms / 1.5 s / 3 s, bounded by `max_attempts`), then DEFERS (backoff 2/4/8 s, range back to Pending) instead of failing the download; the supervisor ratchets the effective concurrency down to the workers that actually received bytes; the deferred segment is retried ALONE (solo) and the download fails only if it still fails then. Segment stealing: a freed worker splits a segment stuck in Connecting > 5 s (or the largest remaining). Full error chains are now logged per attempt (`error_chain_text`, `EngineError::source()`). New mock mode (`max_concurrent_gets`, 429 past the limit) + test `download_completes_when_server_limits_concurrency_to_three` (2 MiB, 8 segments, limit 3 — PASSES). TWO REGRESSIONS the suite caught: (1) my first loop-top break check (`done >= len`) spun forever on open-ended segments (length reads as u64::MAX) — the FTP unknown-size test failed; fixed to break only when no segment waits on a deferral. (2) my first `is_deferrable` treated a BODY stall (headers arrived, then silence) as deferrable, so the Round 55 stall watchdog (max_retries = 1) deferred forever instead of failing — fixed: only the send-phase stall defers (new `send_stall_error()`), body stalls stay on retry-then-fail. (C) WHITE FLASH — `backgroundColor: "#0B0F24"` in tauri.conf.json + `.background_color(Color(11, 15, 36, 255))` on all three WebviewWindowBuilders + `<style>html, body, #root { background-color: #0B0F24; }</style>` in index.html + `bg-[#0B0F24]` on the table container. Show-when-ready: windows created hidden (`visible: false`), shown by a new `window_ready` Tauri command fired from React after first mount, with a 3 s fallback thread that guarantees no window stays hidden forever. Auto-reload spam stopped: `maybe_auto_reload` removes the heartbeat entry when the window no longer exists. (D) MINOR — (1) speed 1:4 ratio VERIFIED NOT a ×4 bug (checkpoint divides bytes by actual elapsed seconds; the retest numbers were ranges from different moments); the one real defect was `checkpoint_at` pre-aged by CHECKPOINT_INTERVAL (~5× understatement of the first sample) — fixed to `Instant::now()`, no blind ×4. (2) Red banner X: poll paths refuse to re-show the exact dismissed message (a successful poll clears the memory); X gains `WebkitAppRegion: 'no-drag'`. (3) Stale completion dialog: first snapshot seeds `completeDialogHandled` with all already-completed IDs. (4) Deleted files: `DownloadRow.file_missing` (not a DB column) set in `list_downloads`/`get_download` for complete rows whose `save_path` doesn't exist; UI shows "File missing from disk". (5) Probe guards: fires only for http://, https://, ftp:// + valid `new URL()` + no second `://`; monotonic `probeReqId` discards stale responses. Verification: engine suite **469/469 (0 failed, 24 binaries)** — 468 after Round 70 + 1 new test; host tests unit 2/2 + spawn 2/2; npm build passes (4.39 s); extension Node tests video 17/17, popup 4/4, background 4/4, watcher 13/13 (+1 new ping test), manifest validators OK. `cargo fmt --check`: Round 71 regions clean; remaining diffs are pre-existing drift (do not reformat). App crate cannot compile on this VM — VPS build is the first real compile (the C3 show-when-ready flow + `clean_windows_path` are uncompiled here, though the Tauri APIs were verified against vendored tauri-2.12.5). NOT pushed; rides with the next push. RETEST MUST CHECK: popup Connected + host.log exists (Chrome actually launched the host); auto-capture hands over (no swallowed click); video button appears on hover AND immediately; OVH 100Mb.dat completes; no console flash; white-flash timing per window; list vs progress-window speed on the same download; banner X stays dismissed; completion dialog never re-shows; Finished row with a deleted file shows "File missing from disk"; probe stays silent on "X", partial protocols, concatenated URLs.

- **Round 72 — extension auto-injection, OVH patient tail, start-with-1 segmentation (2026-11-18, local only, NOT pushed; user approved "نفذ" ~22:29 EET):** (A) VIDEO BUTTON — root cause VERIFIED IN CODE: `content.js` was injected ONLY via the right-click "Find videos" menu item and `manifest.json` had NO `content_scripts`, so the button could never appear on a real page without the menu click (the popup's "1 downloadable video found" was a stale scan). Fix: `content_scripts` in the manifest (http/https, document_idle, video-detect.js + content.js); content.js re-scans + prunes detached videos + `video_allowed` gate + `rescan_videos` handler; the menu item now rescans first and falls back to injection; the popup triggers a fresh scan on open and labels stored scans with their host, so a stale result can never pose as the current page. Also fixed a pre-existing `v.format` → `variant.format` ReferenceError that left the quality button stuck on "Sending…". (B) AUTO-CAPTURE — verified correct in code (ping first, pause, hand over, cancel ONLY on confirmed receipt, resume on failure — never weakened); the file:// clicks are explained, not a bug (the watcher keys on `item.url`, never the page scheme; a `file://` download itself is correctly skipped; direct-mp4 navigation opens Chrome's media player — no download event exists). (C) OVH TAIL — when only one segment remains unfinished, the download no longer fails after `max_attempts`: it keeps retrying with backoff 5/15/30/60 s (cap 60 s) until success or user cancel/pause, with a 20 s connect timeout for tail attempts (`is_tail_patient`: transport errors incl. received-bytes cases, 429/503, send-stall — NEVER body stalls or 404/416); all three watchdogs suppress while a tail waits patiently. The `is_request()` discovery: a dropped connection is reqwest `Kind::Request` (mid-send close) — `is_connect`/`is_timeout`/`is_body` all miss it; added to `is_tail_patient` only, NOT to `is_deferrable` (the slow-path retry countdown is pinned by an existing test). Redownload in place: new `redownload_download` engine method + Tauri command (probe first, row survives a failed probe, `.part` kept) + "Working…" feedback on clicks, fixing the dead Resume/Redownload clicks on Failed rows. Settings connections field is now digits-only, clamped 1–32. (D) DYNAMIC SEGMENTATION — downloads start with ONE segment and grow by splitting the largest remaining segment (1 MiB floor on REMAINING bytes) whenever a worker frees; the connections setting is now a pure concurrency cap. UI shows active connections separately from parts ("2 connections · 6 parts") in the progress window and main list. Verification: engine suite **473/473 (0 failed, 25 binaries)** — 469 + 4 new Round 72 tests (start-with-1 + growth to 8, tail patient retry, tail never patient on body stall, redownload); host tests unit 2/2 + spawn 2/2; npm build passes; extension Node tests 49/49 (was 38), manifest validators OK. `cargo fmt --check`: Round 72 regions clean; remaining diffs are pre-existing drift. App crate cannot compile on this VM — VPS build is the first real compile. NOT pushed; rides with the next push. RETEST MUST CHECK: video button appears automatically (no menu click) on a real video page; popup scan line names the current page; a download-link click opens the Add URL dialog pre-filled; OVH completes past 99% with the tail retrying ("retrying until it succeeds"); Redownload restarts a Failed row in place; Settings field rejects non-digits and clamps 1–32; "N connection(s) · M part(s)" shows under downloading rows.

- **Round 74 — user-found 16 bugs + whole-download stall retry (2026-11-19, local only, NOT pushed; user approved "نفذ كله" ~07:08 EET):** (1) two stacked progress windows — race in `open_progress_window`, fixed with `WINDOW_BUILD_MUTEX` serializing all window creation + re-check in every builder; (2) free-space display refreshes after delete/finish; (3) "Download removed" restyled compact; (4) Redownload on Finished rows + `file_missing` guards on Open/Open-with/Open-folder; (5) uniform status pills; (6) queue delete/rename — the commands and engine methods did not exist at all, added `delete_queue`/`rename_queue` (store + manager + Tauri commands; delete moves the queue's downloads to the main queue); (7) backdrop-click-to-close removed in 16 places; (8) global WebView context-menu suppression, custom menu on rows only; (9) single instance via `tauri-plugin-single-instance` 2.5.5; (10) File > Exit now calls `exit_app` (was hiding to tray); (11) Cancel asks "Are you sure?" first (new fixed-size `ConfirmDialog`); (12) clipboard poller rewritten (fresh `arboard::Clipboard` per tick) + capture dialog is the SAME Add URL window (filename title, URL locked); (13) standalone Add URL window (`add-url-<id>`) that pops in front over any window, same treatment for complete/error dialogs; (14) dialog-close audit (Escape/Cancel/X everywhere; progress + main still minimize to tray on X); (16) drag & resize ONLY on main + active progress window (user-narrowed; fixed dialogs get `draggable={false}`; tao 0.37.2 verified `RESIZABLE` → `WS_SIZEBOX` + `WM_NCHITTEST`, with a `.shadow(false)` fallback named if side/bottom edges don't resize); (A) whole-download stall retries instead of failing at 180 s (patient-tail cap, default 10 min); (B) progress-window Resume — no frontend or engine defect found, most likely the orphaned duplicate window from (1); (C) popup "no videos found" is extension-side, no app/host cause. Item 15 (video dropdown) by a parallel agent, not here. Verification: engine 483/483, host 4/4, npm build passes. VPS retest still owed (first real compile of the new Rust).

- **Round 75 — user's own-PC test, 7 items (2026-11-19, local only, NOT pushed; user approved "نفذ" ~14:38 EET):** (1) linkId — `api.ts:177`/`185` sent `link_id`, Tauri v2 defaults to camelCase keys so EVERY `get_pending_link`/`open_add_url_window` call failed ("missing required key linkId"); fixed JS-side to `{ linkId }`, plus a guard test (`apiKeys.test.mjs`, 2 tests) that fails on any invoke argument key containing `_` — the guard caught two MORE always-broken invokes of the same class, `update_queue` and `create_grabber_project` (required Rust params, camelCase keys), so queue scheduling could never be saved from the UI and no Site Grabber project could ever be created from the UI; both fixed to camelCase. The 7 stuck `pending` rows (4× mov_bbb, 3× El Kalam) will be offered one at a time by the auto-open queue, no-spam via the item-(6) `failedCaptureIds` set. (2) Categories — each sidebar click called THREE handlers, React batching let the LAST write win so `selectedCategory` ended null; now exactly ONE handler per click; selection state + handlers extracted into `useSidebarSelection.ts`; test `sidebar_selection.test.tsx` 4/4, and it pins the bug (old triple-call restored → 3/4 fail). Vitest 2 + jsdom 25 + @testing-library/react 16 added as devDeps (`npm run test:ui`). (3) Clipboard — `GetClipboardSequenceNumber()` is now the change signal; DROPPED the same-text hash, the session-long `seen` set, and the `url_exists` hard block; `ClipboardMonitor` holds only `last_sequence`; an already-downloaded URL is offered with `already_in_list: true` and the Add URL window shows "This link is already in your list — you can download it again or cancel."; the offer opens the Add URL window directly — `ClipboardOfferDialog.tsx` DELETED; the shell poller skips the read entirely when the sequence is unchanged (no arboard open, no RDP channel cross) and records the sequence after the app's own clipboard writes (`note_own_clipboard_write`); `dismiss_clipboard_offer` removed; clipboard tests rewritten, 17/17 (same text + new sequence → offered twice; unchanged sequence across 5 polls → once; own-write sequence not offered). (4) Popup scan — aggregate, NOT frameId:0-only: `background.js` keeps scans keyed by tab + frameId, the popup shows the SUM across frames; a 0-video frame can never overwrite a non-zero aggregate; frame 0 reporting a new page URL or tab removal resets the tab's map; `scanActiveTab` waits ~400 ms for frames to report; `background.test.js` 13/13 (+4 aggregate tests). (5) Connected — Named Mutex `Local\FetchlineAppRunning`, created at the very top of `run()` in `lib.rs` and held in Tauri managed state for the process lifetime; the host Ping handler calls `OpenMutexW` and replies `app_alive: true|false` (`None` off Windows); popup shows Connected ONLY when `app_alive` is not false, else "Sonic Manager is closed - open it to catch downloads"; `popup.test.js` 10/10 (+2); Windows-only engine test `host_ping_app_alive_follows_the_mutex` (absent → false, held → true, dropped → false) runs only on a Windows build. (6) Banner — dismissals keyed per (source, message); a successful poll clears ONLY its own source's dismissal, never a non-poll error; failed auto-open ids go into `failedCaptureIds` and are never retried until a NEW pending id arrives or the user clicks the banner's Retry ("Capture failed — Open"); new pure module `bannerErrors.ts` + 5 tests. A per-row capture action was not feasible (no captures-list UI in the main window) — the banner Retry is the surface. (7) Cancel dialog — `ConfirmDialog.tsx` restyled Sonic (`#0B0F24` surface, purple→pink→cyan gradient top bar, fixed `w-[420px] max-w-[92vw]`); the safe button ("Keep downloading" / "Keep it") gets `autoFocus` so Enter keeps the download; fixed size kept, Escape/X/Cancel unchanged. Verification: engine 485/485 (0 failed; 483 + 2 new clipboard tests), extension 69/69, host 4/4, Vitest UI 9/9, `apiKeys` 2/2, `speedGraph` 8/8, npm build passes, `cargo fmt` on changed Rust regions clean (pre-existing drift untouched). VPS retest still owed (first real compile of the new Windows FFI + shell changes; Windows-only mutex test runs only there).
