# Architecture: IDM rebuild — Windows Desktop Application

Source: `replica/recon.md` + `replica/features.csv` (14 screens, 10 flows, 9 entities).
Constraint from the user: **Desktop Application on Windows, not web.**

## Stack

| layer | choice | why |
| --- | --- | --- |
| App shell | **Tauri 2** | Native Windows app, tiny binary (~10 MB vs ~150 MB for Electron), uses the system's WebView2, real native window/tray/menus |
| Engine | **Rust** (Tauri core) | The download engine lives in the same Rust process: true threads, no GC pauses, exact control of sockets, files and offsets |
| UI | **TypeScript + React** (in WebView2) | Fast to build the table/tree/dialogs from the recon (S01-S14); tokens come from replica-design later |
| HTTP | `reqwest` + `tokio` | Async HTTP, HTTP/2, Range requests, proxy + auth support, connection pooling |
| Database | **SQLite** (via `rusqlite`, WAL mode) | Single-user desktop app: no server, one file in `%APPDATA%`, survives crashes, fast enough for thousands of rows |
| Browser extension | WebExtension (Manifest V3) + **Native Messaging** | Extension in Chrome/Firefox/Edge talks to the desktop app over stdin/stdout JSON; no private APIs, user's own browser only |
| Packaging | `tauri build` → **MSIX / NSIS installer** | Windows install, auto-update via Tauri updater, Start Menu + file associations |
| Licence | Local licence key check (offline, signed) | No server needed for v1; a server check can be added later without changing the schema |
| Tests | `cargo test` + Playwright for the UI | Engine tests run without a network (mock server); UI tests drive the real flows |

### The alternative, if you know .NET better

**WinUI 3 + C# (.NET 8)** is the most native Windows choice: easiest tray,
context menu, drag-and-drop, shutdown-on-completion and Windows DPAPI, one
language end to end. Pick it if the builder is a C# developer. It costs:
Windows-only, a bigger runtime, and slower UI iteration than React.
Do **not** pick Electron: it ships a whole Chromium per app, uses far more
RAM, and buys nothing the engine needs.

### What was deliberately NOT chosen

- **No Postgres / Supabase / web backend.** This is a local, single-user
  desktop app. A server adds cost, latency and a privacy problem for zero
  benefit in v1.
- **No microservices.** One process: UI + engine + scheduler + SQLite.
- **No Electron.** See above.

## Schema

Tables: 9. Access rules: **none needed server-side** — single local user,
one SQLite file, owned by the Windows account. Secrets (site logins, proxy
passwords) are encrypted with Windows DPAPI before storage; the DB never
holds a plaintext password. Full SQL: `replica/schema.sql`.

Key tables (see schema.sql for the full DDL):

- `downloads` — one row per download: url, final_url, filename, save_path,
  category_id, queue_id, size_bytes, downloaded_bytes, status, speed, ETA,
  referrer, headers/cookies JSON, error, timestamps.
- `segments` — one row per segment of a download: index, start_byte,
  end_byte, downloaded_bytes, status, connection id. Unique
  `(download_id, segment_index)`.
- `queues` — main + custom queues, schedule type (one_time / periodic),
  start/stop time, start-on-startup, on-complete action.
- `categories` — builtin + custom, default folder, file extensions.
- `grabber_projects` — site-grabber projects: start URL, filters, depth,
  mode, schedule, status.
- `site_logins` — host, username, encrypted password, auth type.
- `proxy_configs` — type, host, port, encrypted credentials.
- `settings` — singleton row: default folder, max connections, speed limit,
  quota, language, theme, browser integration, antivirus command, file
  types, excluded sites.
- `licence` — singleton row: owner, serial, status, trial expiry.

Hard constraints (database, not UI):

- A segment's byte range must lie inside its download:
  `start_byte >= 0 AND end_byte < size_bytes` (when size is known).
- Two segments of the same download must not overlap — enforced by a
  `BEFORE INSERT` trigger in schema.sql, because SQLite has no exclusion
  constraint. The engine splits only by halving the largest segment, so
  this should never fire; it is a safety net.
- A download in `complete` status must have
  `downloaded_bytes = size_bytes` (when size is known) — enforced by a
  `CHECK` on status transitions in the engine, and a trigger.
- Deleting a download cascades to its segments
  (`ON DELETE CASCADE`); deleting a category is blocked while downloads
  reference it (`ON DELETE RESTRICT`) — the user must move them first.
- `downloads.status` is a `CHECK` enum:
  `queued | downloading | paused | complete | error | deleted`.
- Indexes on every foreign key and every filtered/sorted column:
  `downloads(status, created_at)`, `downloads(category_id)`,
  `downloads(queue_id)`, `segments(download_id, segment_index)`.

## API — internal, not HTTP

There is no web API. The UI calls the Rust core through Tauri commands, and
the browser extension talks to the app through Native Messaging (JSON over
stdin/stdout). One command per flow:

| command | does | input | output | flow |
| --- | --- | --- | --- | --- |
| `add_download(url, filename?, folder?, category?)` | Probe URL (HEAD), create download + segments | URL | download id + S02 prefill | F01, F02 |
| `start_download(id)` / `pause_download(id)` / `resume_download(id)` / `cancel_download(id)` | Engine control | id | status | F01, F02, F03 |
| `delete_download(id, delete_file?)` | Remove row (+ file) | id | ok | S01 |
| `list_downloads(category?, queue?, status?)` | Main table | filters | rows + totals | S01 |
| `create_queue(name)` / `update_queue(id, schedule)` / `start_queue(id)` | Queue + scheduler | queue fields | queue | F04 |
| `add_to_queue(download_id, queue_id)` | Move a download into a queue | ids | ok | F04 |
| `add_batch(urls[], category?)` | Validate lines, create one download each | URL list | created ids + invalid lines | F08 |
| `download_all_links(page_url, links[])` | Create downloads from a page checklist | links | created ids | F07 |
| `create_grabber_project(url, filters, depth)` / `run_grabber(id)` | Site grabber | project fields | project id | F06 |
| `get_settings()` / `update_settings(patch)` | Options dialog | settings fields | settings | F09 |
| `set_speed_limit(kbps?)` / `set_download_limit(mb, hours)` | Limits | values | ok | S12 |
| `get_categories()` / `create_category(name, folder, extensions)` | Categories | fields | category | F10 |
| `register_licence(name, serial)` | Licence activation | name + serial | status | S11 |
| `check_for_updates()` | Weekly update check | — | latest version | S09 |

Native Messaging (extension → app):

| message | does | flow |
| --- | --- | --- |
| `download_link {url, referrer, cookies}` | Catch a browser click, open S02 | F01 |
| `download_all {page_url, links[]}` | Open S07 checklist | F07 |
| `video_found {page_url, variants[]}` | Show S08 panel with qualities | F05 |

Background jobs (in-process, not cron):

| job | schedule | what it does |
| --- | --- | --- |
| Scheduler tick | every 30 s | Start/stop queues whose time arrived; run periodic sync queues |
| Resume-on-startup | on launch | Resume downloads that were active at shutdown, if the queue says so |
| Update check | weekly | Check for a new version, list changes, ask before updating |
| Segment checkpoint | every 2 s during a download | Persist segment progress to SQLite so a crash loses at most 2 s |

## The parts that bite

- **Resume is a negotiation, not a flag.** Some servers ignore `Range`,
  some lie about `Content-Length`, some change the file mid-download.
  The engine must re-validate with `ETag`/`Last-Modified` and restart
  from zero when the file changed — never silently stitch a wrong file.
- **Dynamic segmentation races.** Splitting the largest segment while
  other connections write to the same file needs exact offsets and a
  per-download write lock; a 1-byte overlap corrupts the file.
- **Speed limiter.** A token bucket per download and a global bucket;
  limiting must not starve the UI thread or the checkpoint job.
- **Browser extension trust.** The extension sends cookies/headers for
  logged-in sites. Only accept Native Messaging from the installed
  extension id, and never log cookies or credentials.
- **Shutdown-on-completion.** Windows shutdown needs the `SeShutdownPrivilege`
  path and a user-visible countdown with a cancel option — never shut
  down silently.
- **Time zones / scheduler.** Queue times are stored in UTC, displayed in
  the user's local time; a scheduled queue must still fire after a
  daylight-saving change.
- **Partial files.** Downloads live as `.part` files until complete, then
  are atomically renamed; a crash must never leave a "complete" file that
  is actually partial.

## Download Engine — how it works

1. **Probe.** `HEAD` (fall back to `GET` with `Range: bytes=0-0`) to learn
   size, `Accept-Ranges: bytes`, `ETag`, `Last-Modified`, filename from
   `Content-Disposition`. If size is unknown or the server refuses ranges,
   download as a single segment.
2. **Segment.** Start with `min(max_connections, 8)` segments for a known
   size; each segment is a `Range` request on its own tokio task.
   **Dynamic split:** when a new connection frees up, find the largest
   unfinished segment and halve it — exactly IDM's documented behaviour.
3. **Write.** Pre-allocate the `.part` file to the full size (when known),
   each task writes only at its own offsets under a write lock, and
   checkpoints its progress to SQLite every 2 s.
4. **Resume.** On pause / crash / network drop, persist every segment's
   `downloaded_bytes`. On resume, re-probe the server; if `ETag` or size
   changed, restart the download; otherwise re-open each unfinished
   segment at its last byte.
5. **Retry.** Exponential backoff per segment (1 s, 2 s, 4 s, … up to
   5 attempts, then the download goes to `error` with the message shown).
6. **Finish.** When all segments are done, verify total bytes, rename
   `.part` → final file, run the antivirus hook if configured, then fire
   the queue's on-complete action.
7. **Limits.** A token bucket caps bytes/second globally and per download;
   the quota (`X MB every Y hours`) pauses the queue when reached.

## Build order

### Recommendation: start with M, not L

**Start with M (a few weeks).** The L build adds the grabber, the video
panel and the browser extension — the three riskiest parts — before the
core loop is proven. Build M first, ship it, then add L on top of a
working engine. The schema already covers L, so nothing is thrown away.

1. **Vertical slice (M, week 1).** F01 + F02 + F03 end to end, ugly:
   S01 main window, S02 add dialog, S03 progress, SQLite, engine with
   single + multi-segment download and resume. Tables: downloads,
   segments, categories, settings.
2. **Must-haves, M (weeks 2-3).** Queues + scheduler (F04, S05),
   categories (F10, S04), batch + download-all (F08, F07), Options
   dialog (F09, S09), proxy/auth, speed limiter + quotas (S12).
3. **Should-haves (week 4).** Dark theme, custom toolbar/skins,
   antivirus hook, update check, licence/trial (S11, S14).
4. **L additions (after M ships).** Video grabber (F05, S08) +
   browser extension, site grabber (F06, S06), periodic sync queues.
5. **Fixes from replica-entrepreneur**, once it has run.

Each milestone lists its screens (S-IDs), tables and commands in the
table above.

## Deferred — the hard features to postpone

- **Site grabber (F06, S06).** A polite crawler with depth, filters and
  deduplication is a project of its own. Defer to L.
- **Video grabber + browser extension (F05, S08, S13).** Cross-browser
  video detection and Native Messaging are the second-hardest part, and
  they depend on the engine being solid first. Defer to L.
- **Periodic synchronisation queues.** One-time scheduling is M; sync
  (diffing a remote listing against local files) is L.
- **Advanced browser integration** (catching downloads from any app).
  Out of scope for v1 — see recon.
- **Dial-up, sounds, full multilingual UI.** Legacy / polish; add after
  the core loop works.
