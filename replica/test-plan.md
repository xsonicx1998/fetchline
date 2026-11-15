# Test plan: Fetchline (IDM rebuild) — built slices only

Build: 2026-11-14, after replica-build slice 1 (S01/S02/S03) and slice 2
(S04/S05). Env: this Linux VM, local mock HTTP server only — no public
internet, no IDM servers. Engine: `cargo test` in `app/engine`
(51 integration + 4 unit tests). UI: `npm run build` + Vite preview smoke
test. There is no desktop binary to click through: the Tauri shell does
not compile on this VM (missing `gdk-3.0`), so every case that needs the
running desktop app is marked **blocked** with its reason. The UI code
paths behind those cases were reviewed by reading the components.

Scope: only features marked `yes`/`partial` in `replica/features.csv` —
F02 add URL, F03 pause/resume, F10 categories, F04 queues + scheduler +
on-completion action. F01 (browser catch), F05–F09, F11 are not built and
are not in this plan. Periodic synchronisation is deferred to L and is
not tested as a feature (its rejection is tested).

Result key: PASS = ran and passed · FAIL→FIXED = reproduced, then fixed
(see `bugs.md`) · DOC = test documents a known gap, no fix in this pass.

## F02 — Add a URL and start a download (S01, S02)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F02-H1 | happy | Add a valid URL, Start Download | Download completes, bytes match the server exactly, `.part` gone | cargo test `full_download_multi_segment_matches_bytes` | PASS |
| F02-H2 | happy | Add a URL, "Save for later" | Download is filed in the main queue, status `queued` | cargo test `add_download_with_queue_files_it_in_main_queue` | PASS |
| F02-H3 | happy | Add a URL with a filename from Content-Disposition / URL | Filename used; category auto-picked from the extension | cargo test (existing probe + category tests) | PASS |
| F02-E1 | edge: HEAD hides length | Server answers HEAD without Content-Length | Download still works as one open-ended segment | cargo test `probe_falls_back_when_head_hides_length` | PASS |
| F02-E2 | edge: HEAD refused | Server answers HEAD 405 | Probe falls back to ranged GET, learns size + ranges | cargo test `probe_falls_back_when_head_refused` | PASS |
| F02-E3 | edge: unicode filename | Filename `🎬 فيلم.mp4` | Stored unchanged, category Video | cargo test `add_download_keeps_unicode_filename` | PASS |
| F02-E4 | edge: duplicate file on disk | Add the same link after the first finished | Second file saved as `file (2).bin` | cargo test `second_add_after_first_completes_gets_suffixed_path` | PASS |
| F02-N1 | negative: empty URL | Add with an empty link | `Invalid address` error, no download created | cargo test `add_download_rejects_empty_url` | PASS |
| F02-N2 | negative: wrong scheme | Add `ftp://…` | `Only http:// and https://` error | cargo test `add_download_rejects_non_http_scheme` | PASS |
| F02-N3 | negative: unparsable URL | Add `not a url` | `Invalid address` error | cargo test `add_download_rejects_unparsable_url` | PASS |
| F02-N4 | negative: 404 | Server answers 404 to HEAD | Error naming 404, no download created | cargo test `add_download_rejects_404` | PASS |
| F02-N5 | negative: unreachable host | URL on a closed port | `Network error`, no download created | cargo test `add_download_unreachable_host_errors` | PASS |
| F02-N6 | negative: duplicate add | Add the same link twice before starting either | Two different save paths, second suffixed ` (2)` | cargo test `duplicate_add_gets_different_save_path` | FAIL→FIXED (BUG-002) |
| F02-M1 | manual: dialog validation | Type a bad link in S02, press Start Download | Inline error, dialog stays open | blocked — no desktop binary | BLOCKED |
| F02-M2 | manual: Escape / backdrop | Open S02, press Escape or click outside | Dialog closes, nothing added | blocked — no desktop binary | BLOCKED |
| F02-T1 | to check: very long filename | Filename > 200 chars | Untested — Windows path limits may reject it at file creation | — | TO CHECK |

## F03 — Pause and resume (S01, S03)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F03-H1 | happy | Start, pause mid-download, resume | Completes byte-exact from persisted progress | cargo test `pause_then_resume_completes_with_correct_bytes` | PASS |
| F03-H2 | happy | Pause, then Start (not Resume) on the paused row | Continues from persisted progress, completes | cargo test `start_download_on_paused_download_resumes` | PASS |
| F03-E1 | edge: file changed | Server file grows + new ETag while paused, then resume | Restarts from zero, new size, byte-exact | cargo test `resume_restarts_when_remote_file_changes` | PASS |
| F03-E2 | edge: ETag-only change | Same size/bytes, new ETag while paused, then resume | Restarts from zero, new ETag stored | cargo test `resume_restarts_when_only_etag_changes` | PASS |
| F03-E3 | edge: server drops resume | Server starts answering ranged GETs with 200 + whole file, then resume | Download goes to `error` naming the ignored range; no wrong bytes stitched in | cargo test `resume_fails_when_server_ignores_range` | PASS |
| F03-E4 | edge: pause timing | Pause lands before any checkpoint | Progress still correct (checkpoint on pause) | cargo test (pause tests; timing-sensitive — passed 3 consecutive full runs) | PASS |
| F03-N1 | negative: pause when idle | Pause a queued (never started) download | `not running` error | cargo test `pause_download_when_not_running_errors` | PASS |
| F03-N2 | negative: start/resume a finished download | Start or Resume on a complete row | `already complete` error | cargo test `start_and_resume_on_complete_error` | PASS |
| F03-M1 | manual: pause/resume buttons | Toolbar Pause/Resume, S03 dialog buttons | Status flips, progress continues from the same byte | blocked — no desktop binary | BLOCKED |
| F03-T1 | to check: restart after power loss | Kill the app mid-download, relaunch, resume | Resume from the last checkpoint (same code path as F03-H1) | — | TO CHECK (Windows) |

## F10 — Categories and auto-assign (S04)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F10-H1 | happy | Add `movie.mp4`, `song.mp3`, `doc.zip` | Video / Music / Compressed, builtin counts update | cargo test `schema_loads_and_seeds_categories` | PASS |
| F10-H2 | happy | Create custom category `Clips` (`.xyz`), add `clip.xyz` | Auto-assigned to `Clips` | cargo test `category_auto_assign_uses_custom_category` | PASS |
| F10-H3 | happy | Move a download to another category | Category changes, counts update | cargo test `category_delete_blocked_with_downloads_then_allowed` (move step) | PASS |
| F10-E1 | edge: unknown extension | Add `notes.qqq` | Falls back to Documents | cargo test `category_auto_assign_unknown_extension_falls_back_to_documents` | PASS |
| F10-E2 | edge: uppercase extension | Add `TRAILER.MP4` | Video (case-insensitive match) | cargo test `category_auto_assign_is_case_insensitive` | PASS |
| F10-N1 | negative: bad category | Empty name / empty folder / no extensions / duplicate name | Clear validation error, nothing created | cargo test `create_category_validation_errors` | PASS |
| F10-N2 | negative: delete in use | Delete a category that has downloads | Refused with "move them first"; delete works after moving them | cargo test `category_delete_blocked_with_downloads_then_allowed` | PASS |
| F10-N3 | negative: delete builtin | Delete `cat-video` | Refused: built-in | cargo test (same test, builtin step) | PASS |
| F10-N4 | negative: delete after Remove | Delete a category whose downloads were all Removed | Delete succeeds (was: raw foreign-key error) | cargo test `delete_category_allowed_when_downloads_deleted` | FAIL→FIXED (BUG-004) |
| F10-N5 | negative: unknown ids | Move/delete with ids that do not exist | Not-found error | cargo test `move_to_category_unknown_ids_error`, `delete_category_unknown_id_errors` | PASS |
| F10-M1 | manual: drag to category | Drag a table row onto a sidebar category | Row moves, counts update | blocked — no desktop binary | BLOCKED |
| F10-M2 | manual: "Move to category…" menu | Toolbar menu on a selected row | Category changes | blocked — no desktop binary | BLOCKED |

## F04 — Queues, reorder, scheduler, on-completion action (S05)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F04-H1 | happy | Queue 3 files, Start Now | Drains in insertion order, each finished file leaves the queue, all byte-exact | cargo test `queue_drains_in_order_and_clears_queue_id` | PASS |
| F04-H2 | happy | Start queue, Stop mid-download, Start again | Current file pauses (resumable), rest never start; restart drains all | cargo test `queue_stop_leaves_remaining_files_queued` | PASS |
| F04-E1 | edge: reorder | Move first file to last, move to index 99, reorder a 1-file queue | Order follows, out-of-range clamps to the end, no error | cargo test `queue_reorder_changes_order`, `reorder_single_item_and_out_of_range_clamps` | PASS |
| F04-E2 | edge: empty queue | Start an empty queue / start a running queue twice / stop an idle queue | Clear errors (`no files`, `already running`, `not running`) | cargo test `queue_state_errors`, `start_queue_twice_errors` | PASS |
| F04-E3 | edge: file fails mid-queue | Second file's server starts answering 500 | Run stops, failed file stays queued with `error` naming 500, first file complete | cargo test `queue_stops_when_a_file_errors` | PASS |
| F04-E4 | edge: unknown ids | Add/reorder/remove with unknown ids; empty or duplicate queue name | Not-found / validation errors | cargo test `queue_state_errors` | PASS |
| F04-N1 | negative: queue a finished download | Drag/add a complete download into a queue | Refused: "already finished" | cargo test `add_to_queue_rejects_completed_download` | FAIL→FIXED (BUG-003) |
| F04-N2 | negative: queue of finished files only | Queue holds only a finished file, Start Now | Finished file leaves the queue; no completion action is armed | cargo test `queue_with_only_completed_files_registers_no_action_and_drops_them` | FAIL→FIXED (BUG-003) |
| F04-S1 | scheduler: due queue | Start time in the past, tick | Queue starts and drains | cargo test `scheduler_starts_queue_at_start_time` | PASS |
| F04-S2 | scheduler: future queue | Start time in the future, tick | Queue untouched | cargo test (same test, future step) | PASS |
| F04-S3 | scheduler: stop time | Stop time arrives mid-run | Current file pauses, rest stays queued | cargo test `scheduler_respects_stop_time` | PASS |
| F04-S4 | scheduler: stale window | Start and stop both in the past, tick | Never starts | cargo test (same test, stale step) | PASS |
| F04-S5 | scheduler: one-time only | Queue drains, add another file, tick again | Second file is not auto-started | cargo test `scheduler_one_time_queue_does_not_restart` | PASS |
| F04-S6 | scheduler: clear it | Set a schedule, then save "No schedule" | Saves, times cleared, tick never starts it | cargo test `update_queue_accepts_none_and_clears_schedule` | FAIL→FIXED (BUG-001) |
| F04-S7 | scheduler: no queues | Tick with only the seeded main queue | No-op, no error | cargo test `scheduler_tick_with_no_queues_is_noop` | PASS |
| F04-S8 | scheduler: startup | Start-on-startup queue is empty | Skipped, no error | cargo test `start_startup_queues_skips_empty_queues` | PASS |
| F04-S9 | scheduler: malformed time | Save start time `tomorrow at noon` | Rejected at save with a clear error; nothing stored (was: stored, tick silently never starts it) | cargo test `update_queue_rejects_malformed_start_time` | FAIL→FIXED (BUG-005) |
| F04-S10 | scheduler: inverted window | Save stop before start | Rejected at save — "The stop time must be after the start time." (was: stored, never runs) | cargo test `update_queue_rejects_stop_before_start` | FAIL→FIXED (BUG-006) |
| F04-S11 | scheduler: periodic | Save schedule type `periodic` with interval 30 | Saves; queue reads back `periodic`, interval 30 (was: refused "not built yet" — periodic sync is built now, see Round 6) | cargo test `update_queue_accepts_periodic_sync_with_valid_interval` | PASS (was PASS — behaviour changed deliberately) |
| F04-A1 | action: run program | Queue finishes with `run_program` (marker script) | Program runs, pending action cleared | cargo test `queue_action_run_program_executes_and_dismiss_clears` | PASS |
| F04-A2 | action: open file | Queue finishes with `open_file` | Pending action carries the finished path; executing it succeeds | cargo test `open_file_action_executes` | PASS |
| F04-A3 | action: shutdown | Queue finishes with `shutdown` | Pending action exists; executing it refuses off Windows; dismiss clears it | cargo test `queue_action_shutdown_errors_off_windows` | PASS |
| F04-A4 | action: no pending | Execute/dismiss with nothing pending | Execute errors, dismiss is a no-op | cargo test `execute_and_dismiss_without_pending` | PASS |
| F04-M1 | manual: scheduler dialog | Save stop before start in the dialog | UI error "stop must be after start", nothing saved | blocked — no desktop binary | BLOCKED |
| F04-M2 | manual: countdown dialog | Queue with shutdown finishes | 60 s countdown, Run now / Cancel; nothing runs silently | blocked — no desktop binary | BLOCKED |
| F04-M3 | manual: scheduler tabs | Files tab: up/down reorder, remove; Schedule tab: save | Order and schedule persist | blocked — no desktop binary | BLOCKED |

## Not tested here (out of scope for this pass)

- F01 browser catch / extension (not built).
- F05–F09, F11 (not built).
- Periodic synchronisation (deferred to L by design).
- The Tauri shell itself: `cargo check` in `app/src-tauri` still fails on
  this VM (missing `gdk-3.0`); the 16 queue/category commands are
  uncompiled, and drag-and-drop + the countdown dialog are UI-only until
  the first Windows build.
- Playwright: not installed on this VM and no Chromium is available, so
  no browser-driven e2e specs were written. The Vite preview serves the
  built UI (HTTP 200, `Fetchline` title, JS asset 200), and the
  components were reviewed by reading them.

## Summary

- Automated engine cases: 51 integration + 4 unit tests, all passing
  (3 consecutive full runs, 0 failures).
- UI build: `npm run build` passes; Vite preview serves the app.
- Manual / blocked: 8 cases (dialog validation, Escape/backdrop, pause
  buttons, drag-and-drop ×2, scheduler dialog validation, countdown
  dialog, scheduler tabs) — all need the Windows desktop binary.
- Bugs found: 4 × S2 (all fixed, tests kept), 2 × S3 (documented, not
  fixed), 1 × S4 (documented, not fixed). See `bugs.md`.

## Round 2 — Settings (S09), proxy, speed limiter (S12), integration (2026-11-14)

Extends the round-1 plan; it does not replace it. Build: slices 1–4
(S01–S05, S07, S09, S10, S12). Env: same Linux VM, local mock
HTTP / HTTP-proxy / SOCKS5 servers only — no public internet, no IDM
servers. Engine: `cargo test` in `app/engine` (98 integration + 12
unit, 2 consecutive full runs, 0 failures). UI: `npm run build` passes;
Vite preview serves the built UI (index 200, `Fetchline` title, JS
asset 200 — note: preview must be started with `--host 127.0.0.1`,
it otherwise binds IPv6 localhost only). No desktop binary: the
Tauri shell still does not compile here (missing `gdk-3.0`), so all
35 Tauri commands are uncompiled and every case needing the running
desktop app is marked **blocked**.

New coverage in this round: 17 integration tests (98 − 81), plus the
mock HTTP proxy gained optional Basic-auth enforcement
(`HttpProxyServer::start_with_auth`) so wrong credentials can be
tested. No S1 and no new S2 was found. New bugs: 3 × S3, 1 × S4
(see `bugs.md` BUG-008–011). BUG-005/006/007 were re-checked and are
still present (see below).

### S09 — Settings applied live

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-H1 | happy | `set_speed_limit(-5)`; then `set_speed_limit(10_000_000)`; then `Some(0)` | Negative rejected, nothing stored; huge sane value stored and applied (10_240_000_000 B/s); 0 = unlimited (NULL) | cargo test `speed_limit_rejects_negative_and_accepts_huge_sane_value` | PASS |
| S09-H2 | edge: mid-flight | Start a 2 MiB download (4 segments), change max_connections to 1 mid-flight | Running download keeps 4 segments; the next download has 1 | cargo test `max_connections_change_does_not_alter_running_download` | PASS (DOC) |
| S09-H3 | happy | max_retries = 0, server GETs fail with 500 | Download errors after a single attempt, fast (< 10 s), error names 500 | cargo test `max_retries_zero_means_single_attempt_and_fast_error` | PASS |
| S09-N1 | negative | `test_proxy` with port 0 / type `ntlm` | Validation error, nothing dialled | cargo test `test_proxy_rejects_invalid_input` | PASS |
| S09-N2 | negative: secret leak | Save proxy with password, serialise the UI row | JSON holds only `has_password`, never the password | cargo test `proxy_row_serialised_contains_no_password` | PASS |
| S09-M1 | manual: dialog | All 6 tabs, validation, Escape/backdrop, Save/Cancel | Values persist, bad values rejected inline | blocked — no desktop binary | BLOCKED |
| S09-M2 | manual: defaults | default_action = save-for-later; confirm_delete on/off | Add dialog primary button follows the default; Remove asks (or not) | blocked — no desktop binary (code reviewed: `App.tsx:423` passes `default_action`; `:195` gates the confirm dialog) | BLOCKED |
| S09-M3 | manual: status bar | Speed-limit button in the status bar | Shows the current cap, opens Settings | blocked — no desktop binary (code reviewed: `App.tsx:411–415`) | BLOCKED |
| S09-M4 | manual: proxy Test | Saved password, Test without retyping | Dialog hints the password must be retyped (code reviewed) | blocked — no desktop binary | BLOCKED |
| S09-M5 | manual: theme | Toggle theme in the header | Applies at once, persists in settings | blocked — no desktop binary (code reviewed: `App.tsx:182–188`) | BLOCKED |
| S09-T1 | to check: partial save | Save settings, then a category edit fails | Settings + earlier categories are already saved; dialog shows the error, stays open (code review only) | — | TO CHECK (desktop) |

### S12 — Speed limiter

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S12-H1 | happy | Two 256 KiB downloads at once, cap 32 KiB/s | Together ~16 s (512 KiB / 32 KiB/s) — the cap is global, not per download; both byte-exact | cargo test `speed_limit_is_global_across_concurrent_downloads` | PASS |
| S12-H2 | edge: big chunk | 64 KiB chunks at a 16 KiB/s cap, 128 KiB file | Completes in ~8 s (round-1 fix: large acquires are spent in rate-sized pieces) | cargo test `speed_limit_chunk_larger_than_rate_completes` | PASS |
| S12-E1 | edge: absurd cap | `set_speed_limit(i64::MAX)` | Rejected with a clear "too large" error (max 10,000,000 KB/s); nothing stored, cap stays unlimited — FIXED, see bugs.md BUG-008 | cargo test `speed_limit_absurd_value_is_rejected` | PASS (was FAIL/DOC) |
| S12-M1 | manual: status bar | Cap shown in the status bar | Current cap visible | blocked — no desktop binary | BLOCKED |

### S09-P — Proxy (HTTP / SOCKS5)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-P1 | happy/negative | HTTP proxy demanding auth; right creds, then wrong creds | Right creds download through it; wrong creds fail the probe with 407, no download created | cargo test `http_proxy_wrong_credentials_are_rejected` | PASS |
| S09-P2 | edge: scope | `use_for = ["https"]`, http:// download | Completes direct, proxy never contacted (hits 0) | cargo test `proxy_https_only_scope_skips_http_downloads` | PASS (DOC) |
| S09-P3 | happy | `use_for = ["http"]`, http:// download | Goes through the proxy (hits > 0) | cargo test `proxy_http_only_scope_uses_proxy_for_http` | PASS |
| S09-P4 | happy | Download All page fetch with a proxy saved | Page fetch goes through the proxy (hits > 0) | cargo test `fetch_page_links_goes_through_proxy` | PASS |
| S09-P5 | happy | Save proxy A, download; save proxy B, download | First download uses A, second uses B, A gets no further hits — no restart | cargo test `proxy_change_applies_to_next_download` | PASS |
| S09-P6 | edge: Test scope | `test_proxy` with https-only proxy + http:// test URL | Fails with a clear "only used for https" error, 0 proxy hits — FIXED, see bugs.md BUG-010 | cargo test `test_proxy_https_only_scope_rejects_http_test_url` | PASS (was PASS/DOC) |
| S09-P7 | edge: Test auth | `test_proxy` with wrong proxy credentials | Fails with "Proxy authentication failed (407)" — the UI shows it as a failure, not "OK" — FIXED, see bugs.md BUG-009 | cargo test `test_proxy_wrong_credentials_is_an_error` | PASS (was PASS/DOC) |
| S09-P9 | happy: Test scope | `test_proxy` with http-only proxy + http:// test URL | Returns 200, proxy hits > 0 — the test really uses the proxy | cargo test `test_proxy_http_scope_uses_proxy_for_http_url` | PASS |
| S09-P10 | edge: Test FTP | `test_proxy` with an FTP proxy + http:// test URL | Fails with a clear FTP error instead of going direct and reporting success — FIXED, see bugs.md BUG-010 | cargo test `test_proxy_ftp_type_is_rejected` | PASS |
| S09-P8 | already covered (round 1 / milestone 4) | HTTP + auth, SOCKS5 + auth + wrong password, disabled proxy, bypass list, FTP stored-but-not-applied, test_proxy 200 / down proxy | All still pass in the full suite | existing tests, re-run | PASS |
| S09-P11 | happy: FTP probe + download | Local FTP fixture serves 512 KiB (SIZE on); add `ftp://` URL, no proxy | Probe reports the size; download completes byte-exact | cargo test `ftp_probe_and_download_direct` | PASS |
| S09-P12 | happy: FTP proxy | FTP proxy configured (proxy USER/PASS, then `USER <user>@<host>`); add `ftp://` URL | Proxy login sequence in order; download byte-exact; origin login sees `user@host` | cargo test `ftp_proxy_routes_and_authenticates_in_order` | PASS |
| S09-P13 | edge: FTP bypass | FTP proxy configured + bypass list containing the FTP host | Download goes direct; proxy sees 0 connections | cargo test `ftp_proxy_bypass_list_goes_direct` | PASS |
| S09-P14 | happy: FTP segments + resume | 2 MiB file over FTP, 4 segments; pause mid-download, resume | Multi-segment byte-exact; pause/resume completes byte-exact (REST) | cargo test `ftp_download_multi_segment_byte_exact`, `ftp_pause_then_resume_completes_byte_exact` | PASS |
| S09-P15 | edge: FTP no SIZE | Fixture with SIZE disabled | Probe reports unknown size; download still completes byte-exact (open-ended) | cargo test `ftp_download_without_size_is_open_ended_and_byte_exact` | PASS |
| S09-P16 | edge: FTP + HTTP proxy | Only an HTTP proxy configured; add `ftp://` URL | FTP goes direct; HTTP proxy never contacted (FTP through an HTTP proxy is out of scope) | cargo test `ftp_download_goes_direct_when_only_an_http_proxy_is_configured` | PASS |
| S09-P17 | happy: NTLM proxy | Local stub proxy demanding NTLM; http:// download with `DOMAIN\alice` + password | Type 1/2/3 handshake; download byte-exact | cargo test `ntlm_proxy_authenticates_http_download` | PASS |
| S09-P18 | negative: NTLM wrong password | Same stub, wrong password | Probe fails with 407, no download created | cargo test `ntlm_proxy_wrong_password_is_rejected` | PASS |
| S09-P19 | happy: NTLM Test button | `test_proxy` against the NTLM stub, right then wrong password | Right password returns 200; wrong password is an error | cargo test `ntlm_proxy_test_button_succeeds_and_wrong_password_fails` | PASS |
| S09-P20 | edge: Kerberos only | Stub offering only `Negotiate` | Kerberos is now attempted through the real GSSAPI stack and fails with a clear error naming Kerberos ("No credentials were supplied…"); nothing is sent, never a faked handshake — see S09-P26 for the manager-level case | cargo test `kerberos_only_proxy_is_refused_clearly` | PASS |
| S09-P21 | edge: NTLM + https | `test_proxy` with an https:// URL through the NTLM stub | Clear "only supported for http:// URLs" error (TLS tunnel after the handshake is not built) | cargo test `ntlm_proxy_https_test_url_is_refused_clearly` | PASS |
| S09-P22 | happy: Test FTP | `test_proxy` with an ftp:// URL through a draft FTP proxy, and refusal cases (FTP proxy + http URL, HTTP proxy + ftp URL) | ftp URL returns 200; the refusal cases are clear errors | cargo test `test_proxy_ftp_url_through_ftp_proxy_and_http_url_refusals` | PASS |
| S09-P23 | note: independent check | libcurl `--proxy-ntlm` against the same stub (correct + wrong password) | Correct password: 200, 512 KiB byte-exact; wrong password: 407 — the stub's Type 2 / Type 3 verification is validated by an independent NTLM client, not only by the engine | manual curl run (see build-log S09 proxy finish) | PASS |
| S09-P24 | happy: Kerberos handshake (scripted) | Scripted initiator + stub demanding `Negotiate`; GET, ranged GET, HEAD; a 407 token passed as the first step's input | Byte-exact 512 KiB; ranged GET 206 + first 1 KiB with `Content-Range: bytes 0-1023/524288`; HEAD 200 with Content-Length; stub saw a well-formed NegTokenInit + NegTokenResp (validated by the stub's own DER code); the stub's response token was fed into step 2 | cargo test `kerberos_proxy_authenticates_http_request`, `kerberos_proxy_ranged_get_and_head_work`, `kerberos_proxy_initial_407_token_reaches_the_first_step` | PASS |
| S09-P25 | negative: Kerberos no credentials (scripted) | Failing scripted initiator + Negotiate-only stub | Clear Kerberos error; no Basic sent, no Negotiate token sent, nothing forwarded | cargo test `kerberos_proxy_failing_initiator_is_a_clear_kerberos_error` | PASS |
| S09-P26 | edge: Kerberos only, real GSSAPI | Manager `add_download` + Test button through a Negotiate-only stub (no ticket on this VM) | Clear Kerberos error; no Basic header, no Negotiate token; no download created | cargo test `kerberos_only_proxy_fails_clearly_with_real_gssapi_and_never_sends_basic` | PASS |
| S09-P27 | happy: Kerberos → Basic fallback | Stub offering Negotiate + Basic; real GSSAPI fails (no ticket) | Download byte-exact via Basic; Test button returns 200; a Basic header was sent | cargo test `kerberos_falls_back_to_basic_when_no_ticket` | PASS |
| S09-P28 | happy: Kerberos → NTLM fallback | Stub offering Negotiate + NTLM; real GSSAPI fails (no ticket) | Download byte-exact via NTLM (Type 1 + Type 3 seen by the stub); Test button returns 200 | cargo test `kerberos_falls_back_to_ntlm_when_no_ticket` | PASS |
| S09-P29 | negative: explicit auth never downgrades | `auth=kerberos` + Basic-only stub; `auth=ntlm` + Negotiate-only stub | Errors name Kerberos / NTLM; no Basic sent in the first case, no Negotiate token and no NTLM Type 1 in the second | cargo test `kerberos_auth_setting_never_downgrades_to_basic`, `ntlm_auth_setting_never_uses_kerberos` | PASS |
| S09-P30 | edge: Kerberos + https | `test_proxy` with an https:// URL through the Negotiate stub | Clear error naming Kerberos and https (TLS tunnel after the handshake is not built) | cargo test `kerberos_https_test_url_is_refused_clearly` | PASS |
| S09-P31 | happy: auth setting storage | Save `auth=kerberos` → round-trip; save without `auth` keeps it; invalid value rejected; Kerberos + SOCKS and NTLM + FTP rejected; pre-Kerberos DB (no `auth` column) migrates to `negotiate-auto` | As stated | cargo test `proxy_auth_setting_round_trip_and_validation`, `proxy_auth_column_migrates_on_old_databases` | PASS |
| S09-P32 | happy: cookie probe + Basic proxy | Cookie-gated 128 KiB file behind a demanding Basic proxy, default `negotiate-auto` | The cookie probe answers the 407 with Basic + cookies; download byte-exact; proxy used | cargo test `cookie_gated_download_behind_basic_proxy_still_works` | PASS |
| S09-P33 | negative: cookie probe + Negotiate-only | Cookie-gated link behind a Negotiate-only stub | Clear Kerberos error (the raw handshake cannot carry cookies); no download created | cargo test `cookie_gated_download_behind_kerberos_only_proxy_fails_clearly` | PASS |
| S09-P34 | note: what the stub does not prove | — | The stub proves SPNEGO framing + handshake mechanics, not a real domain login; no KDC was ever contacted; the SSPI (Windows) path is written, never compiled | — | DOC |

### S09-I — Integration: settings ↔ engine ↔ queues/categories/scheduler

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-I1 | happy | Queue with max_connections = 1 (2 MiB files), then a capped drain (2 × 128 KiB at 64 KiB/s) | 1 segment per file; capped drain takes ≥ 3 s; all byte-exact | cargo test `queue_drain_honours_max_connections_and_speed_limit` | PASS |
| S09-I2 | happy | Complete a file in a category, edit the category's folder | Finished file stays at its old path; a new download uses the new folder | cargo test `category_folder_edit_does_not_move_completed_files` | PASS |
| S09-I3 | happy | Schedule a queue (start in the past) with max_connections = 2 and a 256 KB/s cap set | Scheduler tick drains the queue | cargo test `scheduler_tick_drains_queue_with_settings_applied` | PASS |
| S09-I4 | note | A proxy change applies to new downloads and to segments created by a dynamic split; a segment already running keeps the client it started with (code review of `manager.rs`: `run_segment` holds the `Arc<Client>` it was spawned with) | — | code review | DOC |

### S09 — file_types_auto / excluded_sites (BUG-011 fix pass, 2026-11-14)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-B1 | happy | `file_types_auto = []`, add .zip | Auto-assigns to Compressed (old behaviour) | cargo test `file_types_auto_limits_auto_assign` | PASS |
| S09-B2 | happy | `file_types_auto = ["mp4"]`, add .mp4 then .zip | .mp4 → Video; .zip → Documents (not Compressed) | cargo test `file_types_auto_limits_auto_assign` | PASS |
| S09-B3 | negative | `excluded_sites = ["127.0.0.1"]`, add a download from the mock server | Refused before any probe, "excluded in Settings" | cargo test `excluded_sites_blocks_add_download` | PASS |
| S09-B4 | negative | `excluded_sites = ["example.com"]`, add `http://sub.example.com/file.bin` | Refused (subdomain match), no network attempt | cargo test `excluded_sites_blocks_add_download` | PASS |
| S09-B5 | negative | Excluded page URL in Download All | Refused with "excluded" before fetching | cargo test `fetch_page_links_flags_excluded_sites` | PASS |
| S09-B6 | happy | Page with one excluded link and one clean link | Excluded link listed with an "excluded" error (never probed); clean link probed normally | cargo test `fetch_page_links_flags_excluded_sites` | PASS |
| S09-B7 | negative | Batch with one excluded line (line 2) | Line 2 reported with its number and "excluded"; line 1 added | cargo test `batch_add_flags_excluded_site_lines` | PASS |
| S09-B8 | happy | Save `file_types_auto = ["MP4", ".mkv", ""]`, `excluded_sites = ["Example.COM ", "https://foo.org/some/path"]` | Stored normalised as `["mp4", "mkv"]` and `["example.com", "foo.org"]` | cargo test `update_settings_normalizes_file_types_and_excluded_sites` | PASS |

### Regression (round 1 areas, re-run in full)

- Full suite: **98 integration + 12 unit, 0 failed, 2 consecutive
  full runs** (was 81 + 12 before this round; +17 integration).
- Round-1 areas spot-checked by the passing suite: add/pause/resume
  (F02/F03), categories (F10), queues/scheduler/actions (F04),
  Download All + batch (S07/S10), proxy + limiter (S09/S12).
- BUG-005 (malformed schedule time) and BUG-006 (stop before start):
  re-checked — the two DOC tests still pass, so both are **still
  present**, unchanged by milestone 4.
- BUG-007 (sidebar delete-category button mouse-only): re-checked —
  `CategorySidebar.tsx:111` still `hidden … group-hover:block` with
  no `focus-within`; the queue Start button at `:157` has the same
  pattern. **Still present.**

### Not tested in this round (same limits as round 1)

- No desktop binary / Tauri shell: `cargo check` in `app/src-tauri`
  still fails on this VM (missing `gdk-3.0`); all 35 Tauri commands
  are uncompiled, and the 6 manual UI cases above are blocked.
- No Playwright e2e specs: Playwright/Chromium are not installed on
  this VM. The Vite preview smoke test (index 200, `Fetchline`
  title, JS asset 200) is the UI check that was possible.
- DPAPI password storage on Windows: the DPAPI path only runs there
  and has never run; on this VM passwords are base64-obfuscated
  only (documented in `secret.rs` / build-log.md).
- Resume across a process restart, shutdown/hibernate execution on
  Windows, periodic sync (deferred by design).

## Round-2 summary

- Automated engine cases: 98 integration + 12 unit, all passing
  (2 consecutive full runs, 0 failures). 17 new integration tests.
- UI build: `npm run build` passes; Vite preview serves the app.
- Manual / blocked: 6 new cases (Settings dialog, defaults,
  status-bar button, proxy Test, theme, status-bar cap) — all need
  the Windows desktop binary.
- Bugs found: 0 × S1, 0 × S2, 3 × S3 (BUG-008–010), 1 × S4 (BUG-011).
  BUG-005/006/007 re-checked, still open. See `bugs.md`.

## Bug-fix pass — BUG-008–011 fixed (2026-11-14)

Failing-test-first, per bug (see `bugs.md` for the fixes):
- BUG-008: `speed_limit_absurd_value_is_rejected` — S12-E1 now PASS.
- BUG-009: `test_proxy_wrong_credentials_is_an_error` — S09-P7 now PASS.
- BUG-010: `test_proxy_https_only_scope_rejects_http_test_url`,
  `test_proxy_http_scope_uses_proxy_for_http_url`,
  `test_proxy_ftp_type_is_rejected` — S09-P6 now PASS, plus P9/P10.
- BUG-011: `file_types_auto_limits_auto_assign`,
  `excluded_sites_blocks_add_download`,
  `fetch_page_links_flags_excluded_sites`,
  `batch_add_flags_excluded_site_lines`,
  `update_settings_normalizes_file_types_and_excluded_sites` — S09-B1–B8.
- Full suite after the fixes: **106 integration + 12 unit, 0 failed,
  2 consecutive full runs** (was 98 + 12; +8 net — the two DOC tests
  for BUG-009/010 were converted, not duplicated). `npm run build`
  passes. BUG-005/006/007 untouched and still open.

## Bug-fix pass — BUG-005–007 fixed (2026-11-14)

Failing-test-first, per bug (see `bugs.md` for the fixes):
- BUG-005: `update_queue_rejects_malformed_start_time`,
  `update_queue_rejects_malformed_stop_time`,
  `scheduler_tick_reports_legacy_malformed_schedule_but_runs_valid_queues`
  (legacy rows are now reported by the tick, not silently skipped),
  plus the timeutil unit test `rejects_impossible_dates` — F04-S9
  now FAIL→FIXED.
- BUG-006: `update_queue_rejects_stop_before_start`,
  `update_queue_rejects_stop_equal_to_start_and_stop_without_start`
  — F04-S10 now FAIL→FIXED. Engine and UI now use the same message:
  "The stop time must be after the start time."
- BUG-007: no automated test is possible without a desktop binary
  (no Playwright on this VM). Verified by source check (no
  `hidden`/`group-hover:block` left on either sidebar button) and by
  the built CSS containing
  `.group:focus-within .group-focus-within\:opacity-100{opacity:1}`
  and `.focus-visible\:opacity-100:focus-visible{opacity:1}`. A real
  keyboard walk-through still needs the desktop binary.
- Full suite after the fixes: **109 integration + 13 unit, 0 failed,
  2 consecutive full runs** (was 106 + 12; +3 integration, +1 unit —
  the two DOC tests for BUG-005/006 were converted, not duplicated).
  `cargo check --tests` warning-free. `npm run build` passes.
  No open bugs remain in `bugs.md`.

## Round 3 — Browser extension (S13 / F01) — 2026-11-14

Extends the round-1/2 plan; it does not replace it. Build: slices
1–5 plus S13 (MV3 extension, native host, `pending_links` queue,
drain through the real `add_download`). Env: same Linux VM, local
mock HTTP / page servers only — no public internet, no IDM
servers, no browser automation (no headless Chromium works on
this VM; no Playwright). Engine: `cargo test` in `app/engine`
(109 integration + 13 unit + 21 extension tests, 0 failed, 2
consecutive full runs — 19 extension tests at build time, +2
regression tests added in the Phase 2 verification round for
BUG-012). UI: `npm run build` passes; `npm run test:extension`
passes; `node --check` passes on `background.js` / `popup.js`.
The real `fetchline-host` binary was driven with
native-messaging frames against a local HTTP server (27 smoke
checks; see `bugs.md` Round 4). No desktop binary: the Tauri
shell still does not compile here (missing `gdk-3.0`), so the
`drain_pending_links` command and the startup/30 s drain calls
are uncompiled, and every case needing the running desktop app
is marked **blocked**.

### F01 — One-click browser download (S13)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F01-H1 | happy | Extension sends `download_link` (valid URL, mock server) → drain | Row in `pending_links`, then a real download row, status `queued`, category auto-assigned | cargo test `drain_picks_up_enqueued_link_via_add_download` + host smoke A3/A4 + scratch drain (7 accepted) | PASS |
| F01-H2 | happy | `download_all` with links → drain | Each link enqueued, then drained | cargo test `host_download_all_with_links_enqueues_each` + smoke A7 | PASS |
| F01-H3 | happy | `download_all` with no links (extension has no content script) → host expands page | Page fetched, live links enqueued, dead link skipped | cargo test `enqueue_page_links_expands_a_page_into_pending_rows` + smoke A8 | PASS |
| F01-H4 | happy | `ping` / `get_settings` | Pong; desktop flag + excluded list reported | cargo test `host_ping_and_get_settings` + smoke A1/A2 | PASS |
| F01-E1 | edge: duplicate URL | Same URL sent twice → drain | Both accepted, second save path suffixed ` (2)` (BUG-002 guard) | cargo test `drain_duplicate_url_gets_different_save_path` + scratch drain (`(2)`/`(3)`/`(4)`) | PASS |
| F01-E2 | edge: dead link in the queue | Dead link + good link → drain | Dead link marked `rejected` with the reason, good link still accepted | cargo test `drain_marks_dead_link_rejected_and_keeps_going` + scratch drain (404 rejected, 7 accepted) | PASS |
| F01-E3 | edge: double drain | Drain twice | Second drain accepts/rejects nothing (rows are decided, never re-accepted) | scratch drain test (deleted after the run; logic pinned by `mark_pending_link`'s pending-only UPDATE) | PASS |
| F01-E4 | edge: pre-S13 database | Open a DB file made before S13 (no `pending_links` table) with the host | Table added by migration, existing downloads/settings/categories intact | host smoke migration check (python-built pre-S13 DB, ping, table exists, rows intact) | PASS |
| F01-E5 | edge: malformed frames | Oversized length, truncated payload, malformed JSON, unknown type, zero-length frame | Oversized/truncated exit promptly (rc 0, no hang); malformed JSON / unknown type / zero-length get a framed error reply and the host stays alive | host smoke D1–D5 (D1 was FAIL→FIXED, BUG-013; D5 needs the pipe closed — an open pipe correctly waits for the rest of the frame) | FAIL→FIXED (BUG-013) |
| F01-E6 | edge: locked database | Host started while the DB file holds an exclusive lock | Fails cleanly, no hang (observed: ~5 s rusqlite busy timeout, then exit 1 with "database is locked" on stderr) | host smoke locked-DB check | PASS (DOC) |
| F01-E7 | edge: missing database | Host pointed at a path in a non-existent directory | Parent directories created, fresh DB opened | host smoke fresh-nested-DB check | PASS |
| F01-N1 | negative: excluded site | `download_link` / `download_all` (empty links) to an excluded site | Refused at the host boundary, nothing enqueued; a row enqueued before the exclusion is rejected at drain time | cargo test `host_rejects_excluded_site_without_enqueueing`, `drain_rejects_excluded_site_at_drain_time` + smoke B1–B4 | PASS |
| F01-N2 | negative: integration off | `download_link` / `download_all` (with and without links) while `browser_integration` is off | Refused, nothing enqueued; rows enqueued before the switch-off are rejected at drain time | cargo test `host_rejects_when_browser_integration_is_off`, `drain_rejects_when_browser_integration_turned_off` + smoke C1/C4 | PASS |
| F01-N3 | negative: invalid URL | `download_link` with `not a url`, `ftp://…` | Refused, nothing enqueued | cargo test `host_rejects_invalid_url_without_enqueueing` + smoke A5/A6 | PASS |
| F01-N4 | negative: video | `video_found` message | Refused with a clear "not built yet" error, nothing enqueued (F05 is a later milestone) | cargo test `host_video_found_is_not_built_yet` + smoke A9 | PASS |
| F01-N5 | negative: Download All bypass | `download_all` with no links while integration is off | Refused, nothing enqueued (was: `ok:true, enqueued:2` — the off switch was bypassed on this one path) | cargo test `enqueue_page_links_refuses_when_browser_integration_is_off` + smoke C2 | FAIL→FIXED (BUG-012) |
| F01-N6 | negative: all links skipped | `download_all` with links, every link refused | Error reply, not `ok:true` with `enqueued:0` (was: success reported, user never told) | cargo test `host_download_all_all_skipped_is_an_error` + smoke C3 | FAIL→FIXED (BUG-012) |
| F01-N7 | negative: cookies | `download_link` carrying a `cookies` field | Field ignored, link queued; no cookie ever stored or logged | smoke A10 (host never logs a message body — code review of `main.rs` / `native_host.rs`) | PASS |
| F01-M1 | manual: popup | Open the popup, toggle catch, opt a site out, "Send this page" | Status, toggle state, per-site opt-out behave as in `popup.js` | blocked — no browser automation on this VM (code reviewed: `popup.js`, `popup.html`) | BLOCKED |
| F01-M2 | manual: context menu + watcher | Right-click a link, start a browser download with catching on | Menu hands the URL over; watcher cancels the browser download and hands it over | blocked — no browser automation on this VM (code reviewed: `background.js`) | BLOCKED |
| F01-M3 | manual: drain in the desktop app | Start the desktop app, send a link, wait ≤ 30 s | Download appears queued in the main window | blocked — no desktop binary; the Tauri `drain_pending_links` command and the startup/30 s drain calls are uncompiled (`gdk-3.0`) | BLOCKED |
| F01-T1 | to check: Windows handshake | Registry install of `com.fetchline.host.json`, extension ID in `allowed_origins`, first drain against the real desktop app's DB on Windows | Handshake works against the real app-data folder | — | TO CHECK (Windows) |
| F01-T2 | to check: download watcher cancel-before-handoff | Hand-off fails after the browser download was cancelled | Browser download is gone; user must right-click and retry (documented in `background.js`; no auto-retry by design) | — | TO CHECK (browser) |

### Manifest / permissions (verified by reading + test)

`manifest.json` is MV3 with exactly `activeTab`, `contextMenus`,
`downloads`, `storage`, `nativeMessaging` — no `<all_urls>`, no
`host_permissions`, no `cookies`, no `tabs`, no `scripting`, no
`webRequest`, no content script. Verified by
`npm run test:extension` (Python validator), by cargo test
`extension_manifest_is_mv3_and_minimal`, and by reading the file.
Icons 16/32/48/128 PNG + SVG all present.

### Regression (rounds 1–2 areas, re-run in full)

- Full suite: **109 integration + 13 unit + 21 extension, 0
  failed, 2 consecutive full runs**. BUG-002 (`duplicate_add_gets_different_save_path`), BUG-005 (`update_queue_rejects_malformed_start_time`), BUG-006 (`update_queue_rejects_stop_before_start`), BUG-008 (`speed_limit_absurd_value_is_rejected`), BUG-011 (`excluded_sites_blocks_add_download`, `file_types_auto_limits_auto_assign`) all pass by name in the output.
- `cargo check --tests` warning-free (0 warnings).

### Not tested in this round (same limits as rounds 1–2, plus S13)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `drain_pending_links` command is uncompiled, and F01-M1–M3 are
  blocked. The extension JS has never executed in a real browser
  here — popup, context menu and download watcher are verified
  by code review + `node --check` + the host-side protocol
  tests only.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- The Windows handshake (registry install, extension ID,
  app-data folder) is unproven — the host was tested on this
  Linux VM only, against the same SQLite file the engine uses.

## Round-3 summary

- Automated engine cases: 109 integration + 13 unit + 21
  extension, all passing (2 consecutive full runs, 0 failures).
  21 new extension tests (19 at build, +2 regression tests for
  BUG-012 in verification).
- Host smoke test: 27 checks against the real binary (26 pass
  after the BUG-012/013 fixes; the 27th, D5 truncated-payload,
  was a test artifact — the host correctly waits on an open
  pipe and exits at once on EOF, verified separately).
- UI build: `npm run build` passes; `npm run test:extension`
  passes; `node --check` passes on both extension scripts.
- Manual / blocked: 3 new cases (popup, context menu + watcher,
  drain in the desktop app) — all need a real browser / the
  Windows desktop binary.
- Bugs found: 1 × S2 (BUG-012), 1 × S4 (BUG-013). Both fixed
  failing-test-first, tests kept. See `bugs.md`.

---

## Round 4 — Video grabber (S08 / F05) — 2026-11-14

Extends the round-3 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 (extension detector + floating button/panel,
`video_found` host path, desktop video panel). Env: same Linux VM,
local mock HTTP / page servers only — no public internet, no IDM
servers, no browser automation (no headless Chromium works on
this VM; no Playwright). Engine: `cargo test` in `app/engine`
(109 integration + 13 unit + 21 extension + 21 video tests, 0
failed). UI: `npm run build` passes; `npm run test:extension`
passes; `node --test extension/tests/video-detect.test.js`
(11 passed, 0 failed); `node --check` passes on all four
extension scripts. The real `fetchline-host` binary was driven
with native-messaging frames against a local HTTP server serving
a real .mp4 fixture (11/11 smoke checks, `app/host/video-smoke.py`).
No desktop binary: the Tauri shell still does not compile here
(missing `gdk-3.0`), so the `detect_page_videos` command is
uncompiled, and every case needing the running desktop app is
marked **blocked**.

### F05 — Capture a video from a web page (S08)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F05-H1 | happy | `video_found` with a good .mp4 variant → drain | Row in `pending_links` (source `video`), then a real download row, category Video; the drain starts it and it completes byte-exact (Milestone 2: no longer left queued) | cargo test `video_found_end_to_end_drains_into_a_real_download` + host smoke V2 | PASS |
| F05-H2 | happy | Start the drained video download | Completes byte-exact from the fixture (100 KiB) | same test (start + wait complete + byte compare) | PASS |
| F05-H3 | happy | Scan a page with video + audio + dead video | Video (MP4, size, downloadable), audio (MP3), dead link flagged with error and not downloadable | cargo test `detect_page_videos_lists_media_with_probe_results` | PASS |
| F05-H4 | happy | `video_found` with several qualities, one picked | Only the chosen variant is enqueued (extension sends `variants: [chosen]`) | host smoke V2 (enqueued 1) + code review of `content.js` `sendVariant` | PASS |
| F05-E1 | edge: multiple qualities | Page with `<video>` + two `<source>` qualities | All qualities listed, each with its own label (720p / 480p / 1080p) | cargo test `fixture_video_with_sources_and_multiple_qualities` (Rust + JS) | PASS |
| F05-E2 | edge: audio only | Page with only an `<audio>` element | One MP3 found, quality "Unknown quality" | cargo test `fixture_audio_only` (Rust + JS) | PASS |
| F05-E3 | edge: no video | Page with no media | No videos found; panel shows the empty state | cargo test `fixture_no_video` (Rust + JS) | PASS |
| F05-E4 | edge: duplicate URLs | Same clip as `<video src>`, `<source>` and link | Listed once | cargo test `fixture_duplicates_are_deduped` (Rust + JS) | PASS |
| F05-E5 | edge: live stream | Page with an .m3u8 stream | Listed as live (M3U8), offered as a playlist file with a note, never as a merged video | cargo test `fixture_live_stream_is_marked_live` + `video_validation_notes_hls_playlists_and_live_streams` | PASS |
| F05-E6 | edge: only low quality | Clip with no dimensions and no label | Quality "Unknown quality" — never invented | cargo test `quality_is_unknown_without_evidence` (Rust + JS) | PASS |
| F05-E7 | edge: pre-S08 database | Open a DB made before S08 (old `pending_links` CHECK) | CHECK widened by migration, old row preserved, a `video` row can be enqueued | cargo test `pre_s08_database_migrates_pending_links_to_allow_video_source` | PASS |
| F05-N1 | negative: DRM | `video_found` with a DRM variant | Refused with a clear DRM error, nothing enqueued | cargo test `video_validation_refuses_drm` + `host_video_found_all_skipped_is_an_error` + host smoke V3 | PASS |
| F05-N2 | negative: bad scheme | `video_found` with an `ftp://` variant | Skipped; a good variant in the same message still queues | cargo test `video_validation_skips_bad_scheme_and_unknown_extension` + host smoke V4 | PASS |
| F05-N3 | negative: unknown extension | `video_found` with a `.html` variant | Skipped, not queued | same test | PASS |
| F05-N4 | negative: empty variants | `video_found` with `variants: []` | Error, nothing enqueued | cargo test `video_validation_rejects_empty_variants_excluded_page_and_integration_off` + host smoke V5 | PASS |
| F05-N5 | negative: excluded site | `video_found` from an excluded page / variant host | Refused at the host boundary, nothing enqueued | same test + host smoke V6 | PASS |
| F05-N6 | negative: integration off | `video_found` while `browser_integration` is off | Refused, nothing enqueued | same test | PASS |
| F05-N7 | negative: invented quality | Variant claims "Ultra HD 8K Pro" | Quality replaced with "Unknown quality"; a real "720p" claim is kept | cargo test `video_validation_replaces_an_invented_quality_and_keeps_a_real_one` | FAIL→FIXED (BUG-015) |
| F05-N8 | negative: all skipped | `video_found` where every variant is refused | Error reply, not `ok:true` with `enqueued:0` (BUG-012 rule) | cargo test `host_video_found_all_skipped_is_an_error` | PASS |
| F05-N9 | negative: file types allow-list | `file_types_auto = ["mp4"]`, variant is `.webm` | Skipped with a clear reason; the `.mp4` variant still queues | cargo test `video_validation_skips_variant_from_excluded_site_and_outside_file_types_auto` | PASS |
| F05-M1 | manual: floating button + panel | Open the popup, "Find videos on this page", hover a video, pick a quality | Button appears on hover, panel lists qualities, picking one queues the video | blocked — no browser automation on this VM (code reviewed: `content.js`, `popup.js`, `background.js`) | BLOCKED |
| F05-M2 | manual: desktop panel | Toolbar "Video grabber", paste a page link, Download / Later | Panel lists videos; Download opens S02 prefilled, Later queues | blocked — no desktop binary; the `detect_page_videos` command is uncompiled (`gdk-3.0`) | BLOCKED |
| F05-M3 | manual: drain in the desktop app | Send a video from the browser, wait ≤ 30 s | Download appears queued in the main window | blocked — no desktop binary (same drain as F01-M3) | BLOCKED |
| F05-T1 | to check: Windows handshake for video | First `video_found` against the real desktop app's DB on Windows | Enqueue + drain work against the real app-data folder | — | TO CHECK (Windows) |
| F05-T2 | to check: DRM detection in the desktop panel | Desktop panel on a page with a DRM video | The probe fails and the panel shows the error (static HTML cannot detect DRM) | — | TO CHECK (desktop) |

### Detection / quality (shared by both detectors)

The Rust detector (`engine/src/video.rs`) and the JS detector
(`extension/video-detect.js`) run the same rules over the same
fixtures (`extension/tests/fixtures/`): `<video src>`, `<source>`
children, `<audio src>`, directly linked media; dedup by URL;
quality from real dimensions, then a `720p`/`1080p`/`4K` label in
the URL/file name, else "Unknown quality". The live-DOM scan
additionally reads `duration === Infinity` (live) and `mediaKeys`
(DRM); the static scan reads the fixtures' `data-live` /
`data-drm` markers instead. Two detector bugs were found by the
tests and fixed (BUG-014: `.m3u8` not classified as Video, so a
live stream was invisible; BUG-016: `<source>` children inherited
the parent element's dimensions, inventing qualities).

### Regression (rounds 1–3 areas, re-run in full)

- Full suite: **109 integration + 13 unit + 21 extension + 21
  video, 0 failed** (was 109 + 13 + 21; +21 video, +4 unit in
  `video.rs`). `cargo check --tests` warning-free.
- S13 regression: `download_link` still enqueues (extension
  tests 21/21 + host smoke V7); BUG-002/005/006/008/011/012
  regression tests all pass by name in the output.
- `npm run build` passes; Vite preview serves the app (index 200,
  `Fetchline` title).

### Not tested in this round (same limits as round 3, plus S08)

- No real browser: the popup scan, floating button and panel have
  never executed in Chrome/Firefox/Edge here — verified by code
  review + `node --check` + the Node fixture tests only.
- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `detect_page_videos` command and the S02 `initialUrl` prefill
  are uncompiled, and F05-M1–M3 are blocked.
- The Windows handshake (registry install, extension ID,
  app-data folder) is unproven — the host was tested on this
  Linux VM only, against the same SQLite file the engine uses.

## Round-4 summary

- Automated engine cases: 109 integration + 13 unit + 21
  extension + 21 video, all passing. 21 new video tests (+4 unit
  in `video.rs`).
- Extension JS tests: 11 passed, 0 failed (`node --test`).
- Host smoke test: 11 checks against the real binary, all pass
  (`app/host/video-smoke.py`).
- UI build: `npm run build` passes; `npm run test:extension`
  passes; Vite preview serves the app.
- Manual / blocked: 3 new cases (floating button + panel, desktop
  panel, drain in the desktop app) — all need a real browser / the
  Windows desktop binary.
- Bugs found: 3 × S3 (BUG-014/015/016). All fixed
  failing-test-first, tests kept. See `bugs.md`.

---

## Round 5 — Site grabber (S06 / F06) — independent verification, 2026-11-14

Build: slices 1–5 plus S13 plus S08 plus S06. Env: this Linux VM,
local fixture servers only — no real website was ever crawled. New
verifier tests: `app/engine/tests/grabber_verify_tests.rs` (14
tests, counting fixture with per-request timestamps). Shipped
grabber tests: `app/engine/tests/grabber_tests.rs` (15 tests).
Result key: PASS = ran and passed · FAIL = reproduced, not fixed
(see `bugs.md`) · BLOCKED = needs the desktop binary.

Fix round 2026-11-15: BUG-017/018/019 FIXED — the verifier suite
now passes 14/14 on three consecutive runs and the full engine
suite is 201/201, 0 failed (21 unit + 109 integration + 21
extension + 15 grabber + 14 verifier + 21 video). See `bugs.md`
Round 6 and `build-log.md` S06 for the fixes.

### F06 — Site grabber: download a site / gallery (S06)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F06-H1 | happy | Wizard: name + start URL, images+documents, depth 2, Run | Matched files listed, downloads complete, rows tagged with `grabber_project_id` | cargo test `download_mode_downloads_matched_files_and_tags_them` | PASS |
| F06-H2 | happy | Explore-only run | All matched files listed, zero downloads created | cargo test `explore_only_lists_matched_files_without_downloading`; verifier `verify_off_site_page_linking_back_is_never_followed` (also asserts `list_downloads` empty) | PASS |
| F06-H3 | happy | Depth 0 | Start page only: 1 page, its files only | cargo test `depth_limit_is_enforced` | PASS |
| F06-H4 | happy | Depth 5 accepted, depth 6 rejected | Depth 5 runs all 7 fixture pages; depth 6 create fails "too deep" | verifier `verify_depth_5_accepted_depth_6_rejected` | PASS |
| F06-H5 | happy | Type filter = images only | Only `pic.png` matched | cargo test `type_filter_only_matches_selected_types` | PASS |
| F06-H6 | happy | Page cap 2 / 3 | Status `done` (clean stop, not failure), exactly 2 / 3 pages fetched, robots.txt fetched once | cargo test `max_pages_cap_stops_cleanly`; verifier `verify_page_cap_stops_cleanly_without_extra_fetches` (also counts requests) | PASS |
| F06-H7 | happy | Scheduled project, past start time | Scheduler tick starts it; future schedule stays `idle` | cargo test `scheduled_project_starts_on_the_scheduler_tick` | PASS |
| F06-H8 | happy | Re-run a finished project | Second run allowed after the first finishes | cargo test `run_grabber_twice_is_rejected_while_running` | PASS |
| F06-H9 | happy | Re-run downloads get distinct save paths (BUG-002) | 10 downloads after 2 runs, 10 distinct paths, all `complete`, all tagged | verifier `verify_rerun_gives_grabbed_files_distinct_save_paths` | PASS |
| F06-E1 | edge: robots Allow under Disallow, same group | `Disallow: /private/` + `Allow: /private/open/` in one `*` group | Open sub-path crawled, rest of `/private/` blocked | verifier `verify_robots_allow_under_disallow_same_group` | PASS |
| F06-E2 | edge: robots Allow in Fetchline group vs Disallow in `*` group | `*` group first, then Fetchline `Allow: /private/open/` | Open sub-path crawled | verifier `verify_robots_fetchline_allow_beats_star_disallow_both_orders` + `verify_robots_allow_under_disallow_cross_group_crawl` | PASS (BUG-017 FIXED 2026-11-15) |
| F06-E3 | edge: robots blocks the start page | Start URL under `Disallow: /` | Status `failed`, error names robots.txt, 0 pages | cargo test `robots_disallow_blocks_the_start_url`; verifier `verify_robots_start_url_under_disallow_fails_but_allowed_subpath_runs` (also: allowed sub-path start runs) | PASS |
| F06-E4 | edge: off-site page links back | Index links to an off-site page that links back to a same-host page + file | Off-site server gets 0 requests; the back-linked file never matched | verifier `verify_off_site_page_linking_back_is_never_followed` | PASS |
| F06-E5 | edge: polite delay, measured | 6-page crawl, timestamps on every page fetch | Consecutive fetches ≥ 500 ms apart (measured smallest gap 504 ms) | verifier `verify_polite_delay_between_page_fetches_is_measured` | PASS |
| F06-E6 | edge: stop mid-crawl | Stop 600 ms into a 7-page crawl | Status `stopped`, < 7 pages, prompt (measured 508 ms) | cargo test `stop_grabber_records_stopped`; verifier `verify_stop_is_prompt` | PASS |
| F06-E7 | edge: dead file, exactly 2 attempts | `dead.pdf` 404s on probe (1 HEAD per direct add) | Exactly 2 probe requests in the grabber run, file recorded `failed` once, run still `done` | verifier `verify_dead_file_is_attempted_exactly_twice`; cargo test `two_attempts_then_the_grabber_gives_up_on_a_dead_file` | PASS |
| F06-N1 | negative: depth 6 / page cap 1001 / no types / bad URL / empty name | Create with each invalid input | Clear rejection, no project created | cargo test `create_rejects_depth_over_5_empty_types_and_bad_url` | PASS |
| F06-N2 | negative: excluded start site | Start URL on an excluded host | Rejected at create, 0 requests hit the server | cargo test `create_rejects_excluded_start_site`; verifier `verify_excluded_start_site_makes_no_request` (counts requests) | PASS |
| F06-N3 | negative: site excluded after create | Create, then exclude the host, then run | Run refuses before any fetch | verifier `verify_excluded_after_create_stops_the_run_before_any_fetch` | PASS (BUG-018 FIXED 2026-11-15) |
| F06-N4 | negative: login-required start page | Start URL answers 401 | Status `failed`, error names login, no cookies sent | cargo test `login_required_start_page_is_reported_not_skipped` | PASS |
| F06-N5 | negative: dead start page | Start URL 404s | Status `failed`, error names 404 | cargo test `dead_start_page_fails_with_the_status` | PASS |
| F06-N6 | negative: run twice while running | Second `run_grabber` while the first runs | Rejected "already running" | cargo test `run_grabber_twice_is_rejected_while_running` | PASS |
| F06-N7 | negative: re-run reads stale status | Read back immediately after re-running a `done` project | Reads `exploring`/`downloading`, never stale `done` | verifier `verify_rerun_status_is_stale_immediately_after_run` | PASS (BUG-019 FIXED 2026-11-15, 3 consecutive runs) |
| F06-M1 | manual: wizard click-through | All 3 steps, Run, live progress, Stop, finished/failed/stopped states | Each state renders; Stop works; counts update | blocked — no desktop binary; verified by source inspection + `npm run build` only | BLOCKED |
| F06-M2 | manual: BUG-019 UI symptom | Fresh run where the first poll lands before the task's first write | Dialog must start polling, not stick on idle | blocked — no desktop binary; timing-dependent | BLOCKED (engine race FIXED 2026-11-15 — synchronous `exploring` write removes the stale read; direct dialog observation still needs the binary) |
| F06-T1 | to check: stop during the download phase | Stop while a file is downloading | Waits for the current file by design (code comment); not measured | — | TO CHECK |
| F06-T2 | to check: huge real site | Crawl against a real website | Never attempted — local fixtures only, by rule | — | TO CHECK |

### Regression (rounds 1–4 areas, re-run in full)

| suite | cases | result |
| --- | --- | --- |
| engine lib unit tests | 21 (17 old + 4 in `grabber.rs`) | PASS, 0 failed (2 consecutive runs) |
| `engine_tests.rs` (S01–S05, S07 Download All, S10 batch, S09/S12) | 109 | PASS, 0 failed |
| `extension_tests.rs` (S13 link catch) | 21 | PASS, 0 failed |
| `video_tests.rs` (S08 video grabber) | 21 | PASS, 0 failed |
| `grabber_tests.rs` (S06 shipped) | 15 | PASS, 0 failed |
| extension JS: `npm run test:extension` (manifest) + `node --check` on background.js, content.js, popup.js, video-detect.js + `test:extension-video` | 11 Node tests | PASS, 0 failed |
| UI build: `npm run build` (tsc + vite) | — | PASS, 0 errors |
| `cargo check --tests` | — | PASS; only the known dead-code warnings in the `engine_tests` binary (GrabberSite unused there), same as S13/S08 |

No S07/S13/S08 source file changed in this slice: `links.rs`,
`native_host.rs`, `video.rs`, `extension_tests.rs`,
`video_tests.rs` all carry pre-S06 mtimes (21:24–21:30), while the
S06 files are 21:49–21:52; `manager.rs`/`store.rs` changed only
through the shared `add_download` path and the new grabber code, as
claimed. No git repo exists, so this is by mtime + content, not
`git diff`.

### UI check (S06 wizard, source inspection — no desktop binary)

- `SiteGrabberDialog.tsx` renders all 3 wizard steps (Project /
  What to grab / How to run it), the progress view (4 count cards,
  live matched-file list with per-file status/error, Stop button),
  and `done` / `failed` / `stopped` / `idle` states plus the
  page-limit note. Toolbar "Site grabber" button in `App.tsx`
  opens it.
- Keyboard: every input has a `<label>`, step-1 name field is
  autofocused on open, Escape and backdrop-click close, all
  actions are real `<button>`s, type/mode controls are native
  checkboxes/radios. (Same caveat as BUG-007's fix: verified by
  source, not by a keyboard walk-through — no binary.)
- Tokens only: no raw hex colour in the new component (`grep`:
  0 matches); it uses `bg-accent` / `border-border` / `text-danger`
  tokens like the rest of the UI.
- API wiring: `src/lib/api.ts` passes snake_case params matching
  the 5 Tauri commands in `src-tauri/src/lib.rs`
  (`create/run/stop/get/list_grabber_projects`, all registered);
  commands are uncompiled (`gdk-3.0`), as with every milestone.

### Not tested in this round (same limits as round 4, plus S06)

- The 5 new Tauri commands have never executed (uncompiled);
  the wizard is verified via `npm run build` + source only.
- The crawl has never run against a real website.
- Stop during the download phase (waits for the current file by
  design) was not measured.

## Round-5 summary

- Automated engine cases: shipped suite 187 (109 integration +
  21 unit + 21 extension + 15 grabber + 21 video), 0 failed, 2
  consecutive runs. Verifier's own adversarial suite: 14 tests,
  10 pass, 4 fail (BUG-017/018/019 — kept, not fixed).
- Extension JS tests: 11 passed, 0 failed (`node --test`).
- UI build: `npm run build` passes; `npm run test:extension`
  passes; `node --check` passes on all extension JS files.
- Manual / blocked: 2 new cases (wizard click-through, BUG-019
  UI symptom) — both need the Windows desktop binary.
- Bugs found: 3 × S3 (BUG-017/018/019). All OPEN, tests kept.
  See `bugs.md`.
- Parity: 58.3/100 (was 56.2), must-haves 10/17 (unchanged —
  site grabber is `partial`), media area 25.0 → 50.0.

---

## Round 6 — Periodic synchronisation (S05 / F04) — 2026-11-15

Extends the round-1/2 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync. Env: same
Linux VM, local mock HTTP servers only (`MockServer` in
`engine/tests/common/mock.rs`, extended with a `gone` toggle for
remote deletion and GET/HEAD request counters) — no public internet,
no IDM servers, no browser automation. Engine: `cargo test` in
`app/engine` (111 integration + 21 unit + 21 extension + 15 grabber
+ 14 verifier + 15 sync + 21 video = 218, 0 failed, 2 consecutive
full runs). UI: `npm run build` passes. No desktop binary: the
Tauri shell still does not compile here (missing `gdk-3.0`), so the
`sync_queue_now` command and the `update_queue` interval parameter
are uncompiled, and every case needing the running desktop app is
marked **blocked**.

### F04 — Periodic synchronisation (S05)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F04-Y1 | happy | Complete file in a periodic queue, sync | `unchanged: 1`, 0 GETs, 1 HEAD, file bytes + mtime untouched, file stays in the queue, last sync + counts stored on the queue | cargo test `sync_skips_unchanged_file_with_zero_get_requests` | PASS |
| F04-Y2 | happy | Server file grows + new ETag, sync | `changed: 1`, file re-downloaded byte-identical (new size + ETag stored), stays in the queue | cargo test `sync_redownloads_changed_file_byte_identical` | PASS |
| F04-Y3 | happy | ETag-only change (same size/bytes), sync | `changed: 1` — a validator change counts as changed | cargo test `sync_redownloads_on_etag_only_change` | PASS |
| F04-Y4 | happy | Two files, only one changes, sync | `changed: 1, unchanged: 1`; unchanged server sees 0 GETs, changed file re-downloaded | cargo test `sync_does_not_redownload_everything` | PASS |
| F04-Y5 | happy | Delete the local copy of a finished file, sync | `changed: 1`, file fetched again byte-identical | cargo test `sync_redownloads_a_file_whose_local_copy_is_gone` | PASS |
| F04-Y6 | edge: failed re-download | Change the file, fail GETs with 500, sync, then heal the server and sync again | First sync `failed: 1`, file in `error`, old file still on disk; second sync retries and records `changed: 1`, byte-identical | cargo test `sync_reports_failed_probe_and_retries_at_the_next_sync` | FAIL→FIXED (BUG-020) |
| F04-Y7 | edge: server unreachable | Server goes away after the download, sync | `failed: 1` with a message; row + local file untouched | cargo test `sync_reports_failed_probe_when_the_server_is_unreachable` | PASS |
| F04-Y8 | edge: remote-deleted | Server answers 404 after the download, sync | `removed: 1` with a 404 message; row + local file stay (reported, not dropped) | cargo test `sync_reports_remote_deleted_file_and_keeps_the_local_copy` | PASS |
| F04-Y9 | edge: excluded site | Exclude 127.0.0.1 after the download, sync | `skipped: 1` ("excluded"), 0 HEAD and 0 GET requests | cargo test `sync_skips_excluded_site_without_any_request` | PASS |
| F04-Y10 | edge: unfinished file | File in the sync queue never started, sync | `skipped: 1` ("not finished downloading yet"), still `queued` | cargo test `sync_skips_a_file_that_has_not_finished_downloading` | PASS |
| F04-Y11 | scheduler: due | Never-synced periodic queue, tick | Sync runs (last sync set); tick 1 min later does nothing; tick 31 min later syncs again | cargo test `sync_tick_runs_when_due_and_not_before_the_interval` | PASS |
| F04-Y12 | scheduler: window | Periodic queue with future start / past window / live window, tick | Future and past windows never sync; live window syncs | cargo test `sync_tick_respects_the_start_stop_window` | PASS |
| F04-Y13 | scheduler: no fight | Periodic queue syncs while a one-time queue drains | Sync completes, drain finishes normally and empties its queue, sync queue keeps its files | cargo test `sync_tick_does_not_fight_a_one_time_drain` | PASS |
| F04-Y14 | negative: interval | Save periodic with no interval, or 0/1/4/-30 min | Rejected ("how often" / "at least every 5 minutes"), nothing stored; 5 min accepted | cargo test `update_queue_rejects_periodic_sync_without_interval_or_too_often` | PASS |
| F04-Y15 | negative: exclusivity | Save one-time with an interval; sync a one-time queue; drain a periodic queue; add a finished file to a one-time queue | Interval on one-time rejected; sync on one-time errors; drain on periodic errors ("use Sync now"); finished file refused for one-time but accepted for periodic | cargo test `update_queue_rejects_interval_on_one_time_queue` + `sync_refuses_a_one_time_queue_and_start_queue_refuses_a_periodic_queue` | PASS |
| F04-Y16 | edge: migration | Open a DB made before S05/F04 (queues table without sync columns) | Sync columns added, old queue intact, periodic schedule saves | cargo test `pre_sync_database_migrates_queues_with_sync_columns` | PASS |
| F04-M4 | manual: dialog | Periodic radio, interval field, last-sync status, "Sync now" | Interval saves (≥ 5 enforced inline), last sync + counts shown, Sync now reports changed/unchanged/removed/failed | blocked — no desktop binary; the `sync_queue_now` command is uncompiled (`gdk-3.0`) | BLOCKED |
| F04-T1 | to check: Last-Modified | Real server sending Last-Modified but no ETag | Change detected via Last-Modified | — | TO CHECK (real website; the mock sends ETag + size only, so this path is code-only) |

### Regression (rounds 1–5 areas, re-run in full)

- Full suite: **111 integration + 21 unit + 21 extension + 15
  grabber + 14 verifier + 15 sync + 21 video = 218, 0 failed, 2
  consecutive full runs** (was 201; +15 sync, +2 integration from
  the rewritten periodic-validation tests in `engine_tests.rs` —
  the old `update_queue_rejects_periodic_sync` test was replaced,
  not duplicated).
- BUG-001–019 regression tests all pass by name in the output; the
  BUG-003 guard (`add_to_queue_rejects_completed_download`) still
  passes for one-time queues, and the new periodic exception is
  pinned by `sync_refuses_a_one_time_queue_and_start_queue_refuses_a_periodic_queue`.
- `cargo check --tests` warning-free (0 warnings).

### Not tested in this round (same limits as rounds 1–5, plus S05)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `sync_queue_now` command is uncompiled, and F04-M4 is blocked.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- Sync has never run against a real website — all checks used
  local fixture servers only.

## Round-6 summary

- Automated engine cases: 218 total, all passing (2 consecutive
  full runs, 0 failures). 15 new sync tests (+2 integration from
  the rewritten validation tests).
- UI build: `npm run build` passes.
- Manual / blocked: 1 new case (periodic dialog — Sync now,
  interval, last-sync status) — needs the Windows desktop binary.
- Bugs found: 1 × S2 (BUG-020). Fixed failing-test-first, test
  kept. See `bugs.md`.
## Round 7 — Finish browser catch (S13 / F01) — Milestone 2, 2026-11-15

Extends the round-3 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync plus the
S13 finish (drain starts downloads, Download All context entry
verified through the real host binary, popup pending count /
last hand-off). Env: same Linux VM, local mock HTTP / page
servers only — no public internet, no IDM servers, no browser
automation (no headless Chromium works on this VM; no
Playwright). Engine: `cargo test` in `app/engine` (222 total, 0
failed, 2 consecutive full runs — 21 unit + 111 integration +
25 extension + 15 grabber + 14 verifier + 15 sync + 21 video;
was 218, +4 new extension tests). UI: `npm run build` passes;
`npm run test:extension` passes; `node --check` passes on all
four extension scripts; extension JS tests 21/21 (`node --test`
popup 6 + background 4 + video-detect 11). Host smoke:
download-all 14/14 (`app/host/download-all-smoke.py`, new),
video 11/11 re-run green. No desktop binary: the Tauri shell
still does not compile here (missing `gdk-3.0`), so the
`drain_pending_links` doc change is uncompiled and every case
needing the running desktop app is marked **blocked**.

### F01 — One-click browser download, finished (S13)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F01-H5 | happy | Extension sends `download_link` → drain (default settings) | Download STARTS at once and completes byte-exact (512 KiB fixture), not just queued | cargo test `drain_picks_up_enqueued_link_via_add_download` (rewritten: was asserting `queued`) | PASS |
| F01-H6 | happy | Same, with `browser_start_drained` off | Download created and left `queued`, 0 bytes, no file; user can start it by hand and it completes | cargo test `drain_leaves_download_queued_when_start_drained_is_off` | PASS |
| F01-H7 | happy | Context-menu "Download all links on this page" → real `fetchline-host` binary, local page (2 files + image + duplicate + dead link) | 3 links enqueued as `download_all` rows; duplicate de-duplicated; dead link skipped; reply carries `pending_count` | host smoke `download-all-smoke.py` D3/D3b/D4 (14/14) | PASS |
| F01-H8 | happy | Popup after hand-offs | Pending count ("N links waiting") and last hand-off ("Last sent …" / "Last send failed …") shown; send-page success and failure both recorded | node tests `popup.test.js` (6) + `background.test.js` (4) | PASS |
| F01-E8 | edge: pre-Milestone-2 database | Open a DB whose settings table has no `browser_start_drained` | Column added by migration, default ON, existing settings intact | cargo test `pre_m2_database_migrates_settings_with_browser_start_drained` | PASS |
| F01-N8 | negative: integration off / excluded, Download All | `download_all` with no links while off, or for an excluded page, via the real host binary | Error reply, nothing enqueued, pending count unchanged | host smoke D6/D6b/D7/D7b (BUG-012 path) | PASS |
| F01-N9 | negative: pending count | `get_settings` on a fresh DB, then after 2 enqueues | `pending_count` 0, then 1 and 2 on the queued replies | cargo test `host_get_settings_reports_pending_count` | PASS |
| F01-H9 | happy | `video_found` → drain (default settings) | Drained video download starts at once and completes byte-exact (same shared drain) | cargo test `video_found_end_to_end_drains_into_a_real_download` (renamed; was `…_real_queued_download`, asserted `queued` + manual start) | PASS |
| F01-M4 | manual: drain in the desktop app | Start the desktop app, send a link, wait ≤ 30 s | Download starts (not just appears queued) in the main window; with the setting off it appears queued | blocked — no desktop binary; the Tauri `drain_pending_links` doc change is uncompiled (`gdk-3.0`) | BLOCKED |
| F01-M5 | manual: popup status | Open the popup after sending links | Pending count and last hand-off read as in `popup.js` | blocked — no browser automation on this VM (code reviewed + Node fake-`chrome` tests) | BLOCKED |
| F01-M6 | manual: Download All menu | Right-click a page background, "Download all links on this page" | Page links start downloading | blocked — no browser automation on this VM (code reviewed + real-host smoke test) | BLOCKED |
| F01-T1 | to check: Windows handshake | Registry install of `com.fetchline.host.json`, extension ID in `allowed_origins`, first drain against the real desktop app's DB on Windows | Handshake works against the real app-data folder | — | TO CHECK (Windows — checklist in `app/extension/README.md`) |

### Regression (rounds 1–6 areas, re-run in full)

- Full suite: **222 total, 0 failed, 2 consecutive full runs**
  (21 unit + 111 integration + 25 extension + 15 grabber + 14
  verifier + 15 sync + 21 video). The 15 sync tests still pass
  (Milestone 1 untouched). BUG-002/005/006/008/011/012/020
  regression tests all pass by name in the output.
- `cargo check --tests` warning-free (0 warnings).
- S13 regression: `download_link` still enqueues (extension
  tests 25/25 + host smoke V7/D1); BUG-012's off-switch paths
  re-verified through the real binary (smoke D6/D7).
- `npm run build` passes; Vite preview serves the app (index
  200, `Fetchline` title, checked this round on 127.0.0.1:4176).

### Not tested in this round (same limits as rounds 1–6, plus S13)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `drain_pending_links` doc change and the Settings dialog
  checkbox are uncompiled, and F01-M4–M6 are blocked.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- The Windows handshake (registry install, extension ID,
  app-data folder) is unproven — the checklist for proving it is
  now written in `app/extension/README.md`, but none of its
  steps has run.
- A failed drain start (`DrainSummary.start_failed`) has no
  triggering test: a fresh download's `start_download` cannot
  fail in the paths exercised here, so the recording is
  code-reviewed only.

## Round-7 summary

- Automated engine cases: 222 total, all passing (2 consecutive
  full runs, 0 failures). 4 new extension tests; 2 existing
  tests rewritten for the new start-by-default behaviour (drain
  + video end-to-end).
- Extension JS tests: 21 passed, 0 failed (11 video-detect + 6
  popup + 4 background — the last 10 are new).
- Host smoke tests: download-all 14/14 (new), video 11/11
  (re-run green).
- UI build: `npm run build` passes; `npm run test:extension`
  passes.
- Manual / blocked: 3 cases (drain in the desktop app, popup
  status, Download All menu) — all need a real browser / the
  Windows desktop binary.
- Bugs found: none. See `bugs.md` Round 8.
- `parity.md` still scores the pre-sync state (58.3/100,
  must-haves 10/17) — a fresh replica-diff pass is needed to
  re-score the browser-catch row (still `partial` in
  `features.csv`, now only for the unproven Windows handshake
  and the never-in-a-real-browser extension).

## Round 8 — Finish video grabber (S08 / F05) — Milestone 3, 2026-11-15

Extends the round-4 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync plus the
S08 finish (HLS decision + playlist understanding). Env: same
Linux VM, local fixture servers only — no public internet, no
IDM servers, no browser automation (no headless Chromium works
on this VM; no Playwright). Engine: `cargo test` in `app/engine`
(235 total, 0 failed, 2 consecutive full runs — 24 unit + 111
integration + 25 extension + 15 grabber + 14 verifier + 15 sync
+ 31 video). UI: `npm run build` passes; `npm run
test:extension` passes; `node --test` on all three extension
test files (27 passed, 0 failed); `node --check` passes on all
four extension scripts. The real `fetchline-host` binary was
driven with native-messaging frames against a local HTTP server
serving a real .mp4, a real .m3u8 and a real .mpd (13/13 smoke
checks, `app/host/video-smoke.py`). No desktop binary: the
Tauri shell still does not compile here (missing `gdk-3.0`), so
the `detect_page_videos` command (now returning the new `note`
field) is uncompiled, and every case needing the running desktop
app is marked **blocked**.

### F05 — Capture a video from a web page (S08), new cases

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F05-H5 | happy: HLS decision | Master playlist page scanned (desktop panel) | Master expands into its 3 real variants (1280 × 720 / 1920 × 1080 / Unknown quality), each probed for size, each note says "not merged"; the master itself is not offered | cargo test `detect_page_videos_expands_master_playlist_into_real_variants` + `hls_parse_matches_shared_fixtures` (Rust + JS over the same fixture) | PASS |
| F05-H6 | happy: VOD playlist | Page links a VOD .m3u8 (3 segments, 25 s) | Listed once, live=false, note reports "3 segments, 25 s" and "not merged" | cargo test `detect_page_videos_vod_playlist_reports_segments_and_not_live` | PASS |
| F05-H7 | happy: .m3u8 download | `video_found` with a .m3u8 variant → drain | Download completes; bytes on disk are exactly the playlist text — no segments fetched, nothing merged | cargo test `video_found_m3u8_drains_into_a_real_playlist_download` + host smoke V9 | PASS |
| F05-H8 | happy: DASH manifest | Page links a .mpd manifest | Listed as MPD, quality "Unknown quality", note says "DASH manifest … not merged", downloadable | cargo test `detect_page_videos_dash_manifest_is_listed_as_is` + `video_validation_accepts_mpd_with_dash_note` + host smoke V10 | PASS |
| F05-E8 | edge: live playlist | Page links a live .m3u8 (no ENDLIST) | Listed live=true, note says "Live stream … not merged" | cargo test `detect_page_videos_live_playlist_is_live` | PASS |
| F05-E9 | edge: only low quality | Page with a plain clip, no dimensions, no label | Listed once, quality "Unknown quality" — never invented | cargo test `fixture_hls_master_dash_and_low_quality` (Rust + JS, `low-quality.html`) | PASS |
| F05-E10 | edge: master with 10 variants | Page links a master with 10 variant playlists, all dead | 8 entries shown (cap), all listed as not downloadable with their probe error — none hidden | cargo test `detect_page_videos_master_variant_list_is_capped` | PASS |
| F05-E11 | edge: unparsable playlist | Playlist text that is not `#EXTM3U` / empty | Parser returns null (JS) / None (Rust); the scan keeps the single playlist item with its generic note | unit tests `parse_m3u8_encrypted_is_flagged_and_junk_is_not_a_playlist` + JS parseM3u8 test | PASS |
| F05-N10 | negative: encrypted playlist | Page links an AES-128 `#EXT-X-KEY` playlist | Listed as NOT downloadable, error names encryption (was: offered as a normal playlist — the flag was never set) | cargo test `detect_page_videos_encrypted_playlist_is_not_downloadable` + JS/Rust parse tests | FAIL→FIXED (BUG-021) |
| F05-M4 | manual: extension panel notes | Hover a video with a playlist variant, open the panel | Playlist note shown under the variant; button reads "Download playlist" / "Download manifest" | blocked — no browser automation on this VM (code reviewed: `content.js`, `video-detect.js`) | BLOCKED |
| F05-M5 | manual: desktop variant list | Toolbar "Video grabber", paste a page with a master playlist | Panel lists the expanded variants with notes; Download opens S02 prefilled, Later queues | blocked — no desktop binary; the `detect_page_videos` command is uncompiled (`gdk-3.0`) | BLOCKED |

### Regression (rounds 1–7 areas, re-run in full)

- Full suite: **24 unit + 111 integration + 25 extension + 15
  grabber + 14 verifier + 15 sync + 31 video = 235, 0 failed, 2
  consecutive full runs** (was 222 — +3 unit in `video.rs`, +10
  video tests). S13 extension tests (25) and S05 sync tests (15)
  still pass by count and by name in the output; BUG-002/005/
  006/008/011/012/020 regression tests all pass.
- `cargo check --tests` warning-free (0 warnings).
- Host smoke: video 13/13 (was 11/11 — V9 real .m3u8, V10 real
  .mpd), download-all 14/14 re-run green.

### Not tested in this round (same limits as round 4, plus S08)

- No real browser: the popup scan, floating button and panel
  have never executed in Chrome/Firefox/Edge here — verified by
  code review + `node --check` + the Node fixture tests only.
  The extension-side `parseM3u8` is tested over the shared
  playlist fixtures, but the content script does not fetch
  playlists itself (a cross-origin fetch from the page would
  usually be blocked) — variant expansion in the browser is the
  desktop panel's job.
- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `detect_page_videos` command and the S02 `initialUrl` prefill
  are uncompiled, and F05-M4/M5 are blocked.
- The Windows handshake (registry install, extension ID,
  app-data folder) is unproven — the host was tested on this
  Linux VM only, against the same SQLite file the engine uses.

## Round-8 summary

- Automated engine cases: 235 total, all passing (2 consecutive
  full runs, 0 failures). 13 new tests (3 unit in `video.rs` +
  10 video tests).
- Extension JS tests: 27 passed, 0 failed (17 video-detect, was
  11; 6 popup; 4 background).
- Host smoke tests: video 13/13 (was 11/11), download-all 14/14
  re-run green.
- UI build: `npm run build` passes; `npm run test:extension`
  passes.
- Manual / blocked: 2 new cases (extension panel notes, desktop
  variant list) — both need a real browser / the Windows desktop
  binary.
- Bugs found: 1 × S3 (BUG-021, encrypted-playlist flag missed in
  both parsers). Fixed failing-test-first, tests kept. See
  `bugs.md` Round 9.
- `parity.md` still scores the pre-sync state (58.3/100,
  must-haves 10/17) — a fresh replica-diff pass is needed to
  re-score now that the video-grabber row is `yes` in
  `features.csv` (the row was flipped per the milestone brief:
  everything except the impossible-here real-browser / Windows
  proof is proven; the remaining limits are listed in the row
  note).

## Round 9 — Finish site grabber (S06 / F06) — Milestone 4, 2026-11-15

Extends the round-5 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync plus the
Milestone 4 finish (per-project queue choice + on-completion
action reusing the existing queue completion machinery). Env:
same Linux VM, local fixture servers only — no real website was
ever crawled (forbidden as a test: do not hammer real sites).
Engine: `cargo test` in `app/engine` (248 total — 24 unit + 111
integration + 25 extension + 28 grabber + 14 verifier + 15 sync
+ 31 video, 0 failed, 2 consecutive full runs; was 235 — +13
new grabber tests). `cargo check --tests` warning-free. `npm run
build` passes (tsc + vite). No desktop binary: the Tauri shell
still does not compile here (missing `gdk-3.0`), so the
`create_grabber_project` command changes are uncompiled, and
every case needing the running desktop app is marked **blocked**.

### F06 — Site grabber, finished: queue choice (S06)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F06-Q1 | happy | Project with queue "Photos", download mode, depth 1, images+documents | All 3 grabbed files filed in "Photos", all `complete`; main queue empty | cargo test `grabber_files_land_in_the_chosen_queue_not_main` | PASS |
| F06-Q2 | happy | Project with no queue named (7-arg create) | Reads back `queue-main`, action `none`; its 1 file lands in the main queue | cargo test `grabber_create_defaults_to_main_queue_and_no_action` | PASS |
| F06-Q3 | negative: unknown queue | Create with `queue_id = "queue-nope"` | Rejected "Queue not found: queue-nope — pick an existing queue", nothing saved | cargo test `grabber_create_rejects_unknown_queue` | PASS |
| F06-Q4 | negative: queue deleted after create | Create with a valid queue, delete the queue row, run | Run fails with "no longer exists", 0 pages explored, 0 downloads | cargo test `grabber_run_fails_when_its_queue_is_gone` | PASS |
| F06-Q5 | edge: pre-Milestone-4 database | Open a DB with a `grabber_projects` table from before Milestone 4 | Columns added by migration, old project survives and reads back as main queue + no action | cargo test `pre_milestone4_database_migrates_grabber_projects` | PASS |

### F06 — Site grabber, finished: on-completion action (S06)

The action reuses the EXISTING queue completion machinery: the
project's action is registered as a `PendingAction` under its
target queue id (one shared `register_completion_action` helper
— the queue drain uses the same one), so the existing
`queue-action-pending` event, the existing 60 s countdown
dialog, `execute_queue_action` and `dismiss_queue_action` all
work on it unchanged. There is no second countdown and no
second cancel path. The action is armed ONLY when the run
finishes cleanly (`done`) AND downloaded ≥ 1 file — the BUG-003
lesson, applied to grabber projects.

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F06-A1 | happy | Project with `run_program` (marker script), download 1 image | Run `done`, 1 downloaded; pending action on the main queue is `run_program` with the file path; executing it runs the script; pending cleared | cargo test `grabber_completion_action_runs_when_files_downloaded` | PASS |
| F06-A2 | happy | Project with `open_file`, download 3 files (depth 1, images+documents) | Pending action is `open_file` with `file_path` = the last file downloaded (`notes.txt`) | cargo test `grabber_completion_action_open_file_names_the_last_file` | PASS |
| F06-A3 | negative: nothing downloaded | Project with `run_program`, start page whose only matched file is dead (404) | Run `done` (1 matched, 0 downloaded, 1 failed); NO pending action | cargo test `grabber_completion_action_not_armed_when_nothing_downloaded` (also: a run matching 0 files arms nothing) | PASS |
| F06-A4 | negative: explore-only | Project with `shutdown`, explore-only mode | Run `done`, 8 matched, 0 downloaded, 0 downloads created; NO pending action | cargo test `grabber_completion_action_not_armed_in_explore_only` | PASS |
| F06-A5 | negative: run failed | Project with `shutdown`, dead start page (404) | Run `failed`, 0 downloaded; NO pending action | cargo test `grabber_completion_action_not_armed_when_the_run_failed` | PASS |
| F06-A6 | negative: stopped after downloading | Project with `shutdown`, stop mid-download-phase after ≥ 1 file finished | Run `stopped`, ≥ 1 downloaded; NO pending action (mirrors the queue drain: stopped early → no action) | cargo test `grabber_completion_action_not_armed_when_stopped_after_downloading` | PASS |
| F06-A7 | happy: cancel path | Project with `shutdown`, download 1 file | Pending `shutdown` exists; dismiss clears it; executing afterwards errors (nothing runs) — the shutdown itself is NEVER executed in a test | cargo test `grabber_completion_action_cancel_clears_it` | PASS |
| F06-A8 | negative: bad action | Create with action `explode`; `run_program` with no / blank program | Rejected "Unknown completion action" / "Choose a program"; `shutdown` with no program accepted | cargo test `grabber_create_rejects_bad_completion_action` | PASS |
| F06-M3 | manual: wizard queue + action | Step 3: pick a queue, pick "Shut down the computer", Run | Dropdown lists the queues (main first); countdown note shown; on finish the existing 60 s countdown dialog appears for the target queue | blocked — no desktop binary; the Tauri command changes are uncompiled (`gdk-3.0`); verified by `npm run build` + source inspection only | BLOCKED |
| F06-T1 | to check (partly covered) | Stop during the download phase | Status `stopped`, files downloaded before the stop stay; no action armed — now measured by F06-A6. The "current file finishes first" wait itself is still code-review only | cargo test `grabber_completion_action_not_armed_when_stopped_after_downloading` | TO CHECK (wait detail) |

### Regression (rounds 1–8 areas, re-run in full)

- Full suite: **248 total, 0 failed, 2 consecutive full runs**
  (24 unit + 111 integration + 25 extension + 28 grabber + 14
  verifier + 15 sync + 31 video; was 235 — +13 grabber).
  Grabber verifier 14/14, sync 15/15, extension 25/25, video
  31/31 all still pass.
- Queue completion actions re-checked by the passing suite: the
  drain's arming now goes through the shared
  `register_completion_action` helper — F04-A1/A2/A3
  (`queue_action_run_program_executes_and_dismiss_clears`,
  `open_file_action_executes`,
  `queue_action_shutdown_errors_off_windows`) all pass by name,
  and the BUG-003 regression test
  (`queue_with_only_completed_files_registers_no_action_and_drops_them`)
  passes unchanged.
- `cargo check --tests` warning-free (0 warnings).

### Not tested in this round (same limits as round 5, plus S06)

- No real website was ever crawled — forbidden as a test; all
  checks used the local `GrabberSite` fixture (plus the new
  `/only-dead.html` page whose only file 404s).
- No desktop binary / Tauri shell: the `create_grabber_project`
  command signature change is uncompiled, and F06-M1–M3 are
  blocked. The wizard's queue dropdown and action picker are
  verified by `npm run build` + source inspection only.
- The 60 s countdown dialog itself was not clicked in a running
  app (no binary); the engine side — arming, the pending state,
  execute, and the dismiss/cancel path — is covered by F06-A1/A7,
  and the dialog is the same `QueueActionDialog` the queue drain
  already uses (F04-M2, still blocked).
- A grabber-armed action and a queue-drain action for the SAME
  queue share one `PendingAction` slot (keyed by queue id): if
  both fire, the second overwrites the first. Documented in the
  build log; not tested (needs two runs racing, timing-dependent).

## Round-9 summary

- Automated engine cases: 248 total, all passing (2 consecutive
  full runs, 0 failures). 13 new grabber tests (queue choice 5,
  completion action 8).
- UI build: `npm run build` passes (tsc + vite).
- Manual / blocked: 1 new case (F06-M3, wizard queue + action) —
  needs the Windows desktop binary.
- Bugs found: none in this round. No BUG-022 entry: the new
  tests were written against the new code and passed on the
  first full run; no failing-test-first fix was needed.
- `parity.md` still scores the pre-sync state (58.3/100,
  must-haves 10/17) — a fresh replica-diff pass is needed to
  re-score now that the site-grabber row is `yes` in
  `features.csv` (flipped per the milestone brief: everything
  except the impossible-here real-website crawl proof is proven;
  the remaining limits are listed in the row note).

---

## Round 11 — Finish progress dialog (S03) — Milestone 5, 2026-11-15

Extends the round-10 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync plus the S03
finish (60 s speed graph, engine-driven retry countdown with
Retry now, Open folder on completion). Env: same Linux VM, local
mock HTTP server only — no public internet, no IDM servers, no
browser automation (no headless Chromium works on this VM; no
Playwright). Engine: `cargo test` in `app/engine` (255 total, 0
failed, 2 consecutive full runs — 24 unit + 111 integration + 25
extension + 28 grabber + 14 verifier + 7 progress + 15 sync + 31
video; was 248 — +7 progress tests). UI: `npm run build` passes;
`npm run test:speed-graph` (8 passed, 0 failed); extension JS
tests 27/27 re-run green; `npm run test:extension` passes. The
real `fetchline-host` binary was not re-driven (no host change in
this milestone). No desktop binary: the Tauri shell still does
not compile here (missing `gdk-3.0`), so the `retry_now` and
`open_folder` commands are uncompiled, and every case needing
the running desktop app is marked **blocked**.

### S03 — Download progress dialog: speed graph, retry countdown, Open folder (F02/F03)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S03-G1 | happy: sampler cadence | Record samples at 100 ms intervals, then at exactly 500 ms | Sub-500 ms samples dropped; the 500 ms sample kept; input never mutated | node test `samples are appended at the right cadence, not per event` | PASS |
| S03-G2 | happy: 60 s prune | Samples at t=0/500/1000, then one at t=61 000 | t=0 and t=500 pruned; t=1000 (exactly at the edge) kept | node test `old samples are pruned past the 60 s window` | PASS |
| S03-G3 | happy: list bounded | 500 samples over 250 s | List holds 121 (the last 60 s at 500 ms), never grows past it | node test `the window bounds the list: long runs never grow it past 121` | PASS |
| S03-G4 | happy: points scale | Samples 0 / 50 / 100 B/s over 60 s, box 120×40 | Newest at x=120, oldest at x=0; fastest at y=0, stalled at y=40, half at y=20; all points inside the box | node tests `graph points scale to the fastest sample…` + `graph points stay inside the box…` | PASS |
| S03-G5 | edge: empty / one sample | No samples; one sample | No points; one point at the right edge rendered as a dot, not a line | node test `empty samples give no points; one sample gives a dot, not a line` | PASS |
| S03-G6 | edge: negative speed | Record −5 B/s | Stored as 0, graphed at the bottom | node test `a negative speed is stored as zero, never graphed below the axis` | PASS |
| S03-G7 | happy: polyline | Three points | `"0,40 60,20 120,0"` in order | node test `polyline string joins points in order` | PASS |
| S03-R1 | happy: countdown on 500 + recovery | GETs fail with 500, max_retries=3, start | Progress event carries retry attempt 1 of 3, 2 s left, error names 500; server recovers → download completes byte-exact, retry state cleared | cargo test `retry_countdown_appears_on_500_and_recovers` | PASS |
| S03-R2 | happy: countdown on dropped connection | Throttled download, drop the connection mid-body (new `set_drop_get` fixture) | Retry wait appears after bytes were already downloaded, with a non-empty error; connection restored → completes byte-exact | cargo test `retry_countdown_appears_when_connection_drops_mid_download` | PASS |
| S03-R3 | happy: Retry now | 500s, max_retries=3, Retry now on the first wait | Second wait arrives < 1.5 s later (without it: 2 s); download still ends in `error` once attempts are exhausted — Retry now hurries, it does not add attempts | cargo test `retry_now_skips_the_backoff` | PASS |
| S03-R4 | negative: Retry now with nothing waiting | Healthy running download, queued download, unknown id | All refuse with a clear error ("not running" / "not waiting for a retry") | cargo test `retry_now_errors_when_nothing_is_waiting` | PASS |
| S03-R5 | happy: cancel during retry | 500s, wait for the retry wait, Cancel | Status `deleted` at once; no retry wait survives | cargo test `cancel_during_retry_deletes_the_download` | PASS |
| S03-R6 | happy: no countdown when healthy | Throttled healthy download, watch events for 2 s | Every event carries `retry: null`; getter returns None | cargo test `no_retry_state_while_downloading_normally` | PASS |
| S03-O1 | happy/negative: Open folder validation | Complete a download, resolve its folder, then delete the folder | Folder resolves to the download folder; after deletion both `folder_for_download` and `open_folder` fail with "no longer exists" before spawning anything; unknown id is not found | cargo test `open_folder_validation` | PASS |
| S03-M1 | manual: graph in the dialog | Download a large file, watch the dialog | Graph builds as the download runs, newest at the right edge; empty state before the first samples | blocked — no desktop binary; verified by `npm run build` + built-bundle check (strings + `warning-soft` rule present) + the 8 sampler tests | BLOCKED |
| S03-M2 | manual: countdown in the dialog | Drop the connection mid-download, watch the dialog | "Connection lost — retrying in Ns (attempt a+1 of m)" ticks down from the engine's deadline; Retry now retries at once; Cancel deletes | blocked — no desktop binary; the engine state behind it is covered by S03-R1–R5 | BLOCKED |
| S03-M3 | manual: Open folder | Finish a download, click "Open folder"; delete the folder, click again | File manager opens the folder; with the folder gone the dialog shows "The folder no longer exists: …" and opens nothing | blocked — no desktop binary; the Tauri command is uncompiled (`gdk-3.0`); the validation is covered by S03-O1, the spawn itself has never executed | BLOCKED |
| S03-M4 | manual: states + keyboard | Tab through the dialog in each state (downloading, waiting for retry, paused, complete, error) | Every button reachable and labelled; Escape / backdrop closes | blocked — no desktop binary (code reviewed: native buttons, `role="dialog"`, `aria-modal`, labelled Close; same patterns as the other dialogs) | BLOCKED |
| S03-T1 | to check: pause during a retry wait | Pause while a segment waits out its backoff | Status `paused`, no stale countdown in the snapshot (the pause paths clear the slot first; `run()` clears it at the end) — code review only, no dedicated test | — | TO CHECK (engine) |

### Regression (rounds 1–10 areas, re-run in full)

- Full suite: **255 total, 0 failed, 2 consecutive full runs**
  (24 unit + 111 integration + 25 extension + 28 grabber + 14
  verifier + 7 progress + 15 sync + 31 video; was 248 — +7
  progress). The progress suite alone was run 3 times during
  development (7/7 every time).
- S01–S03 regression: add/pause/resume (F02/F03) still pass by
  name in the output (`full_download_multi_segment_matches_bytes`,
  `pause_then_resume_completes_with_correct_bytes`,
  `resume_restarts_when_remote_file_changes`); the segment retry
  itself is unchanged except for the sleep shape — the
  `max_retries_zero_means_single_attempt_and_fast_error` test
  (single attempt, fast error) still passes, which pins that the
  backoff values were not altered.
- Sync + extension + video + grabber suites still pass in full
  (15 + 25 + 31 + 28 + 14), as do the extension JS tests (27/27).
- `cargo check --tests` warning-free (0 warnings).
- `npm run build` passes; the built bundle contains the new
  dialog strings and the emitted `warning-soft` rule; no raw hex
  in the new/changed UI code (tokens only).

### Not tested in this round (same limits as round 10, plus S03)

- No desktop binary / Tauri shell: the `retry_now` and
  `open_folder` commands are uncompiled, and S03-M1–M4 are
  blocked. The dialog is verified via `npm run build` + the
  built-bundle check + source inspection only.
- The shell-open spawn (`open_folder` success path) has never
  executed anywhere: it needs a desktop app and a file manager,
  and there is neither on this Linux VM. Only the validation
  (folder exists / not found / no longer exists) is tested.
- The retry countdown's ticking is driven by the engine's
  deadline and re-emitted every 250 ms; the dialog renders
  whatever the latest event says. A wall-clock check that the
  number on screen falls once per second was not run (no binary).
- The 60 s graph was never rendered against a real download in
  the app — the sampler is unit-tested and the SVG is in the
  built bundle, but no screenshot exists (no headless Chromium
  works on this VM; same limit as every replica-diff pass).

## Round-11 summary

- Automated engine cases: 255 total, all passing (2 consecutive
  full runs, 0 failures). 7 new progress tests + 8 speed-graph
  unit tests (`node --test`).
- UI build: `npm run build` passes (tsc + vite); Vite preview
  serves the app (index 200, `Fetchline` title).
- Manual / blocked: 4 new cases (S03-M1–M4, graph, countdown,
  Open folder, states + keyboard) — all need the Windows desktop
  binary.
- Bugs found: none in this round. No BUG-022 entry: the new
  tests passed on their first full runs against the new code; no
  failing-test-first fix was needed. One test caught a design
  wart before it shipped (a 240-sample cap the 60 s window makes
  unreachable — the cap was removed, not tested into place; see
  build-log.md).
- `parity.md` still scores the pre-sync state (58.3/100,
  must-haves 10/17) — a fresh replica-diff pass is needed to
  re-score now that the progress-dialog row is `yes` in
  `features.csv` (flipped per the milestone brief: everything
  except the uncompiled-Tauri part is proven; the remaining
  limits are listed in the row note).

## Round 12 — Final comprehensive verification (all five milestones) — 2026-11-15

Independent verifier pass after S05 periodic sync, S13 finish,
S08 finish, S06 finish and S03 finish. Full engine suite re-run
twice: **271 total, 0 failed** (24 unit + 111 integration + 25
extension + 28 grabber + 14 verifier + 7 progress + 15 sync + 31
video + 16 new final-verify tests). `npm run build` passes;
extension JS 27/27 (`node --test` over the three test files);
speed-graph sampler 8/8; `node --check` passes on all four
extension scripts; `cargo check --tests` warning-free (0
warnings); host smoke against a freshly built `fetchline-host`
binary: video 13/13, download-all 14/14. New verifier suite:
`app/engine/tests/final_verify_tests.rs` (16 tests, local
fixture servers only). No new bug was found: no BUG-022.

One test failed on its first run and was a test bug, not a
product bug: `final_retry_countdown_on_dropped_connection_then_retry_now_recovers`
expected `max_attempts` 3 after setting `max_retries = 3` by
writing the store directly, but the manager caches the retry
count and only `update_settings` / `apply_settings` refreshes
it (the Tauri shell calls `apply_settings` once at launch, and
`update_settings` applies too). The test now sets the value
through `mgr.update_settings`, matching the existing
progress-tests helper; it passes.

One process finding, not a product bug: the `fetchline-host`
binary in `app/host/target/debug/` was stale (built 21:32,
before `native_host.rs` changed at 22:49), so the first smoke
run failed V10 (.mpd refused) and D2/D3b/D5b (no
`pending_count` in replies). Rebuilding the binary
(`cargo build` in `app/host`) made both smoke scripts pass
13/13 and 14/14. The checked-in binary is a build artefact,
not a shipped file — but any future smoke run must rebuild it
first.

| Case | What it checks | Expected | Test | Result |
| --- | --- | --- | --- | --- |
| F04-V1 | Sync: unchanged skipped, changed re-downloaded | Unchanged sync: `unchanged: 1`, 0 GETs; after a version bump: `changed: 1`, bytes byte-identical to the new version | `final_sync_changed_redownloaded_unchanged_skipped` | PASS |
| F04-V2 | Sync: remote-deleted | `removed: 1`, local copy kept on disk | `final_sync_remote_deleted_reported_and_local_kept` | PASS |
| F04-V3 | Sync: interval floor | Intervals 0/1/4 min and no interval rejected ("at least every 5 minutes"); 5 min accepted | `final_sync_interval_below_5_minutes_rejected` | PASS |
| F04-V4 | Sync: one_time/periodic exclusivity | Interval on one-time rejected; `start_queue` refuses periodic; `run_sync_queue` refuses one-time | `final_sync_one_time_and_periodic_are_exclusive` | PASS |
| F01-V1 | Drain: drained link actually downloads | Accepted, started (no `start_failed`), completes byte-exact | `final_drain_starts_download_not_just_queued` | PASS |
| F01-V2 | Drain: excluded after queueing | Excluded after enqueue: drain rejects, no download created | `final_drain_refuses_excluded_site` | PASS |
| F01-V3 | Drain: duplicate guard | Same URL twice: both accepted, different save paths, both complete byte-exact | `final_drain_duplicate_links_get_different_paths` | PASS |
| F05-V1 | Video: encrypted playlist not offered | Shared fixture parses `encrypted: true`; page scan lists it not downloadable, error names encryption | `final_video_encrypted_playlist_not_offered_as_downloadable` | PASS |
| F05-V2 | Video: .m3u8 offered as-is | Scan note says "not merged"; download saves the playlist bytes exactly | `final_video_m3u8_downloaded_as_is_bytes_exact` | PASS |
| F05-V3 | Video: quality never invented | "Ultra HD 8K Pro" → "Unknown quality"; "720p" kept; plain video with no evidence → "Unknown quality" | `final_video_quality_never_invented` | PASS |
| F06-V1 | Grabber: BUG-019 regression | Status reads `exploring`/`downloading` immediately after `run_grabber`, never a stale terminal state | `final_grabber_status_running_immediately_after_run` | PASS |
| F06-V2 | Grabber: queue choice respected | Project stored with the chosen queue; grabbed files land in it, all complete | `final_grabber_queue_choice_respected` | PASS |
| F06-V3 | Grabber: action not armed without downloads | Explore-only + shutdown: no action; all-dead download run + shutdown: no action; successful run + shutdown: action armed, then dismissed | `final_grabber_completion_action_not_armed_without_downloads` | PASS |
| S03-V1 | Progress: retry countdown on dropped connection, Retry now | Retry event (attempt ≥ 1, countdown 1–16 s, non-empty error); server recovers + Retry now → completes | `final_retry_countdown_on_dropped_connection_then_retry_now_recovers` | PASS |
| S03-V2 | Progress: cancel during retry wait | Cancel during the wait deletes at once; no retry state survives | `final_cancel_during_retry_wait_works` | PASS |
| S03-V3 | Progress: open-folder validation | Folder resolves while present; deleted folder → clear "no longer exists" error; unknown download → error | `final_open_folder_rejects_deleted_folder` | PASS |

BUG-017/018 regressions are covered by the kept verifier suite
(`grabber_verify_tests.rs`, 14/14 in both full runs): robots
cross-group Allow, excluded-after-create stops the run before
any fetch, and the BUG-019 status race.

## Round-12 summary

- Automated engine cases: 271 total, all passing (2 consecutive
  full runs, 0 failures, no flakes). 16 new final-verify tests.
- UI build: `npm run build` passes; extension JS 27/27;
  speed-graph 8/8; `cargo check --tests` 0 warnings.
- Host smoke (fresh binary): video 13/13, download-all 14/14.
- Regression sweep: BUG-001–021 regression tests all present
  and passing (32 named tests + BUG-007 source check +
  BUG-013/016 covered by the host smoke / JS tests).
- Bugs found: none. No BUG-022 entry.
- Licence (S11) untouched: `features.csv` row still `no`, no
  licence/registration code in the app or Tauri shell.
- Parity re-run (this pass, `parity.py` on the current
  `features.csv`): **66.7 / 100, must-haves 13 of 17 done**
  (parity.md itself was NOT edited — a replica-diff pass owns it).
- Still unproven (unchanged): no Windows .exe; Tauri shell
  uncompiled on Linux (gdk-3.0); extension never run in a real
  browser; no real website crawled; Windows handshake unproven.

---

## Finish periodic synchronisation (S05 / F04) — PARTIAL 2 of 4 — 2026-11-15

Extends the Round-6 plan; it does not replace it. Build: slices
1–5 plus S13 plus S08 plus S06 plus S05 periodic sync plus the S05
finish (Last-Modified fixture, ETag-only / size-only variants,
interval boundaries, empty queue, mid-download skip, shrunk file).
Env: same Linux VM, local mock HTTP servers only
(`MockServer` in `engine/tests/common/mock.rs`, extended with
`no_etag` / `send_last_modified` flags, `bump_last_modified`,
`bump_size` and `shrink_to`) — no public internet, no IDM servers,
no browser automation. Engine: `cargo test` in `app/engine`
(28 unit + 111 integration + 25 extension + 16 final-verify + 28
grabber + 14 verifier + 14 options + 7 progress + 23 sync + 31
video = 297, 0 failed on the first full run; see the note on the
second run below). UI: `npm run build` passes. No desktop binary:
the Tauri shell still does not compile here (missing `gdk-3.0`),
so the `sync_queue_now` command and the `update_queue` interval
parameter are uncompiled, and F04-M4 stays blocked.

### F04 — Periodic synchronisation, finish cases (S05)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F04-Y17 | happy | Server sends Last-Modified, no ETag; timestamp changes, same size/bytes | `changed: 1`, file re-downloaded byte-identical, new Last-Modified stored | cargo test `sync_redownloads_on_last_modified_only_change` | PASS |
| F04-Y18 | happy | Same fixture, timestamp unchanged | `unchanged: 1`, 0 GETs, 1 HEAD, bytes + mtime untouched | cargo test `sync_skips_when_last_modified_unchanged` | PASS |
| F04-Y19 | happy | Server sends no ETag, no Last-Modified; file grows 1 KiB | `changed: 1` — size alone triggers the re-download, byte-identical (new size stored) | cargo test `sync_redownloads_on_size_only_change` | PASS |
| F04-Y20 | happy | Same fixture, size unchanged | `unchanged: 1`, 0 GETs, 1 HEAD — the size comparison alone is enough to skip | cargo test `sync_skips_when_size_only_unchanged` | PASS |
| F04-Y21 | negative: interval boundaries | Save periodic with 4 / 5 / 10,080 / 10,081 min | 4 rejected ("at least every 5 minutes"), 5 accepted, 10,080 accepted, 10,081 rejected ("maximum is 10080 minutes"); a rejected save stores nothing (interval stays 5) | cargo test `sync_interval_boundaries_accept_5_min_and_1_week_reject_below_and_above` | PASS |
| F04-Y22 | edge: empty queue | Sync a periodic queue with no files | All counts 0, no files, no crash; last-sync time still recorded | cargo test `sync_of_an_empty_queue_finishes_cleanly` | PASS |
| F04-Y23 | edge: mid-download | Start a slow 2 MiB download, sync while it is `downloading` | `skipped: 1` ("not finished"), 0 HEAD probes against that file, status stays `downloading`; the download then completes byte-identical, no `.part` left, still in its queue — the sync never touches a running download (SKIP is the documented choice; the second guard in `redownload_changed_file` refuses a file that started mid-sync) | cargo test `sync_skips_a_file_that_is_mid_download_and_leaves_it_byte_identical` | PASS |
| F04-Y24 | edge: remote file shrank | Server file 512 KiB -> 256 KiB, sync | `changed: 1`, local file exactly 256 KiB, byte-identical to the new remote file, stays in its queue | cargo test `sync_redownloads_a_file_that_shrank_byte_identical` | PASS |
| F04-Y3 | re-check | ETag-only change (same size/bytes) | `changed: 1` — still passing, no change needed for the ETag-only variant | cargo test `sync_redownloads_on_etag_only_change` | PASS |
| F04-T1 | to check → covered | Real server sending Last-Modified but no ETag | Covered by F04-Y17/Y18 against a LOCAL fixture server instead — the Last-Modified path is no longer code-only. The real-website half stays forbidden: syncing against a real site as a test would hammer it, so it is never attempted (see the note below) | F04-Y17/Y18 | PASS (local fixture; real-website run forbidden) |
| F04-M4 | manual: dialog | Periodic radio, interval field, last-sync status, "Sync now" | Interval saves (≥ 5 enforced inline), last sync + counts shown, Sync now reports changed/unchanged/removed/failed | blocked — no desktop binary; the `sync_queue_now` command is uncompiled (`gdk-3.0`) | BLOCKED |

### Real-website sync is forbidden as a test

A sync test against a real website would re-probe that site on
every run of the suite — that is hammering a site Fetchline does
not own, and the replica-test rule already forbids it. All sync
checks use local fixture servers only (`MockServer` on 127.0.0.1).
This is recorded in `features.csv` row 11, `build-log.md`, and
here, so a later worker does not "close the gap" by pointing a
sync at a real URL.

### Regression (all other areas, re-run in full)

- Full suite, first run: **28 unit + 111 integration + 25
  extension + 16 final-verify + 28 grabber + 14 verifier + 14
  options + 7 progress + 23 sync + 31 video = 297, 0 failed**
  (was 271; +8 sync, +4 unit and +14 options from the parallel
  Options-dialog worker's suite, which landed during this run).
  Sync suite alone: 23/23 (15 old + 8 new).
- Second full run: 296/297 — one failure in
  `extension_tests::extension_manifest_is_mv3_and_minimal`
  ("must not request cookies"): the parallel browser-catch worker
  had added a `"cookies"` permission to `app/extension/
  manifest.json` while this run was in flight. Not a sync failure
  and not this milestone's code — see `bugs.md` Round 12. The
  sync suite itself passed 23/23 in both full runs.
- One transient failure on the first sync-suite run:
  `sync_reports_failed_probe_and_retries_at_the_next_sync`
  (BUG-020 regression test) failed with its finished file suddenly
  missing from disk, then passed 3/3 in isolation and on the full
  re-run. Consistent with a concurrent run's
  `/tmp/fetchline-test-*` cleanup deleting a live test directory
  while three other workers shared the VM — not a product fault;
  no code was changed for it, no BUG entry opened.

### Not tested in this round (same limits as Round 6, plus S05)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `sync_queue_now` command is uncompiled, and F04-M4 is blocked.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- Sync has never run against a real website — by rule (see above),
  not by omission.

## Finish Options dialog (S09) — PARTIAL 3 of 4 — 2026-11-15

Extends the Round-2 / S09 plan; it does not replace it. Build: the
three missing Options tabs (Site logins, Dial-Up, Sounds) on top of
the 6 existing tabs. Env: same Linux VM, local fixture servers only
(a login-gated server in `engine/tests/options_tests.rs`, the
shared `MockServer`) — no public internet, no IDM servers, no
browser automation, no audio device. Engine: new
`options_tests.rs` (14 tests) + 4 unit tests in `options.rs`.
Credentials in the tests are test-only dummies ("test-user" /
"test-pass-123") and are never a real account; passwords are never
logged. UI: `npm run build` passes. No desktop binary: the Tauri
shell still does not compile here (missing `gdk-3.0`), so the 6 new
Tauri commands (`list/save/delete_site_login`,
`get/save_sound_settings`, `test_sound_file`) are uncompiled, and
every case needing the running desktop app is marked **blocked**.

### S09-L — Site logins (F09 / F02 edge: URL needs login)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-L1 | happy | Save a login for a host, then add + start a download from the login-gated fixture | Probe + segments send Basic auth; download completes byte-exact | cargo test `site_login_gated_download_succeeds_with_login_and_fails_without` | PASS |
| S09-L2 | negative: no login | Same gated URL, no login saved | Add fails with 401, no download created | same test (first half) | PASS |
| S09-L3 | negative: wrong password | Saved login with the wrong password | Add fails with 401 | cargo test `site_login_wrong_password_fails_and_is_not_sent_to_another_host` | PASS |
| S09-L4 | negative: other host | Login saved for `localhost`, download from `127.0.0.1` | Add fails with 401 — the credential is not sent to another host | same test (second half) | PASS |
| S09-L5 | happy | Save / list / edit / delete a login | Host normalised (`https://Files.Example.org:8080/some/path` → `files.example.org`); edit keeps the password when none is typed; delete removes it; unknown id errors | cargo test `site_login_round_trip_and_row_hides_password`, `site_login_host_is_cleaned_like_excluded_sites`, `site_login_edit_keeps_password_and_delete_works` | PASS |
| S09-L6 | negative: secret leak | Save a login with a password, serialise the UI row | JSON holds only `has_password`, never the password | cargo test `site_login_round_trip_and_row_hides_password` | PASS |
| S09-L7 | negative: validation | Empty host / host with a space or userinfo / empty username / no password on create / empty password / `ntlm` auth type | Each rejected, nothing stored | cargo test `site_login_validation_rejects_bad_input` | PASS |
| S09-M6 | manual: dialog | Site logins tab: add / edit / delete, masked password, Show toggle, Save/Cancel | Values persist, bad values rejected inline, Cancel saves nothing | blocked — no desktop binary (code reviewed: `SettingsDialog.tsx` logins tab) | BLOCKED |

### S09-D — Dial-Up

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-D1 | happy | Save connection name, auto-dial, hang-up on, redial 3 | Round-trips; name trimmed, empty name clears it | cargo test `dialup_settings_persist_and_validate` | PASS |
| S09-D2 | negative: redial range | Redial -1 / 11 | Rejected, nothing stored (redial stays 3) | same test | PASS |
| S09-D3 | happy | Queue drains cleanly with hang-up on and no other action | A `hangup` pending action is armed, carrying the connection name; executing it off Windows errors "only available on Windows" and consumes the action | cargo test `dialup_hangup_arms_action_only_when_queue_has_no_action` | PASS |
| S09-D4 | edge: queue action wins | Queue with `shutdown` + hang-up on, drains cleanly | The armed action is `shutdown`, not `hangup` — one action per queue | cargo test `dialup_hangup_loses_to_a_queue_action_and_needs_a_download` | PASS |
| S09-D5 | edge: nothing downloaded | Queue whose file fails, hang-up on | No action armed (BUG-003 rule) | same test (second half) | PASS |
| S09-D6 | note | Auto-dial / redial | Stored and validated only; no dial is ever executed on this VM (no RAS) and none is faked — see build-log.md | code review of `options.rs` / `store.rs` | DOC |
| S09-M7 | manual: dialog | Dial-Up tab: name, auto-dial, hang-up, redial 0–10, Save/Cancel | Values persist, redial out of range rejected inline | blocked — no desktop binary | BLOCKED |

### S09-S — Sounds

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S09-S1 | happy | Enable download_complete + download_failed with WAV files | Completed download emits `download_complete` with its file; failed download emits `download_failed` with its file | cargo test `sound_events_fire_on_complete_and_failure_with_resolved_files` | PASS |
| S09-S2 | happy | Enable queue_finished, drain a queue | `queue_finished` emitted with its file once the drain finishes | cargo test `sound_queue_finished_fires_only_when_something_downloaded` | PASS |
| S09-S3 | edge: pause plays nothing | Pause a download (and complete one with nothing configured) | No sound event for a pause; no event at all when nothing is configured | cargo test `sound_disabled_event_records_nothing_and_playback_is_windows_only` (first half) + code review of `run()` status check | PASS |
| S09-S4 | happy | Save the three events, disable one, clear a file | Defaults are 3 events, all disabled; MP3 rejected; unknown event rejected; empty file clears | cargo test `sound_settings_round_trip_and_validation` | PASS |
| S09-S5 | negative: Test button | No file / MP3 / missing file / a real WAV file | Clear validation errors for the first three; the real WAV file passes validation and then errors "only available on Windows" (playback is Windows-only, never heard here) | cargo test `sound_disabled_event_records_nothing_and_playback_is_windows_only` (second half) | PASS |
| S09-S6 | happy: migration | Open a pre-S09 database (no dial-up columns, no sound table) | Dial-up defaults + all three sound events (disabled) appear; the new fields save into the migrated database | cargo test `dialup_and_sound_migration_adds_fields_to_old_db` | PASS |
| S09-M8 | manual: dialog | Sounds tab: enable per event, WAV path, Test button, Save/Cancel | Values persist, non-WAV rejected inline, Test reports the validation / Windows-only result | blocked — no desktop binary; the `test_sound_file` command is uncompiled (`gdk-3.0`) | BLOCKED |

### Regression (all other areas, re-run in full)

- Full suite: **297 tests** — 28 unit + 111 integration + 25
  extension + 16 final-verify + 28 grabber + 14 verifier + 14
  options + 7 progress + 23 sync + 31 video. The parallel sync
  worker's full run passed 297/297, 0 failed (including these 14
  options tests); my per-binary re-runs totalled 296/297 — the
  one failure is `extension_manifest_is_mv3_and_minimal`
  ("must not request cookies"), the parallel browser-catch
  worker's in-flight manifest edit, not this milestone's code
  (see bugs.md Round 12, recorded by the sync worker, and
  Round 13 below). The sync suite passed 23/23 in isolation
  after a transient failure under parallel load (see below).
- BUG-001–021 regression tests all pass by name in the output
  (spot-checked: BUG-002, BUG-003, BUG-005, BUG-006, BUG-008,
  BUG-011, BUG-012, BUG-020).
- `cargo check --tests` warning-free (0 warnings). `npm run
  build` passes (tsc + vite).
- One transient failure on a sync-suite run under parallel load:
  `sync_reports_failed_probe_and_retries_at_the_next_sync`
  (BUG-020 regression test) failed with its finished file
  suddenly missing from disk, then passed in isolation and on
  the full re-run (23/23). Same interference the sync worker's
  build-log entry records (a concurrent run's
  `/tmp/fetchline-test-*` cleanup deleting a live test
  directory while four workers shared the VM) — not a product
  fault, and not this milestone's code (this milestone does not
  touch the sync re-download path except through
  `resume_download`'s probe, which sends no credential when
  none is saved); no code was changed for it, no BUG entry
  opened.

### Not tested in this round (same limits as Round 2, plus S09)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  6 new Tauri commands are uncompiled, and S09-M6/M7/M8 are
  blocked. The Settings dialog has never been clicked in the
  running desktop app.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- DPAPI password storage on Windows, Windows RAS execution
  (auto-dial / redial / hang-up), and sound playback on Windows
  have never run anywhere here — all three are Windows-only
  branches, like shutdown/hibernate.
- At verification time the periodic-sync re-probe and the
  page-scan probes did not yet send site credentials (the
  add/download paths did) — recorded in build-log.md. Parallel
  workers have since reworked those probe paths in the same
  window (the sync re-probe now calls the credential-aware
  `probe_url`); that rework was not re-verified here.

## Round 13 — Finish browser catch (S13 / F01) — S13 finish, 2026-11-15

Extends the round-7 plan; it does not replace it. Build: S13
finish (cookie forwarding end to end, download-watcher fix,
Firefox host manifest, Windows-handshake checklist verified by
inspection). Env: same Linux VM, local mock HTTP servers only
— no public internet, no IDM servers, no browser automation
(no headless Chromium works on this VM; no Playwright).
Engine: `cargo test` in `app/engine` (326 total, 0 failed, 2
consecutive full runs — 35 unit + 111 integration + 32
extension + 16 final-verify + 28 grabber + 14 verifier + 14
progress + 7 + 15 sync + 23 video + 31 proxy; was 271 at the
start of the round, +55 — the 31 proxy tests and other new
tests are the parallel workers' additions in the same window,
not this round's; this round added 7 extension tests). UI:
`npm run build` passes; `npm run test:extension` passes
(both validators); `node --check` passes on all four
extension scripts; extension JS tests 44/44 (video-detect 17
+ popup 6 + background 4 + watcher 9 new + speed-graph 8).
Host smoke, rebuilt fresh: download-all 14/14, video 13/13,
cookie 14/14 (new `cookie-smoke.py` C1–C7). No desktop
binary: the Tauri shell still does not compile here (missing
`gdk-3.0`), so the `drain_pending_links` command is
uncompiled and every case needing the running desktop app is
marked **blocked**.

### F01 — One-click browser download, finished (S13)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F01-H10 | happy: cookie-gated drain | `download_link` with a session cookie → drain (default settings) against a cookie-gated fixture server (401 without the cookie) | Download completes byte-exact (64 KiB); the pending row's cookies are wiped once the drain decides it | cargo test `drain_cookie_gated_link_downloads_when_cookie_present` | PASS |
| F01-H11 | happy: cookies stay per-download | Gated + plain links drained together in one manager | Both complete byte-exact; the plain server saw 0 Cookie headers, the gated server ≥ 1 | cargo test `drain_sends_cookies_only_to_the_download_that_carries_them` | PASS |
| F01-N10 | negative: cookies never stored/logged | `download_link` with cookies → check the queued reply and the DB row; drain with no cookies | Reply never contains the cookie value; row stores the exact pairs; without cookies the drain rejects with a clear 401 and the row is rejected with cookies wiped | cargo test `host_download_link_with_cookies_stores_them_on_the_row` + `drain_cookie_gated_link_rejected_without_cookie` + host smoke C2/C2b/C2c | PASS |
| F01-N11 | negative: invalid cookies refused | `download_link` with `;` in a cookie value, or a space in a cookie name | Error reply, nothing enqueued | cargo test `host_download_link_rejects_an_invalid_cookie` + host smoke C3/C3b/C4/C4b | PASS |
| F01-N12 | negative: cookies dropped on restart | Drain a gated link with `browser_start_drained` off, drop the manager, new manager on the same DB, start the download | Download fails with a clear 401 (no stale session reused) | cargo test `cookies_are_dropped_when_the_app_restarts` | PASS |
| F01-E9 | edge: download watcher stream | Simulated `chrome.downloads.onCreated` stream (fake `chrome` in Node): downloadable link, excluded site, integration off, file type not in the catch list, duplicate URL, full local-path filename, non-http(s)/finished downloads | Cancel + hand-off of `.zip` with cookies attached; excluded/off/out-of-list untouched; duplicate handed over twice; local-path filename still caught (BUG-024 regression); non-http(s)/finished ignored | node test `watcher.test.js` (9) | PASS |
| F01-T1 | to check: Windows handshake | Registry install of `com.fetchline.host.json`, extension ID in `allowed_origins`, first drain against the real desktop app's DB on Windows | Handshake works against the real app-data folder | — | TO CHECK (Windows — checklist in `app/extension/README.md`, now annotated per item as verified-by-inspection vs needs-Windows) |
| F01-N7 | negative: cookies field | `download_link` / `video_found` with and without a `cookies` field; `download_all` never carries cookies | With: stored + forwarded per F01-H10/N10; without: row stores NULL; `download_all` carries no cookies field | host smoke C2/C5/C6 + cargo test `cookies_to_header_validates_pairs` | PASS |

### Regression (rounds 1–12 areas, re-run in full)

- Full suite: **326 total, 0 failed, 2 consecutive full runs**
  (35 unit + 111 integration + 32 extension + 16 final-verify
  + 28 grabber + 14 verifier + 14 progress + 7 + 15 sync +
  23 video + 31 proxy). The 15 sync tests, 28 grabber tests
  and 23 video tests still pass (S05/S06/S08 untouched by
  this round). BUG-001–022 regression tests all pass by name
  in the output; BUG-023/024 (this round) pass by name too.
- `cargo check --tests` passes.
- S13 regression: `download_link` still enqueues (extension
  tests 32/32 + host smoke D1/V7); BUG-012's off-switch paths
  re-verified through the real binary (smoke D6/D7, 14/14).
- `npm run build` passes.

### Not tested in this round (same limits as rounds 1–12, plus S13)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`); the
  `drain_pending_links` command is uncompiled, and F01-M4–M6
  stay blocked.
- No Playwright e2e specs: Playwright/Chromium are not installed
  on this VM.
- The Windows handshake (registry install, extension ID,
  app-data folder) is unproven — the checklist for proving it is
  annotated per item in `app/extension/README.md`, but none of
  its steps has run on Windows.
- Pause/resume WITH cookies: no automated test
  (timing-sensitive) — code-reviewed only. The header stays in
  the manager's in-memory map across a pause and the segment
  GETs read it on every attempt; a restart drops it.
- `probe_with_cookies` does not go through proxy/NTLM
  machinery: a cookie-gated link behind an NTLM proxy reports
  the proxy's 407 as a clear error, and the raw NTLM segment
  retry does not carry cookies.
- No real website was ever used: the gated server is a local
  fixture and the cookie is a test-only dummy value
  (`session=test-dummy-value`), never a real session.

## Round-13 summary

- Round 13 (S13 finish): F01-H10/H11, F01-N10/N11/N12,
  F01-E9, F01-N7 all PASS; F01-T1 stays TO CHECK (Windows).
  BUG-023 (Firefox manifest) and BUG-024 (watcher never
  fired) found and fixed, both with kept tests. Row 2
  "Catch a browser link with one click" flipped to yes — see
  features.csv for the justification (every gap closable on
  this VM closed; remaining items environment-only).
- Full suite 326/326, 0 failed, 2 consecutive runs; host
  smoke 14/14 + 13/13 + 14/14 on a freshly rebuilt binary;
  extension JS 44/44; `npm run build` passes.

## Round 14 — Final verification (all four partials) — 2026-11-15

Independent verifier pass (did not build any of the four
partials). Env: same Linux VM, local fixture servers only —
no public internet, no IDM servers, no browser automation.
Engine: `cargo test` in `app/engine` (330 total, 0 failed, 2
consecutive full runs after the BUG-025 fix — 35 unit + 111
integration + 32 extension + 4 final-verify2 + 16
final-verify + 28 grabber + 14 verifier + 14 options + 7
progress + 15 proxy + 23 sync + 31 video; was 326 — +4 new
final-verify2 tests). UI: `npm run build` passes; extension
JS 44/44 (36 extension tests + 8 speed-graph); `node --check`
passes on all four extension scripts; `cargo check --tests`
0 warnings (3 test-only warnings in `proxy_tests.rs` found
and fixed in this pass). Host smoke against a freshly rebuilt
`fetchline-host` binary: video 13/13, download-all 14/14,
cookie 14/14. No desktop binary: the Tauri shell still does
not compile here (missing `gdk-3.0`), so every case needing
the running desktop app stays **blocked**.

### Adversarial spot-checks (a–h)

| check | what was checked | expected | result |
| --- | --- | --- | --- |
| (a) cookie forwarding | Gated fixture: with cookies downloads, without cookies 401, cookies never leak to another host's download | Byte-exact gated drain; plain server sees 0 Cookie headers; restart drops cookies (clear 401) | PASS — existing extension tests (7 cookie tests) pass in both full runs; host cookie smoke 14/14 |
| (b) FTP | Probe + download + resume against the local fixture | Byte-exact, REST resume completes | PASS — proxy tests (15) pass in both full runs, incl. `ftp_probe_and_download_direct`, `ftp_pause_then_resume_completes_byte_exact` |
| (c) NTLM | Stub proxy right/wrong password; Kerberos-only proxy refused | Right password downloads byte-exact; wrong password 407; Kerberos-only a clear "not supported" error | PASS — proxy tests pass in both full runs, incl. `ntlm_proxy_authenticates_http_download`, `ntlm_proxy_wrong_password_is_rejected`, `kerberos_only_proxy_is_refused_clearly` |
| (d) site login | Gated server with/without saved credential | With login byte-exact; without 401; wrong password 401; not sent to another host | PASS — options tests (14) pass in both full runs; the page-scan half of this was FAIL→FIXED (BUG-025, below) |
| (e) sync Last-Modified-only | Server sends Last-Modified, no ETag; timestamp changed vs unchanged | Changed re-downloads; unchanged skips with 0 GETs | PASS — sync tests (23) pass in both full runs, incl. `sync_redownloads_on_last_modified_only_change`, `sync_skips_when_last_modified_unchanged` |
| (f) BUG-001–024 regressions | Every regression test by name | All pass | PASS — 37 named checks pass in the full-run output (see bugs.md Round 15); BUG-007 by source check; BUG-013/016 via host smoke / JS tests |
| (g) `cookies` manifest permission | Manifest test passes NOW; permission genuinely used | `cookiesFor(url)` in background.js reads cookies only for the exact handed-over URL | PASS — `extension_manifest_is_mv3_and_minimal` passes; permission used, not decorative; Python validator agrees |
| (h) cross-worker damage | Key symbols present and intact in all four workers' code | CookiePair, ftp_probe, NTLM Type 3, site-login store fns, sync Last-Modified handling, 9 Settings tabs | PASS — all found by grep; no worker's files clobbered by another |

### BUG-025 — page-scan probes never sent a saved site login (S3, FAIL→FIXED)

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| F07-V1 / F05-V4 | happy | Gated page + gated links, login saved for the host; Download All / video scan | Every link probes OK with its real size; gated clip listed downloadable | cargo test `verify2_page_scan_sends_saved_site_login` | FAIL→FIXED (BUG-025) |
| F07-V2 | negative | Same gated page, no login saved | Scan fails naming 401 | cargo test `verify2_page_scan_without_login_fails_gated_page` | PASS (control, before and after) |
| F07-V3 | negative: host scoping | Open page linking to a gated file; login saved for another host, then for the gated host | Other-host login: link stays 401; right-host login: probes OK with size | cargo test `verify2_page_scan_login_not_sent_to_another_host` | FAIL→FIXED (BUG-025) |
| F05-V5 | happy: HLS | Gated master playlist + gated variants, login saved; video scan | Master expands into 2 variants, both downloadable with real sizes | cargo test `verify2_hls_variant_scan_sends_saved_site_login` | FAIL→FIXED (BUG-025) |

Fix (see bugs.md BUG-025): `site_auth_for_url` (exact-host
only) threaded through the page GET, each link probe, the
playlist-text GET and each HLS variant probe in `manager.rs`.
One wrong test expectation corrected during the fix (a test
bug, not a product bug): an HLS variant's quality is the
honest dimension label "1280 × 720", not "720p".

### Regression (all earlier areas, re-run in full)

- Full suite: **330 total, 0 failed, 2 consecutive full runs**
  (see above). S01–S13, S06, S08, S05 sync and S03 progress
  suites all still pass in full.
- BUG-001–025: all FIXED, zero open (`grep "Status: OPEN"`
  returns nothing).
- Licence (S11) untouched: `features.csv` row still `no`, no
  licence/registration code in the app or Tauri shell (verified
  by search).
- Fresh replica-diff in this pass: **76.4 / 100, must-haves
  16 of 17** — the proxy row was reverted `yes` → `partial`
  by this pass (Kerberos unbuilt, in scope per recon F09); see
  `parity.md` and `features.csv`.

### Not tested in this round (same limits as rounds 1–13, plus the four finishes)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`);
  all new Tauri commands are uncompiled, and every manual UI
  case stays blocked.
- No Playwright e2e specs: Playwright/Chromium are not
  installed on this VM.
- No real browser, no real website, no Windows execution
  (RAS dial-up, DPAPI, Kerberos, Windows handshake, sound
  playback) — all unproven, as with every milestone.
- The site grabber crawl does not send site logins
  (login-required pages are reported, as documented) — a
  follow-up, not part of the BUG-025 fix.
- Pause/resume WITH cookies: no automated test
  (timing-sensitive) — code-reviewed only, as before.

## Round-14 summary

- Automated engine cases: 330 total, all passing (2
  consecutive full runs, 0 failures). 4 new final-verify2
  tests.
- UI build: `npm run build` passes; extension JS 44/44;
  host smoke 13/13 + 14/14 + 14/14 on a freshly rebuilt
  binary; `cargo check --tests` 0 warnings.
- Bugs found: 1 × S3 (BUG-025). Fixed failing-test-first,
  tests kept. See `bugs.md`.
- Parity re-run (this pass, `parity.py` on the verified
  `features.csv`): **76.4 / 100, must-haves 16 of 17 done**
  (proxy reverted to partial; `parity.md` rewritten from
  scratch).

## Update check (S09, should-have) — 2026-11-15

Extends the plan; it does not replace it. Build: slices 1–5 plus
S13 plus S08 plus S06 plus S05 periodic sync plus S09 finish plus
Kerberos, plus the update check. Env: same Linux VM, local
fixture manifest server only — no real update server, no IDM
servers, no public internet. Engine: `cargo test` in
`app/engine` (369 total: 50 unit + 112 integration + 32
extension + 4 final-verify2 + 16 final-verify + 28 grabber + 14
verifier + 14 options + 14 kerberos + 9 update + 7 progress +
15 proxy + 23 sync + 31 video, 0 failed, 2 consecutive full
runs). UI: `npm run build` passes (tsc + vite). No desktop
binary: the Tauri shell still does not compile here (missing
`gdk-3.0`), so the `check_for_updates` command is uncompiled,
and every case needing the running desktop app is marked
**blocked**.

### UPD — Quick update check

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| UPD-H1 | happy | Fixture serves version 0.2.0 + 2 changelog lines + installer address; manual check | `update_available`, latest 0.2.0, changelog shown, installer address present, `updater_configured: false`; download list still empty, exactly 1 manifest GET (nothing installed, nothing downloaded) | cargo test `newer_version_available_shows_changelog_and_never_installs` | PASS |
| UPD-H2 | happy | Fixture serves 0.1.0 (same as current), then 0.0.9, then 0.1.0-beta | No update in all three cases; the weekly channel stays silent when there is nothing new | cargo test `up_to_date_reports_no_update_and_sends_no_prompt` | PASS |
| UPD-H3 | happy: weekly due | Never checked, weekly on | Weekly run returns the update and the UI channel carries it | cargo test `weekly_check_runs_when_due_and_not_before_and_notifies_on_update` | PASS |
| UPD-H4 | happy: weekly not due | Checked just now, then 6 days ago | No fetch, no result (hits stay 1) | same test | PASS |
| UPD-H5 | happy: weekly due again | Last check 8 days ago | Due again, fetches again (hits 2) | same test | PASS |
| UPD-H6 | happy: tick | Fixture URL configured, never checked, `scheduler_tick()` | The existing 30 s tick spawns the check (hits ≥ 1 within 5 s); the next tick does not spawn a second one | cargo test `scheduler_tick_spawns_the_weekly_check_on_the_existing_tick` | PASS |
| UPD-E1 | edge: version compare | Unit cases | 1.10.0 > 1.9.9, 1.2.3 > 1.2.2, 1.2 == 1.2.0, `v` prefix and whitespace tolerated, pre-release < release (beta < beta.2 < beta.10, alpha < beta, rc.1 < release), build metadata ignored | unit tests `version_compare_is_numeric_not_lexical`, `prerelease_is_older_than_the_release` in `update.rs` | PASS |
| UPD-E2 | edge: manifest variants | `notes` string / `changes` alias instead of `changelog` | Changelog built from the notes lines / the `changes` list | unit test `manifest_parsing_accepts_changelog_notes_and_changes` | PASS |
| UPD-E3 | edge: weekly boundaries | Unit cases | Never checked = due; weekly off = never due; 6 days 20 h ago = not due; exactly 7 days ago = due; malformed stored time = due (never a crash) | unit test `weekly_due_logic` | PASS |
| UPD-E4 | edge: pre-update database | Settings table without the update columns | Columns added by migration; default URL is the placeholder, last check none; the new fields save | cargo test `update_columns_migrate_on_old_databases` | PASS |
| UPD-N1 | negative: malformed manifest | Invalid JSON; missing version; empty version; version "banana"; ftp:// installer address | Clear error naming the problem in each case; no crash, no phantom update; the failed check is still recorded (retries next week) | cargo test `malformed_manifest_is_a_clear_error_not_a_crash` | PASS |
| UPD-N2 | negative: network failure | Closed port; server answering 500; ftp:// update URL | "Could not reach the update server" / "status 500" / http(s)-only refusal — never a silent "up to date" | cargo test `network_failure_and_server_error_are_clear_errors` | PASS |
| UPD-N3 | negative: weekly off | Weekly setting off, never checked | Weekly run returns nothing, 0 requests; manual check still works and finds the update | cargo test `weekly_setting_off_skips_automatic_check_but_manual_still_works` | PASS |
| UPD-N4 | negative: bad update URL | Save "", "not a url", ftp://…, bare host | Rejected with "must be an http:// or https:// URL"; the previous URL stays stored | cargo test `update_settings_persist_validate_and_migrate` | PASS |
| UPD-M1 | manual: Help menu + dialog | Help > Check for updates; weekly find opens the dialog | Dialog shows current/latest version, changelog, installer address + Copy, "Remind me later"; up-to-date shows "up to date"; a failed check shows the error and "will retry next week" | blocked — no desktop binary (code reviewed: `App.tsx` Help menu + `UpdateDialog.tsx`; `npm run build` passes) | BLOCKED |
| UPD-M2 | manual: Settings field | Settings > General, update-server address + last-checked time | Address edits and persists; last-checked time shown | blocked — no desktop binary (code reviewed: `SettingsDialog.tsx`) | BLOCKED |
| UPD-T1 | to check: in-app install | Tauri updater plugin + signed release pipeline | Not built: `updater_configured` is false and the dialog offers the installer address instead of an Install button — see build-log.md | — | TO CHECK (Windows / release pipeline) |
| UPD-T2 | to check: real update server | Point `update_url` at the real release server | Never contacted on this VM (local fixture only); the placeholder must be replaced before shipping | — | TO CHECK (release server) |

### Regression (all earlier areas, re-run in full)

- Full suite: **369/369, 0 failed, 2 consecutive full runs**
  (was 354 — +6 unit, +9 update). Every earlier area passes
  by name in the output: add/pause/resume, categories,
  queues/scheduler/actions, Download All + batch, settings,
  proxy incl. Kerberos (14 tests), sync (23), video (31),
  grabber (28 + 14 verifier), progress (7), options (14),
  extension (32).
- `cargo check --tests` 0 warnings; `npm run build` passes.

### Not tested in this round (same limits as rounds 1–16, plus the update check)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`);
  the `check_for_updates` command is uncompiled, and UPD-M1/M2
  are blocked. The dialog and Help menu are verified via
  `npm run build` + source inspection only.
- No Playwright e2e specs: Playwright/Chromium are not
  installed on this VM.
- No real update server was ever contacted; the default URL is
  a placeholder that does not exist yet.
- The in-app install (Tauri updater plugin, signed releases)
  is not built — the dialog honestly says so and offers the
  installer address to copy.

## Round 17 — Download quota + per-download cap (S12) — 2026-11-15

Extends the round-16 plan; it does not replace it. Build: all
earlier slices plus S12 quota + per-download cap
(`engine/src/quota.rs`, `tests/quota_tests.rs`, 19 tests + 2 unit
tests in `quota.rs`). Env: same Linux VM, local mock HTTP server
only — no public internet, no IDM servers. Engine: `cargo test`
in `app/engine` (388/388, 0 failed — was 369, +19 quota + 2
unit). UI: `npm run build` passes. No desktop binary: the Tauri
shell still does not compile here (missing `gdk-3.0`), so the 4
new Tauri commands are uncompiled and every case needing the
running desktop app is marked **blocked**.

Quota semantics under test (chosen, documented in `quota.rs`):
window anchored at first use; counts actual downloaded bytes
only (segment writes — probe/HEAD bytes never count); reached
pauses the queue; rollover automatic on the 30 s tick / before
any start; manual action is Reset quota & resume now (fresh
window + resume). The clock seam is the `*_at(now)` methods:
tests drive a 1-hour window with `now + 3601`, never by waiting.

### S12 — Download quota

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S12-Q1 | happy | Set quota 5 MB / 24 h, read settings back | Enabled, values round-trip; turning it off keeps them | cargo test `quota_settings_validate_and_round_trip` | PASS |
| S12-Q2 | negative: validation | Quota 0 MB, 0 h, 1,000,001 MB, 721 h; enabled with no size/window | All rejected with a clear error, nothing stored | cargo test `quota_settings_validate_and_round_trip` | PASS |
| S12-Q3 | happy: window math | Record bytes via the store at fake times | First byte anchors the window; bytes accumulate inside it; a byte past the end anchors a fresh window | cargo test `quota_window_math_uses_the_clock_seam` | PASS |
| S12-Q4 | happy | 1.5 MiB download, quota 1 MB / 1 h | Pauses mid-file between 1 MB and 1.5 MB, marked quota-paused; status shows used >= 1 MB, window end = start + 1 h, reached, 1 paused download | cargo test `quota_reached_pauses_the_download_mid_file` | PASS |
| S12-Q5 | negative: start refused | While the quota is reached, start the paused download | Refused with a quota error naming the resume time; stays paused | cargo test `quota_reached_pauses_the_download_mid_file` | PASS |
| S12-Q6 | happy: queue stops | Queue with two 1.5 MiB files, quota 1 MB | First pauses, second stays queued, queue run ends | cargo test `quota_reached_stops_the_queue_and_leaves_the_rest_queued` | PASS |
| S12-Q7 | happy: disabled | Quota values stored but switch off, 2 MiB download | Completes byte-exact; usage stays 0 | cargo test `quota_disabled_never_pauses_and_counts_nothing` | PASS |
| S12-Q8 | happy: rollover | Paused by quota, then rollover check at now + 3601 s | Rolls over, download resumes and completes byte-exact; new-window usage is only the post-rollover bytes | cargo test `quota_window_rollover_resumes_the_paused_download` | PASS |
| S12-Q9 | happy: tick rollover | Queue paused by quota, scheduler tick at now + 3601 s | Tick rolls the quota over and resumes the paused file — but does NOT restart the queue run, so the second file stays queued | cargo test `quota_rollover_via_scheduler_tick_resumes_without_restarting_the_queue` | PASS |
| S12-Q10 | happy: reset | Paused by quota, Reset quota | Fresh window (usage 0), download resumes and completes byte-exact | cargo test `quota_reset_resumes_now_and_starts_a_fresh_window` | PASS |
| S12-Q11 | happy: restart | Paused by quota, drop the manager, reopen the same DB file | Usage + window start preserved; a new download still cannot start; reset lets the paused download finish byte-exact | cargo test `quota_window_survives_a_restart` | PASS |
| S12-Q12 | edge: user pause | Pause by hand before the quota is reached, then rollover at now + 3601 s | Window rolls (usage resets) but the download stays paused — only quota-paused downloads auto-resume | cargo test `user_paused_download_is_never_auto_resumed_by_a_rollover` | PASS |
| S12-Q13 | happy: migration | Open a hand-built pre-S12 database | Quota columns/state exist with defaults (off, no limit, 0 used); cap + quota-paused flags usable at once | cargo test `pre_quota_database_migrates_quota_columns_and_state` | PASS |
| S12-M1 | manual: Settings quota | Options > Connection: enable, MB, hours, usage + window end, Reset button | Values persist; usage/window shown; Reset resumes now | blocked — no desktop binary (code reviewed: `SettingsDialog.tsx`; `npm run build` passes) | BLOCKED |
| S12-M2 | manual: status bar | Quota enabled | Usage shown; when reached, "resumes HH:MM" shown, opens Settings | blocked — no desktop binary (code reviewed: `App.tsx`) | BLOCKED |
| S12-T1 | to check: system clock | User changes the system clock mid-window | Window follows wall-clock Unix time, as the scheduler does (documented, not tested) | — | TO CHECK |

### S12 — Per-download cap

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S12-C1 | happy | 256 KiB download, per-download cap 32 KiB/s, global unlimited | Takes ~7 s (>= 5 s, < 25 s), byte-exact | cargo test `per_download_cap_throttles_one_download` | PASS |
| S12-C2 | happy | Per-download cap 0, global unlimited | Completes in < 5 s (cap 0 = global cap only) | cargo test `per_download_cap_zero_means_global_cap_only` | PASS |
| S12-C3 | happy: min wins (global binds) | Global 32 KiB/s, per-download 128 KiB/s, 256 KiB | Takes >= 5 s (global binds, not 128) | cargo test `per_download_cap_and_global_cap_compose_min_wins` | PASS |
| S12-C4 | happy: min wins (per-download binds) | Global 128 KiB/s, per-download 32 KiB/s, 256 KiB | Takes >= 5 s (per-download binds, not 128) | cargo test `per_download_cap_and_global_cap_compose_min_wins` | PASS |
| S12-C5 | happy: two at once | Global 64 KiB/s; A capped 16 KiB/s, B uncapped; both 256 KiB | B finishes first while A is still downloading; both byte-exact | cargo test `per_download_cap_throttles_one_download_while_another_runs_at_the_global_cap` | PASS |
| S12-C6 | happy: live clear | 256 KiB at 16 KiB/s, clear the cap after 2 s | Finishes in < 12 s (vs ~15 s if the cap stayed) | cargo test `per_download_cap_can_be_cleared_live_mid_download` | PASS |
| S12-C7 | negative: validation | Cap -5, 10,000,001 KB/s, unknown download id | Rejected (same 10,000,000 bound as the global cap, BUG-008); 10,000,000 accepted | cargo test `per_download_cap_validation_rejects_absurd_and_negative_values` | PASS |
| S12-C8 | manual: add dialog + progress | S02 cap field, S03 dialog | Cap stored before start; dialog shows "capped at X KB/s" and the quota-paused note when applicable | blocked — no desktop binary (code reviewed: `AddDownloadDialog.tsx`, `ProgressDialog.tsx`) | BLOCKED |

### S12 — Sync counts toward the quota

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S12-S1 | happy | Finished 512 KiB file in a periodic queue; enable quota 1 MB; bump server (513 KiB); sync | changed = 1; quota usage is exactly 513 KiB — the re-download counted, the HEAD probe counted nothing | cargo test `sync_redownload_counts_toward_the_quota` | PASS |
| S12-S2 | happy | Finished 2 MiB file; enable quota 1 MB; bump server; sync | Sync reports failed naming the quota, in < 60 s (not the 120 s re-download timeout); file stays paused + quota-paused, retried at the next sync | cargo test `sync_redownload_paused_by_quota_fails_fast_not_after_120s` | PASS |

### Regression (all earlier areas, re-run in full)

- Full suite: **388/388, 0 failed** (was 369 — +2 unit in
  `quota.rs`, +19 quota). Every earlier area passes by name in
  the output: add/pause/resume, categories, queues/scheduler/
  actions, Download All + batch, settings, proxy incl. Kerberos
  (14 tests), sync (23), video (31), grabber (28 + 14 verifier),
  progress (7), options (14), extension (32). BUG-008 regression
  test (`speed_limit_absurd_value_is_rejected`) passes by name.
- `cargo check --tests` 0 warnings; `npm run build` passes.

### Not tested in this round (same limits as rounds 1–16, plus S12)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`);
  the 4 new commands are uncompiled, and S12-M1/M2/C8 are
  blocked. The quota UI is verified via `npm run build` +
  source inspection only.
- No Playwright e2e specs: Playwright/Chromium are not
  installed on this VM.
- A quota enabled mid-download applies to the next download,
  not the running one (like `max_connections`) — documented in
  `quota.rs` and the build log, not tested as a failure.

## Round 19 — Antivirus hook (S14) — 2026-11-15

Extends the round-18 plan; it does not replace it. Build: all
previous slices plus S14 (hook only — the scanner is external).
Env: same Linux VM, local mock HTTP server + fake scanner script
fixtures only — no real antivirus product was ever run, no
malware sample (not even EICAR) was fetched, created or used.
Engine: `cargo test` in `app/engine` (**410/410, 0 failed** —
was 390; +17 antivirus tests in `tests/antivirus_tests.rs` and
+3 unit tests in `antivirus.rs`). UI: `npm run build` passes;
`cargo check --tests` 0 warnings. No desktop binary: the Tauri
shell still does not compile here (missing `gdk-3.0`), so the 5
new Tauri commands are uncompiled and every case needing the
running desktop app is marked **blocked**.

### S14 — Automatic antivirus scan on completion

| case | type | steps | expected | auto | result |
| --- | --- | --- | --- | --- | --- |
| S14-H1 | happy: clean | Fake scanner exits 0, download 512 KiB | Scan `clean`, stored on the row, file byte-exact, scanned path is the final file (never the `.part`), Open allowed | cargo test `clean_scanner_reports_clean_and_scans_the_final_path` | PASS |
| S14-H2 | happy: threat | Fake scanner exits 1 with "threat found: FakeTest.Threat" | Scan `infected`, detail carries the scanner's report, file NOT auto-deleted or auto-quarantined, Open refused with a threat error, Quarantine moves it into a `Quarantine` folder and re-points the row | cargo test `threat_scanner_reports_infected_and_nothing_is_auto_deleted_or_opened` | PASS |
| S14-H3 | happy: exit 0, threat in output | Fake scanner exits 0 printing "… FakeTest.Threat FOUND" | Scan `infected` (ClamAV-style output counts) | cargo test `exit_zero_with_a_threat_in_the_output_is_still_infected` | PASS |
| S14-H4 | happy: completion event | Download with no scanner, subscribe to progress events | Completion event carries `scan: not_configured`; row agrees | cargo test `completion_event_carries_the_scan_result` | PASS |
| S14-H5 | happy: re-scan | Complete with no scanner, then configure a clean scanner and re-scan | Manual `run_antivirus_scan` returns `clean`, stored result updates | cargo test `manual_rescan_after_completion_records_a_new_result` | PASS |
| S14-H6 | happy: persisted | Clean scan, then reopen the same DB file with a fresh Store | Stored result survives (`clean`, `scanned_at` set) | cargo test `scan_result_is_persisted_across_a_store_reopen` | PASS |
| S14-N1 | negative: missing scanner | Command points at a non-existent program | Scan `scan_failed` with "Could not start"; download still completes | cargo test `missing_scanner_command_is_a_clear_scan_failure` | PASS |
| S14-N2 | negative: not configured | No command in settings | Download completes normally, scan `not_configured` with a clear note | cargo test `no_scanner_configured_completes_normally_with_not_configured` | PASS |
| S14-N3 | negative: turned off | Command set, `antivirus_enabled` off | Scan `not_configured` ("turned off"), scanner never executed (record file absent), manual re-scan same | cargo test `scanning_turned_off_records_not_configured_and_never_runs_the_scanner` | PASS |
| S14-N4 | negative: unfinished | Scan a download that was added but never started | Error "Only a finished download can be scanned" | cargo test `scan_is_refused_for_an_unfinished_download` | PASS |
| S14-N5 | negative: quarantine a clean file | Clean scan, then quarantine | Refused: "Only a file the antivirus scan flagged…" | cargo test `quarantine_is_refused_for_a_clean_file` | PASS |
| S14-E1 | edge: slow scanner | Scanner sleeps 30 s, timeout 5 s | Download completes at once with scan `scanning` (completion never waits); app stays responsive while scanning; after ~5 s the scan is `timed_out` ("did not finish within 5 seconds") — not 30 s | cargo test `scan_does_not_block_completion_and_a_slow_scanner_times_out` + `run_scan_timeout_kills_a_sleeping_scanner_quickly` | PASS |
| S14-E2 | edge: settings + Test | Save command + enabled + timeout 120, read back; Test button logic vs clean / threat / missing / empty command | Round-trip exact; Test returns `clean` / `infected` / `scan_failed`; empty command is an error ("Enter a scanner command first") | cargo test `settings_round_trip_and_test_button_use_the_fake_scanner` | PASS |
| S14-E3 | edge: timeout validation | Save timeout 0 / 4 / 601 / 3600, then 5 / 60 / 600 | Absurd values rejected with a "scan timeout" error, nothing stored; bounds accepted | cargo test `scan_timeout_validation_rejects_absurd_values` | PASS |
| S14-E4 | edge: pre-S14 database | Open a DB made before S14 (no scan columns, no antivirus switch/timeout) | Migration adds them (enabled ON, timeout 60), old complete row reads `scan_status: None`, `set_scan_result` works on it | cargo test `pre_s14_database_migrates_scan_columns_and_settings` | PASS |
| S14-R1 | regression: queue reads | Add a download to the main queue, list its files | Files read back with `scan_status: None` (was: `Db(InvalidColumnIndex(20))` — every queue read failed) | cargo test `queue_files_read_back_with_scan_columns` | FAIL→FIXED (BUG-026) |
| S14-U1 | unit: command split | Quoted Windows path with spaces, `{file}` in quotes, unterminated quote, empty template | Tokenised correctly; unterminated quote / empty template are errors | unit tests in `antivirus.rs` (`split_command_handles_quotes_and_spaces`, `scan_timeout_bounds_are_rejected_not_clamped`, `output_threat_markers`) | PASS |
| S14-M1 | manual: Settings | Options > Downloads virus-check section (enable, command, timeout, Test) | Values persist, Test reports the scanner's verdict, note says Fetchline includes no scanner | blocked — no desktop binary (code reviewed: `SettingsDialog.tsx`) | BLOCKED |
| S14-M2 | manual: dialog | S14 completion dialog (file path, Open / Open folder, scan result, Scan again, Quarantine / Delete on a threat) | Open disabled on a threat, Quarantine/Delete only on a threat, scan states shown in words | blocked — no desktop binary (code reviewed: `ProgressDialog.tsx`) | BLOCKED |
| S14-T1 | to check: real scanner | Windows Defender (MpCmdRun) against a real finished download on Windows | Real exit codes, output text and timing; a genuine clean/threat verdict end to end | — | TO CHECK (Windows) |

### Regression (all earlier areas, re-run in full)

- Full suite: **410/410, 0 failed** (was 390 — +17 antivirus,
  +3 unit in `antivirus.rs`). Every earlier area passes by name
  in the output: add/pause/resume, categories, queues/scheduler/
  actions, Download All + batch, settings, proxy incl. Kerberos
  (14 tests), sync (23), video (31), grabber (28 + 14 verifier),
  progress (7), options (14), extension (32), update (9),
  quota (19). BUG-026 regression test
  (`queue_files_read_back_with_scan_columns`) passes by name.
- `cargo check --tests` 0 warnings; `npm run build` passes.

### Not tested in this round (same limits as rounds 1–18, plus S14)

- No desktop binary / Tauri shell: `cargo check` in
  `app/src-tauri` still fails on this VM (missing `gdk-3.0`);
  the 5 new commands are uncompiled, and S14-M1/M2 are blocked.
  The Settings section and the completion dialog are verified
  via `npm run build` + source inspection only.
- No Playwright e2e specs: Playwright/Chromium are not
  installed on this VM.
- No real antivirus product was ever run (fake scanner fixture
  only); Windows Defender / real AV behaviour on Windows
  remains unproven (see S14-T1).

## Round 20 — Real download + browser capture on the Windows runner (CI) — 2026-11-15

The launch smoke test (run 43686477003, green) proved the app
opens on real Windows. These two close the next gaps: that it
actually DOWNLOADS on Windows, and that a browser link reaches
it on Windows. Both run in CI only; neither changes product
behaviour.

| ID | case | input | expected | how | result |
| --- | --- | --- | --- | --- | --- |
| RD-H1 | happy: real download, pinned file | LICENSE from a pinned commit (7255dff9) via raw.githubusercontent.com, through the real DownloadManager | Completes; saved file is 1,079 bytes, SHA-256 `c2ef3696…f793` | cargo test `real_download_of_pinned_license_matches_sha256` (gated: `FETCHLINE_REAL_DOWNLOAD_TEST=1`) + CI step "Real download test (fails the run)" | PASS on CI (dedicated step); SKIPPED locally (TLS-intercepting proxy — see below) |
| RD-H2 | happy: real probe | Same pinned URL | Probe reports size 1,079 | cargo test `real_probe_of_pinned_license_reports_size` (same gate) | PASS on CI (dedicated step); SKIPPED locally |
| CAP-H1 | happy: host protocol | Real `fetchline-host.exe`, real Native Messaging frames (ping, get_settings, download_link) against the app's real `app.db` | ping pong; get_settings browser_integration=true; download_link ok with pending_link_id; row in `pending_links` (source `browser`, status `pending`) | `scripts/capture-test.py` Phase 1 (CI job `extension-capture-test`) | PASS (dry-run locally vs the real host binary; full run on CI) |
| CAP-H2 | happy: real browser transport | Unpacked extension in headless Chrome; host manifest + registry key written as a user install would; `popup.html` opened, status polled over CDP | Popup reports "Connected to the Fetchline desktop app" | `scripts/capture-test.py` Phase 2 | TO CHECK (CI — reviewed-only on this VM, no Chrome here) |
| CAP-H3 | happy: drain to file | Installed app drains the queue and downloads the pinned LICENSE | File in `%USERPROFILE%\Downloads\LICENSE`, SHA-256 matches | `scripts/capture-test.py` Phase 3 | TO CHECK (CI) |
| CAP-N1 | negative: human click | Right-click "Download with Fetchline" / download watcher in a real browser | Cannot be driven headlessly — no mouse, no real popup window | — | NOT PROVEN (same transport as CAP-H2; menu/watcher wiring covered by Node fake-`chrome` tests) |
| CAP-F1 | finding: packaging gap | Install the MSI, look for `fetchline-host.exe` and the native-messaging registry key | The MSI ships NEITHER (no bundle config in `tauri.conf.json`; the Tauri app does not depend on the host crate) | Inspection (`tauri.conf.json`, `app/src-tauri/Cargo.toml`, repo host manifests) | FINDING — the capture job builds + registers the host itself, runner-only; a user install cannot capture until the installer ships them |

### Regression (all earlier areas, re-run in full)

- Full suite: **413/413, 0 failed** (was 411 — +2 real-download
  tests, SKIPPED ungated on this VM). `cargo check --tests`
  0 warnings; `npm run build` passes.

### Not tested in this round (same limits as rounds 1–19, plus)

- The real-download tests SKIP on this Linux VM by design: it sits
  behind a TLS-intercepting egress proxy (Hatch Sandbox Egress CA)
  that the engine's rustls client (compiled-in webpki roots)
  rejects. They run for real only where the env var is set — the
  CI dedicated step. Never weaken TLS verification to make them
  pass locally.
- Phase 2 of the capture test (headless Chrome + CDP) is
  reviewed-only here: no Chrome on this VM. The CI run is the
  proof; if it proves flaky, the failure names the phase.
- No desktop binary / Tauri shell on this VM (missing `gdk-3.0`)
  — unchanged.
