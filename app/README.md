# Sonic Manager — desktop download manager (vertical slice)

[![Windows build](https://github.com/mohandksonic-ui/fetchline/actions/workflows/build-windows.yml/badge.svg)](https://github.com/mohandksonic-ui/fetchline/actions/workflows/build-windows.yml)

> Windows installer: every push to `main` builds one automatically — see the
> [root README](../README.md#download-windows-no-setup-needed).

Clean-room rebuild of download-manager behaviour, from
`../replica/recon.md` / `../replica/architecture.md` / `../replica/design/`.
**Sonic Manager**. No code, assets, copy or branding from the original application is
used anywhere in this project.

## What this slice does (vertical slice only)

- **S02 — Add a new download:** URL, file name, save folder, category.
  The URL is probed first (HEAD, with a ranged-GET fallback) for size,
  range support, ETag and file name.
- **Start download:** multi-segment download with HTTP `Range` requests
  and dynamic splitting of the largest segment when a connection frees up.
- **S01 — Main window:** live download table (name, size, status, progress,
  speed, time left), categories sidebar, toolbar, status bar, dark theme.
- **S03 — Progress dialog:** live speed/ETA, one bar per segment,
  pause / resume / cancel.
- **Pause / resume:** segment progress is checkpointed to SQLite every
  ~2 s and on pause. Resume re-probes the server; if the size or ETag
  changed, the download restarts from zero instead of stitching a wrong file.
- **S04 — Categories:** builtin categories with counts, custom categories
  (name, folder, file types), auto-assign by file type, move by
  drag-and-drop or toolbar menu, delete blocked while downloads
  reference the category.
- **S05 — Queues and scheduler:** main queue + custom queues, "Save for
  later" files into the main queue, reorder, Start Now drains in order,
  stop pauses the current file. One-time schedules (start/stop time,
  start on startup) on a 30 s tick, and an on-completion action
  (open file / run program / shutdown / hibernate, the last two with a
  60 s countdown and cancel). Periodic synchronisation is deferred.
- **S07 — Download All:** fetch a page, list its links in a checklist
  grouped by file type with a type filter, select all / none / invert,
  live selected count and total size. Dead links are listed with their
  error and excluded; links already downloaded are flagged and left
  unchecked. Start now or add to a queue, each link auto-assigned to
  its category.
- **S10 — Batch download:** one link per line; blank lines ignored,
  invalid lines listed with their line number and skipped, duplicates
  inside the batch flagged. Start now or add to a queue, reusing the
  single-add path (probe, category auto-assign, duplicate-file guard).

- **S09 — Settings:** dialog with General / File Types / Connection /
  Save To / Downloads / Proxy-Socks tabs, persisted in the `settings`
  table and applied live — max connections (1–16) actually changes the
  engine's segment count, per-category folders and extension lists,
  default folder, default queue action (start now / save for later),
  duplicate handling, confirm-before-remove, theme. Proxy: HTTP /
  SOCKS5 with Basic auth, bypass list, Test button; the engine really
  routes probe + download requests through it. FTP is stored but not
  applied (no FTP transport); NTLM/Kerberos are not supported.
  Passwords are DPAPI-protected on Windows; on this Linux VM they are
  only base64-obfuscated (`b64:` — not encryption — the DPAPI path is
  untested on Windows). Dial-Up, Sounds and Site logins tabs are
  deferred; start-on-startup is a stored flag only.
- **S12 — Speed limiter:** global KB/s cap (0 = unlimited) enforced by
  a shared token bucket across all active downloads, live-changeable,
  shown in the status bar as a button that opens Settings. MB-per-hours
  quota and per-download caps are not built.

Not built yet: site grabber, video grabber, browser extension, licence,
periodic synchronisation.
See `../replica/build-log.md`.

## Layout

```
app/
  engine/            Rust download engine (no Tauri dependency)
    src/
      lib.rs         module map
      probe.rs       HEAD probe (+ ranged-GET fallback)
      store.rs       SQLite layer (schema embedded from replica/schema.sql)
      manager.rs     segmented download, pause/resume, retry, events,
                     queue runner + scheduler tick
      queue.rs       completion actions (open file / run program /
                     shutdown / hibernate)
      limiter.rs     global token-bucket speed limiter
      proxy.rs       proxy client build + validation (HTTP/SOCKS5)
      secret.rs      password protection (DPAPI on Windows,
                     base64-obfuscated elsewhere — NOT encryption)
      timeutil.rs    UTC ISO-8601 helpers for the scheduler
      error.rs       user-facing errors
    tests/
      engine_tests.rs  mock HTTP server (std TcpListener, no internet)
      common/mock.rs   shared mock server + helpers + local HTTP/SOCKS5
                       proxy servers
  src-tauri/         Tauri 2 shell: commands + progress events
    src/lib.rs       download commands, category + queue commands,
                     scheduler tick (30 s) + queue-action events,
                     settings + proxy + speed-limit commands
    tauri.conf.json  Windows targets: MSI + NSIS
  src/               React + TypeScript UI (Tailwind, design tokens only)
    components/      Button, ProgressBar, StatusBadge, DownloadTable,
                     AddDownloadDialog, ProgressDialog, CategorySidebar,
                     AddCategoryDialog, AddQueueDialog, SchedulerDialog,
                     QueueActionDialog, SettingsDialog
    lib/             api.ts (Tauri bridge), types.ts, format.ts
```

The database lives in the OS app-data folder
(`%APPDATA%\fetchline\app.db` on Windows, `~/.local/share` on Linux).

## Run on Windows (the target platform)

Prerequisites:

- **Rust** (stable) — https://rustup.rs/ — and the MSVC build tools
  (`Visual Studio Build Tools`, "Desktop development with C++" workload).
- **Node.js 20+** and npm.
- **WebView2** — ships with Windows 10/11; install the Evergreen runtime
  from Microsoft if it is missing.

```powershell
cd app
npm install
npm run build          # type-check + build the React UI into dist/
cargo tauri dev        # or: cargo install tauri-cli@2
```

Release build (produces the MSI + NSIS installer in
`src-tauri/target/release/bundle/`):

```powershell
cargo tauri build
```

## Run / verify on Linux (this VM)

Rust and Node are already installed here. Tauri's desktop binary needs
GTK 3 + WebKitGTK system libraries, which this VM does not have
(`apt-get install libgtk-3-dev libwebkit2gtk-4.1-dev` failed here with
"held broken packages"), so **no Linux desktop binary was built and no
Windows .exe exists**. What was verified on this VM:

```bash
cd app/engine && cargo test
# 81 integration tests + 12 unit tests, 0 failed, against local mock
# HTTP / proxy servers (no public internet): probe, probe fallback,
# full multi-segment download (byte-exact), pause → resume
# (byte-exact), resume after remote file change (restarts from zero,
# byte-exact), schema load + seeds, queue drain order, queue stop +
# restart, scheduler start/stop/one-time-no-restart, reorder, category
# auto-assign, category delete blocked then allowed, periodic rejected,
# run-program action + dismiss, shutdown errors off Windows, download
# all + batch, settings round-trip/validation/migration, segment count
# from settings, category folder/extension edits, duplicate overwrite,
# HTTP + SOCKS5 proxy (incl. auth and bypass), speed cap / unlimited /
# mid-download cap change.
# `cargo check --tests` on the engine is warning-free.

cd app && npm install && npm run build
# tsc + vite build, Tailwind processed (12.6 kB CSS, token classes present).

python3 -c "import sqlite3; sqlite3.connect(':memory:').executescript(
  open('../replica/schema.sql').read())"
# schema.sql loads; 9 tables, 5 seed categories, main queue seed,
# downloads.queue_position column + index.
```

**Tauri shell: NOT verified on this VM.** `cargo check` in `src-tauri`
fails here because the Tauri system library `gdk-3.0` is missing
(`pkg-config` error from crate `gdk-sys`), and it cannot be installed:
`apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev` fails with
"held broken packages" — `libzstd-dev` requires `libzstd1`
(`= 1.5.5+dfsg2-2build1.1`) but this image has the pinned
`1.5.5+dfsg2-2build1.1+hatch1` installed. This is a Linux-VM limitation
only: on Windows, Tauri uses the WebView2 runtime and needs no GTK.
**No Windows .exe exists.** The Tauri command code
(`src-tauri/src/lib.rs`) is written per the architecture and follows the
same engine API the tests exercise, but it has not been compiled.

## Design rules enforced in this slice

- Components use only the design tokens (`src/index.css` custom
  properties via Tailwind classes). No raw hex or pixel values in
  components; no IDM brand, copy or assets.
- All labels and empty states are written fresh.
