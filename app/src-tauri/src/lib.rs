// Tauri shell for Fetchline.
//! The engine (../engine) does all the work; this crate only wires it to
//! the window: Tauri commands (names exactly as in architecture.md) and
//! forwarding `download-progress` / `queue-action-pending` events to the
//! React UI. It also runs the 30 s scheduler tick and starts
//! start-on-startup queues once at launch.
//!
//! NOTE: a full desktop binary needs the Tauri system libraries
//! (GTK 3, WebKitGTK) — see app/README.md. On Windows those are provided
//! by the WebView2 runtime, which ships with Windows 10/11.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use fetchline_engine::{
    BatchResult, CategoryRow, DetectedVideo, DownloadManager, DownloadRow, DrainSummary,
    GrabberProjectRow, HistoryRow, PageLink, PendingAction, PendingLinkRow, ProbeInfo,
    ProxyConfigInput, ProxyConfigRow, QueueRow, QuotaStatus, RefreshAddressResult, ScanResult,
    Settings, SiteLoginInput, SiteLoginRow, SoundSetting, SoundSettingInput, Store, SyncResult,
    UpdateCheckResult,
};
use tauri::menu::{MenuBuilder, MenuEvent, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

/// CREATE_NO_WINDOW (Win32): keep spawned console programs invisible
/// — no console window flashing on the user's screen. Round 70: the
/// PowerShell sound player and the shutdown/rundll32/reg spawns all
/// run with this flag; without it every notification sound (and
/// every failed download, which plays the error sound twice — once
/// from the engine, once from the error dialog) flashed a black
/// console window.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Shared state: one manager for the whole app lifetime.
pub struct AppState {
    pub manager: Arc<DownloadManager>,
    /// Webview heartbeat state (§7.4, Round 56): one entry per
    /// window label (`main`, `progress-<download id>`), stamped
    /// by the `webview_heartbeat` command every 5 s from JS and
    /// read by the monitor thread in `setup`. A window whose
    /// pings stop for 30 s is logged as lost — the renderer may
    /// be dead even though the window still exists.
    pub webview_heartbeats: Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
    /// Per-download completion options (Round 62): set from the
    /// progress window's Options tab ("Close program after
    /// download", "Shut down computer after download"). In
    /// memory only — they apply to the current run of the
    /// download; an app restart forgets them, which is the
    /// honest scope (the options are chosen while the download
    /// runs, and they fire when it completes).
    pub download_completion: Arc<StdMutex<HashMap<String, DownloadCompletionOptions>>>,
}

/// Per-download completion options (Round 62) — see
/// `AppState::download_completion`.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct DownloadCompletionOptions {
    pub close_after: bool,
    pub shutdown_after: bool,
}

/// One window's JS heartbeat state (§7.4, Round 56).
pub struct WebviewHeartbeat {
    /// Epoch millis of the last ping received.
    pub last_seen_ms: u64,
    /// Whether "heartbeat lost" is already logged for the current
    /// outage (so a dead window logs once, not every 5 s).
    pub lost_logged: bool,
    /// Whether the first "heartbeat started" line is logged.
    pub started_logged: bool,
    /// Epoch millis of the last automatic reload (§9.1b, Round
    /// 57): at most one auto-reload per 5 min per window, so a
    /// wedged runtime cannot turn a recovery into a reload loop.
    /// 0 = never auto-reloaded.
    pub last_auto_reload_ms: u64,
    /// Whether the window's creation completed (§9.2, Round 58):
    /// set true only after `build()` returns in
    /// `build_progress_window`. A window whose creation never
    /// finished (main thread wedged in `wait_with_pump`) must
    /// never be auto-reloaded — rebuilding it would re-enter the
    /// same hang. Reset to false before every build; the main
    /// window's entry is set true in `setup` (it already exists
    /// by then).
    pub created_ok: bool,
    /// Auto-reload attempts for the current outage (§9.2, Round
    /// 58): capped at `AUTO_RELOAD_MAX_ATTEMPTS`, then the window
    /// is abandoned. Reset to 0 when the window's pings resume
    /// (the outage ends).
    pub auto_reload_attempts: u32,
}

/// Maximum auto-reload attempts per window per outage (§9.2,
/// Round 58): a rebuild loop against a wedged main thread only
/// makes things worse.
const AUTO_RELOAD_MAX_ATTEMPTS: u32 = 2;

impl WebviewHeartbeat {
    fn new(now_ms: u64) -> Self {
        Self {
            last_seen_ms: now_ms,
            lost_logged: false,
            started_logged: false,
            last_auto_reload_ms: 0,
            created_ok: false,
            auto_reload_attempts: 0,
        }
    }
}

/// Epoch milliseconds — the heartbeat monitor's clock.
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Startup diagnostics (BUG-029). The first Windows build launches to a
// blank white window and exits after ~1-2 s with no error dialog and no
// Windows Error Reporting entry, so the launch failure is invisible.
// Everything below writes to `startup.log` next to the database
// (`%APPDATA%\com.fetchline.app\fetchline\startup.log` on Windows).
// Logging is best-effort only: it never panics, never returns an error,
// and never changes what the app does — if logging itself fails, the app
// simply carries on without a log line.
// ---------------------------------------------------------------------------

/// Log path usable once an AppHandle exists.
fn startup_log_path_from_app(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("fetchline").join("startup.log"))
}

/// Log path usable before any AppHandle exists (the panic hook, and the
/// first lines of `run()`): on Windows `%APPDATA%` holds the same
/// `%APPDATA%\com.fetchline.app\fetchline\startup.log`; anywhere else the
/// temp dir is the fallback so the hook never has nowhere to write.
fn startup_log_path_early() -> std::path::PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return std::path::PathBuf::from(appdata)
                .join("com.fetchline.app")
                .join("fetchline")
                .join("startup.log");
        }
    }
    std::env::temp_dir().join("fetchline-startup.log")
}

/// Append one `[epoch-seconds] message` line to the log file. Creates
/// the parent directory if needed. Infallible by design.
fn startup_log_line(path: &std::path::Path, msg: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{secs}] {msg}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

/// Append one line when a log path is known; a `None` path logs nothing.
fn log_startup(path: Option<&std::path::Path>, msg: &str) {
    if let Some(p) = path {
        startup_log_line(p, msg);
    }
}

fn db_path(app: &AppHandle) -> Result<std::path::PathBuf, tauri::Error> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("fetchline").join("app.db"))
}

/// Build the manager. The database file lives in the OS app-data folder
/// (`%APPDATA%/fetchline/app.db` on Windows).
fn build_manager(app: &AppHandle) -> Result<Arc<DownloadManager>, String> {
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    let path = match db_path(app) {
        Ok(p) => p,
        Err(e) => {
            log_startup(log, &format!("db_path failed: {e}"));
            return Err(e.to_string());
        }
    };
    log_startup(log, &format!("db_path ok: {}", path.display()));
    let store = match Store::open(&path) {
        Ok(s) => s,
        Err(e) => {
            log_startup(log, &format!("store open failed: {e}"));
            return Err(e.to_string());
        }
    };
    log_startup(log, "store opened");
    // Round 56 (§7.1): point the store's diagnostics sink at the
    // same startup.log — the engine's probe/checkpoint/lock log
    // lines land in the file the live tests already read. The
    // log sits next to the database (same folder the shell's
    // `startup_log_path` resolves to); if that cannot be resolved
    // the engine stays silent instead of failing the launch.
    if let Some(log_path) = startup_log_path_from_app(app) {
        store.set_log_path(log_path);
    }
    // Round 55 (defects 1 + 4): rows left `downloading` by a
    // previous run are zombies — no run exists for them in this
    // process, so they could never finish and Resume would refuse
    // them as "already running". Reset them to `paused` (bytes
    // kept) so they can be resumed instead of sitting at 0%
    // forever.
    // Round 56 (§7.1a): the Ok(0) case used to log NOTHING, so a
    // live test could never tell whether reconciliation ran at
    // all — the r55 log's silence was read as "the diagnostics
    // never reached the log". Every outcome is logged now.
    let store = Arc::new(store);
    match store.reconcile_stuck_downloads() {
        Ok(0) => log_startup(log, "reconciled 0 download(s) stuck in 'downloading'"),
        Ok(n) => log_startup(
            log,
            &format!("reconciled {n} download(s) stuck in 'downloading' to 'paused'"),
        ),
        Err(e) => log_startup(log, &format!("reconcile stuck downloads failed: {e}")),
    }
    // 8 connections is the schema default; the stored settings are
    // applied in `setup` (apply_settings), which also wires the
    // saved proxy, speed limit and retry count.
    Ok(DownloadManager::new(store, 8))
}

#[tauri::command]
async fn add_download(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    filename: Option<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
) -> Result<(String, ProbeInfo), String> {
    // Command-entry trace (§7.1e): every call logs entry + exit.
    let n = ADD_DOWNLOAD_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let started = std::time::Instant::now();
    let log = startup_log_path_from_app(&app);
    log_startup(log.as_deref(), &format!("cmd add_download #{n} entry"));
    let result = state
        .manager
        .add_download(&url, filename, folder, category, queue)
        .await
        .map_err(|e| e.to_string());
    trace_command_exit(log.as_deref(), "add_download", n, started, result.is_ok());
    result
}

#[tauri::command]
async fn start_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .start_download(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn pause_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .pause_download(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resume_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .resume_download(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Round 72 (C) — Redownload in place: reset the row's segments
/// and start it over from scratch without deleting the row first.
/// A probe failure leaves the row Failed with the new error (the
/// old delete-first flow lost the row entirely on a flaky probe).
#[tauri::command]
async fn redownload_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .redownload_download(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Refresh the address of a paused or failed download (Round 51):
/// the engine re-probes the new link first and only switches when
/// it serves the same file — or when the user confirms a different
/// file, in which case the download restarts from zero. A probe
/// failure is an error and keeps the old address and progress.
#[tauri::command]
async fn refresh_download_address(
    state: State<'_, AppState>,
    id: String,
    url: String,
    force: bool,
) -> Result<RefreshAddressResult, String> {
    state
        .manager
        .refresh_download_address(&id, &url, force)
        .await
        .map_err(|e| e.to_string())
}

/// Clipboard monitoring (Round 75): the app itself just wrote to
/// the clipboard (the update dialog copies the installer URL) —
/// record the current clipboard sequence number as seen, so the
/// monitor does not offer our own link back to the user. The
/// sequence is read here, on the shell side, because only the
/// shell calls Win32. Off Windows this is a no-op.
#[tauri::command]
fn note_own_clipboard_write(state: State<'_, AppState>) {
    if let Some(seq) = clipboard_sequence_number() {
        state.manager.note_clipboard_sequence(seq);
    }
}

/// Round 75 — the clipboard change signal: the OS sequence number,
/// which increments on every clipboard write. Windows only; `None`
/// elsewhere (the engine then evaluates the text on every poll —
/// the app only ships on Windows).
#[cfg(windows)]
fn clipboard_sequence_number() -> Option<u32> {
    Some(unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() })
}
#[cfg(not(windows))]
fn clipboard_sequence_number() -> Option<u32> {
    None
}

#[tauri::command]
async fn cancel_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .cancel_download(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Skip the backoff of a download waiting for a retry (S03,
/// "Retry now"). The engine refuses when nothing is waiting.
#[tauri::command]
async fn retry_now(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .retry_now(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Open the folder of a finished download in the system file
/// manager (S03). The engine validates the folder still exists
/// first, so a deleted folder is a clear error, not a silent no-op.
#[tauri::command]
async fn open_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.open_folder(&id).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Download progress window (S03, 2026-11-15). One separate OS window
// per download, titled "<percent>% <filename>" — the IDM-style
// dialog, with Fetchline's own look. The window is created here, in
// Rust, not from the frontend: the frontend has no capability grant
// for the webview/window plugin commands, and creating it here also
// sets the title and size at creation. The window's own title is
// likewise updated from Rust, in the progress-event forwarder below.
// (BUG-044, fixed in Round 40: the frontend event listener never
// fired because this app shipped no Tauri capability files;
// `capabilities/default.json` now grants
// `core:event:allow-listen/unlisten` to the main window, so the
// main window's listeners work — the progress window keeps polling
// `get_progress_snapshot` by design, and the dead
// `download-progress` listener stays removed.) Closing the window
// never cancels the download: the engine keeps running and the
// window is only a view of it; double-clicking the row reopens it.
// ---------------------------------------------------------------------------

/// Window label for one download's progress window.
fn progress_window_label(download_id: &str) -> String {
    format!("progress-{download_id}")
}

/// Title of a progress window: "<percent>% <filename>" while the
/// size is known, the filename alone when it is not.
fn progress_window_title(downloaded_bytes: u64, size_bytes: Option<u64>, filename: &str) -> String {
    match size_bytes {
        Some(size) if size > 0 => {
            let percent = downloaded_bytes
                .saturating_mul(100)
                .saturating_div(size)
                .min(100);
            format!("{percent}% {filename}")
        }
        _ => filename.to_string(),
    }
}

/// Minimum gap between two automatic webview reloads of the same
/// window (§9.1b, Round 57): 5 min. A wedged runtime must never
/// turn a recovery into a reload loop. The banner's manual Reload
/// is NOT gated by this — the user asked for it explicitly.
const AUTO_RELOAD_COOLDOWN_MS: u64 = 5 * 60 * 1000;

/// Reload a window's webview right now (§9.1a, Round 57) — the
/// recovery primitive the banner, the heartbeat monitor and the
/// `ProcessFailed` handler all share. Main window: the NATIVE
/// `WebviewWindow::reload()` (Tauri 2.12.5 → wry
/// `ICoreWebView2::Reload()`), which needs neither live JS nor a
/// live IPC channel — the old `eval("window.location.reload()")`
/// needed exactly the two things that are dead when a reload is
/// needed, so it could never fix a dead renderer. A progress
/// window is closed and rebuilt from scratch, which also replaces
/// a dead renderer process with a fresh one. No cooldown here:
/// this is the explicit (or already-gated) reload. Errors are
/// the caller's to report.
fn reload_webview_now(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    label: &str,
    heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
) -> Result<(), String> {
    // Round 74 (1) — the close-and-rebuild is atomic with every
    // other window creation (see `WINDOW_BUILD_MUTEX`): an open
    // racing this rebuild must focus the old window or the new
    // one, never build a second.
    let _guard = lock_window_build();
    if label == "main" {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| "No main window open".to_string())?;
        return window.reload().map_err(|e| e.to_string());
    }
    let Some(download_id) = label.strip_prefix("progress-") else {
        return Err(format!("Unknown window label: {label}"));
    };
    let Some(window) = app.get_webview_window(label) else {
        return Err(format!(
            "No progress window open for download {download_id}"
        ));
    };
    window.close().map_err(|e| e.to_string())?;
    // Round 74 (1) — `close()` only DISPATCHES the close; the
    // window leaves Tauri's manager (and frees its label) when
    // the runtime processes it. Building at once can hit
    // `WindowLabelAlreadyExists` — or attach over the closing
    // window. Wait (bounded) for the label to free first.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while app.get_webview_window(label).is_some() {
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "progress window {label} did not close in time for reload"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let log = startup_log_path_from_app(app);
    log_startup(
        log.as_deref(),
        &format!("progress window {label} closed for reload"),
    );
    build_progress_window(app, manager, download_id, heartbeats)?;
    Ok(())
}

/// Maybe auto-reload a window whose renderer is dead (§9.1b,
/// Round 57; guarded §9.2, Round 58): the heartbeat monitor and
/// the `ProcessFailed` handler call this, never the banner. At most
/// one auto-reload per 5 min per window (`AUTO_RELOAD_COOLDOWN_MS`)
/// and at most `AUTO_RELOAD_MAX_ATTEMPTS` per outage, so a wedged
/// runtime cannot cause a reload loop. A window whose creation
/// never completed is never rebuilt — rebuilding it would re-enter
/// the same main-thread hang that killed the first creation. The
/// rebuild itself runs on a detached `std::thread`, not the
/// monitor thread: a hung rebuild must not silence heartbeat
/// monitoring. Returns `Ok(true)` when a rebuild was dispatched.
/// Every decision is logged — a skipped reload is as diagnostic
/// as a run one. Never restarts the process (auto-restart is a
/// pending user decision, not implemented here).
fn maybe_auto_reload(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    label: &str,
    heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
) -> Result<bool, String> {
    let now = epoch_ms();
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    // Round 71 (C2) — a closed window's heartbeat entry must not
    // keep triggering auto-reloads: the monitor only ever sees a
    // frozen last_seen_ms, flags "lost", and reload_webview_now
    // fails with "No progress window open" — the dozens of FAILED
    // lines in startup.log. Drop the entry silently when the
    // window is gone (a window whose creation never completed has
    // no window either, so this also covers the created_ok guard
    // below). The main window is never closed (its X hides it to
    // the tray), so it is unaffected.
    if app.get_webview_window(label).is_none() {
        if let Ok(mut map) = heartbeats.lock() {
            map.remove(label);
        }
        return Ok(false);
    }
    {
        let Ok(mut map) = heartbeats.lock() else {
            return Ok(false);
        };
        let hb = map
            .entry(label.to_string())
            .or_insert_with(|| WebviewHeartbeat::new(now));
        // Round 58 (§9.2): a window whose creation never
        // completed must not be rebuilt — the rebuild would block
        // in the same unbounded `wait_with_pump` that wedged the
        // first creation.
        if !hb.created_ok {
            log_startup(
                log,
                &format!("webview {label} auto-reload skipped (window creation never completed)"),
            );
            return Ok(false);
        }
        if hb.auto_reload_attempts >= AUTO_RELOAD_MAX_ATTEMPTS {
            log_startup(
                log,
                &format!(
                    "webview {label} auto-reload abandoned ({AUTO_RELOAD_MAX_ATTEMPTS} attempts)"
                ),
            );
            return Ok(false);
        }
        if now.saturating_sub(hb.last_auto_reload_ms) < AUTO_RELOAD_COOLDOWN_MS {
            log_startup(
                log,
                &format!("webview {label} auto-reload skipped (cooldown)"),
            );
            return Ok(false);
        }
        hb.last_auto_reload_ms = now;
        hb.auto_reload_attempts += 1;
        // The lock is released before the reload runs: the reload
        // itself registers a new heartbeat entry (progress windows).
    }
    log_startup(
        log,
        &format!("webview {label} auto-reloading (renderer dead)"),
    );
    // Round 58 (§9.2): the rebuild runs on its own thread. The
    // monitor thread (or the `ProcessFailed` handler) must keep
    // running while a rebuild is stuck in window creation — the
    // "done"/"FAILED" lines are written by the rebuild thread.
    let app_thread = app.clone();
    let manager_thread = Arc::clone(manager);
    let label_thread = label.to_string();
    let heartbeats_thread = Arc::clone(heartbeats);
    std::thread::spawn(move || {
        match reload_webview_now(
            &app_thread,
            &manager_thread,
            &label_thread,
            &heartbeats_thread,
        ) {
            Ok(()) => {
                let log = startup_log_path_from_app(&app_thread);
                log_startup(
                    log.as_deref(),
                    &format!("webview {label_thread} auto-reload done"),
                );
            }
            Err(e) => {
                let log = startup_log_path_from_app(&app_thread);
                log_startup(
                    log.as_deref(),
                    &format!("webview {label_thread} auto-reload FAILED: {e}"),
                );
            }
        }
    });
    Ok(true)
}

/// Register WebView2's `ProcessFailed` event on a window's webview
/// (§9.1c, Round 57). wry 0.57 surfaces no renderer-failure event
/// at all — a dead renderer is invisible to the app, which is how
/// both windows sat blank white for half an hour in the r55 test
/// with nothing in the log. The registration goes through Tauri's
/// `Webview::with_webview` (it hands the closure the platform
/// webview, whose `controller()` returns the
/// `ICoreWebView2Controller`; the raw `ICoreWebView2` comes from
/// it via the `CoreWebView2` propget — Tauri's `PlatformWebview`
/// exposes no `webview()` method) and the `webview2-com 0.39` crate
/// (`ProcessFailedEventHandler::create` +
/// `ICoreWebView2::add_ProcessFailed`), mirroring wry's own
/// `PermissionRequested` registration in wry 0.57.2
/// (webview2/mod.rs). On the event: log the failure kind and
/// trigger the same native reload the heartbeat monitor uses
/// (§9.1b, cooldown-gated — the unresponsive kind fires every
/// few seconds while it lasts). The handler runs on a WebView2
/// thread, so the recovery is best-effort and never panics. A
/// `BrowserProcessExited` kind means the whole WebView is closed
/// (Microsoft: the app must recreate it) — the log line names
/// it, and the progress-window rebuild path does recreate it;
/// the main window has no equivalent in-app recovery.
/// Windows only; other platforms get the no-op below.
#[cfg(target_os = "windows")]
fn register_process_failed(
    app: &AppHandle,
    label: &str,
    heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
    manager: &Arc<DownloadManager>,
) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let label_owned = label.to_string();
    let app_owned = app.clone();
    let heartbeats_owned = Arc::clone(heartbeats);
    let manager_owned = Arc::clone(manager);
    // `with_webview` runs the closure on the main thread and hands
    // it the platform webview. Best-effort throughout: a failed
    // registration only means the heartbeat monitor is the sole
    // detector — a window creation must never fail over it.
    let _ = window.with_webview(move |platform_webview| {
        let controller = platform_webview.controller();
        let core = match unsafe { controller.CoreWebView2() } {
            Ok(core) => core,
            Err(_) => return,
        };
        let handler =
            webview2_com::ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
                use webview2_com::Microsoft::Web::WebView2::Win32::{
                    COREWEBVIEW2_PROCESS_FAILED_KIND,
                    COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
                    COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
                    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
                    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
                };
                let kind = args.as_ref().and_then(|a| {
                    let mut k = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                    unsafe { a.ProcessFailedKind(&mut k) }.ok().map(|_| k)
                });
                let kind_str = match kind {
                    Some(k) if k == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
                        "browser process exited"
                    }
                    Some(k) if k == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => {
                        "render process exited"
                    }
                    Some(k)
                        if k == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE =>
                    {
                        "render process unresponsive"
                    }
                    Some(k)
                        if k == COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED =>
                    {
                        "frame render process exited"
                    }
                    _ => "unknown",
                };
                if let Some(log_path) = startup_log_path_from_app(&app_owned) {
                    startup_log_line(
                        &log_path,
                        &format!("webview {label_owned} renderer process failed ({kind_str})"),
                    );
                }
                let _ =
                    maybe_auto_reload(&app_owned, &manager_owned, &label_owned, &heartbeats_owned);
                Ok(())
            }));
        let mut token: i64 = 0;
        unsafe {
            let _ = core.add_ProcessFailed(&handler, &mut token);
        }
    });
}

/// No-op on non-Windows platforms: their webviews never had this
/// failure mode in the live tests, and the crates above are
/// Windows-only dependencies.
#[cfg(not(target_os = "windows"))]
fn register_process_failed(
    _app: &AppHandle,
    _label: &str,
    _heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
    _manager: &Arc<DownloadManager>,
) {
}

/// A WebView2 environment that may be moved between threads.
/// `ICoreWebView2Environment` is `!Send` (it wraps a raw COM
/// pointer), so it cannot cross the `with_webview` closure's
/// `Send` bound — or a channel — on its own. This wrapper exists
/// only to carry the pointer from the main thread (where
/// `with_webview` runs) back to the calling worker, which stores
/// it in the window builder without ever calling a method on it.
///
/// # Safety
///
/// The wrapped value is only ever *moved* between threads, never
/// *used* off the main thread: the worker holds it and passes it
/// to `WebviewWindowBuilder::with_environment`, and the only COM
/// method call on it — wry's `CreateCoreWebView2Controller` —
/// runs on the main thread, the thread that created it. Cloning
/// or dropping the pointer performs only COM `AddRef`/`Release`,
/// which COM requires to be thread-safe. This is the same transfer
/// Tauri itself already performs: `WebviewAttributes` (which holds
/// this exact type) is `unsafe impl Send` in tauri-runtime
/// (`tauri-runtime-2.12.2/src/webview.rs:434`), and that is how
/// the environment travels from the worker to the main thread in
/// the `builder.with_environment` call below. Microsoft's WebView2
/// threading doc does NOT state the environment is agile, so no
/// agility is claimed here — the guarantee is the narrower
/// move-only use above. Windows only.
#[cfg(target_os = "windows")]
struct SendableWebView2Environment(
    webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment,
);

// SAFETY: see the struct docs — move-only, never called off the
// main thread; AddRef/Release are thread-safe by COM rules.
#[cfg(target_os = "windows")]
unsafe impl Send for SendableWebView2Environment {}

/// Round 58 (§9.1): the main window's WebView2 environment, so a
/// progress window can share it instead of creating a fresh
/// environment on the main thread. A fresh environment creation
/// waits unbounded in wry's `wait_with_pump` — the Round 58
/// main-thread wedge — so a progress window must not pay for one.
/// The wait for the `with_webview` closure is bounded at 5 s: a
/// wedged main thread only costs a fresh environment, never a hung
/// worker. `None` means "fall back to a fresh environment" and is
/// logged by the caller. Windows only.
#[cfg(target_os = "windows")]
fn main_webview_environment(
    app: &AppHandle,
    log: Option<&std::path::Path>,
) -> Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment> {
    let main = app.get_webview_window("main")?;
    let (tx, rx) = std::sync::mpsc::channel::<SendableWebView2Environment>();
    // `with_webview` runs the closure on the main thread.
    let _ = main.with_webview(move |platform_webview| {
        let _ = tx.send(SendableWebView2Environment(platform_webview.environment()));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(env) => Some(env.0),
        Err(_) => {
            log_startup(
                log,
                "progress window: main WebView2 environment unavailable — creating a fresh environment",
            );
            None
        }
    }
}

/// Build one progress window for a download. The window loads the
/// same frontend bundle, so `main.tsx` renders the progress view
/// instead of the main window. Shared by `open_progress_window`
/// and by `reload_webview_window` (which closes a dead window and
/// builds a fresh one — §7.4, Round 56).
/// Round 56 (§7.1f): the log line now goes out BEFORE `build()`
/// (the r55 log had no creation line at all, so a live test could
/// not tell whether the call was even reached), and the builder's
/// URL is logged after creation. Round 58 (§9.3): the window's
/// resolved URL is no longer read back — the `url()` getter blocks
/// a worker with no timeout, and the URL is an input, not a
/// discovery.
/// Round 57 (§9.3): A/B test — the window used to load
/// `index.html#progress=<id>`, and the r55/r56 live tests both
/// showed a blank white window with a correct title. The fragment
/// is one suspect (a navigation the webview never finishes), so
/// this build loads the PLAIN `index.html` and passes the id
/// through an initialization script instead:
/// `window.__PROGRESS_ID__` is set before any document script
/// runs, and `main.tsx` reads it (falling back to the hash). The
/// fragment path stays working as the fallback, so an old window
/// or a missed script still renders the progress view.
/// Round 74 (1) — serialize ALL window creation (progress /
/// complete / error / add-url builders, and the reload rebuild
/// below): `open_progress_window` used to check
/// `get_webview_window` and then `build()` with nothing between
/// them, so two concurrent invokes (the auto-open effect + a
/// double-click, or two polls) both passed the check and both
/// built. Tauri's own duplicate-label check runs in
/// `prepare_window`, BEFORE the window is attached to the
/// manager — two builds that pass it together both create an OS
/// window, and the second `attach_window` silently overwrites
/// the first in the manager map. The first window stays on
/// screen as an orphan: same title, same download, unreachable
/// by focus/close/title updates — the two stacked "1%" windows
/// cancelling each other. Holding this mutex across the
/// check-and-build (and across close-and-rebuild) makes the
/// sequence atomic per process. Builds are rare (user actions),
/// so one global lock is enough; it is never held across an
/// `.await`.
static WINDOW_BUILD_MUTEX: StdMutex<()> = StdMutex::new(());

/// Lock the window-build mutex, recovering from poisoning (a
/// panicking build must not wedge every later window creation).
fn lock_window_build() -> std::sync::MutexGuard<'static, ()> {
    WINDOW_BUILD_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
}

/// Round 71 (C3) — show-when-ready FALLBACK: a window built
/// hidden must never stay hidden if the frontend's ready signal
/// never arrives (JS error, slow bundle). Show it after 3 s
/// unconditionally; `show()` on an already-visible window is a
/// no-op. Detached thread, so it can never block the caller.
fn spawn_show_fallback(window: &tauri::WebviewWindow) {
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        show_window_ready(&window);
    });
}

/// Round 74 (13) — show a window, and when it is one of the
/// independent dialogs (capture Add URL, complete, error), pop
/// it IN FRONT over whatever window the user has open — even
/// when the main window is hidden/minimized to tray: show +
/// focus + always-on-top for ~1 s, then clear the topmost flag
/// so it behaves like a normal window afterwards. The
/// always-on-top is the part that works even when Windows
/// refuses the foreground steal from a background process
/// (focus alone is best-effort there); the 1 s clear keeps the
/// dialog from staying on top forever. Main and progress
/// windows just show — they never take the topmost flag.
fn show_window_ready(window: &tauri::WebviewWindow) {
    let _ = window.show();
    let label = window.label();
    if label.starts_with("add-url-")
        || label.starts_with("complete-")
        || label.starts_with("error-")
    {
        let _ = window.set_focus();
        let _ = window.set_always_on_top(true);
        let w = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            let _ = w.set_always_on_top(false);
        });
    }
}

/// Round 71 (C3) — show-when-ready: windows are built hidden
/// (`visible: false`) so they never flash a blank first paint;
/// the frontend calls this from a `useLayoutEffect` after its
/// root component mounts, and the window appears WITH content.
/// Tauri injects the calling window, so one command serves the
/// main, progress, error and completion windows alike. Round 74
/// (13): the dialogs pop in front here too (see
/// `show_window_ready`), so a capture Add URL window appears
/// over the user's browser even when the main window is hidden
/// to the tray.
#[tauri::command]
async fn window_ready(window: tauri::WebviewWindow) -> Result<(), String> {
    show_window_ready(&window);
    Ok(())
}

/// Round 87 (1) — one line from the frontend into startup.log.
/// A frontend failure that only `console.error`s is invisible
/// on the VPS (the retest reads startup.log, not DevTools), and
/// the R86 Add URL drag failure stayed undiagnosed exactly
/// because its `.catch(() => undefined)` swallowed it. Callers
/// keep `console.error` too — this is the durable copy. The
/// message is capped so a runaway caller cannot flood the log.
#[tauri::command]
async fn log_frontend(app: AppHandle, message: String) -> Result<(), String> {
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let capped: String = message.chars().take(500).collect();
    log_startup(log, &format!("frontend: {capped}"));
    Ok(())
}

fn build_progress_window(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    download_id: &str,
    heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
) -> Result<WebviewWindow, String> {
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    let row = manager
        .get_download(download_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {download_id}"))?;
    let label = progress_window_label(download_id);
    // Round 74 (1) — last-line defence: if a window with this
    // label appeared after the caller's check (a caller that
    // forgot the build mutex), focus it instead of building a
    // second one. Callers hold the mutex, so this is normally a
    // no-op.
    if let Some(existing) = app.get_webview_window(&label) {
        log_startup(
            log,
            &format!("progress window {label} already open — focused (build re-check)"),
        );
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(existing);
    }
    let url = WebviewUrl::App("index.html".into());
    let init_script = format!("window.__PROGRESS_ID__ = {download_id:?};");
    log_startup(
        log,
        &format!("creating progress window {label}: index.html (id via init script)"),
    );
    // Round 58 (§9.2): mark creation as not-yet-completed BEFORE
    // `build()` — a `build()` that never returns must leave the
    // flag false, so the monitor never auto-reloads a window whose
    // creation never finished.
    if let Ok(mut map) = heartbeats.lock() {
        let hb = map
            .entry(label.clone())
            .or_insert_with(|| WebviewHeartbeat::new(epoch_ms()));
        hb.created_ok = false;
    }
    // Round 58 (§9.1): share the main window's WebView2
    // environment instead of creating a fresh one per window.
    // A fresh environment creation waits unbounded on the main
    // thread (`wait_with_pump`) — the Round 58 wedge. Windows
    // only; other platforms build exactly as before.
    #[cfg(target_os = "windows")]
    let shared_environment = main_webview_environment(app, log);
    #[cfg(target_os = "windows")]
    if shared_environment.is_some() {
        log_startup(
            log,
            &format!("progress window {label}: sharing main WebView2 environment"),
        );
    }
    // Round 58 (§9.5b): time the `build()` — a build taking >5 s
    // is diagnostic of a slow or wedged window creation.
    let build_started = std::time::Instant::now();
    let mut builder = WebviewWindowBuilder::new(app, &label, url)
        .initialization_script(init_script)
        .title(progress_window_title(
            row.downloaded_bytes as u64,
            row.size_bytes.map(|s| s as u64),
            &row.filename,
        ))
        // Round 62: the redesigned window is frameless (the
        // frontend draws its own title bar, same as the main
        // window) and square like IDM — landscape, wider than
        // tall, ~660x480. Resize-from-edges and rounded corners
        // are verified on the VPS, not here (no Windows on this
        // VM); decorations(false) + a draggable title bar is the
        // Tauri 2 way to get them.
        .decorations(false)
        .inner_size(660.0, 480.0)
        .min_inner_size(560.0, 380.0)
        .resizable(true)
        .center()
        .devtools(true)
        // Round 71 (C1/C3) — the app's own background (#0B0F24)
        // so a first paint is never white, and hidden until the
        // frontend signals ready (show-when-ready; the 3 s
        // fallback in spawn_show_fallback guarantees it appears).
        .background_color(tauri::window::Color(11, 15, 36, 255))
        .visible(false);
    #[cfg(target_os = "windows")]
    if let Some(env) = shared_environment {
        builder = builder.with_environment(env);
    }
    // Round 60 (§11.4 item 2): prove `build()` was entered and on
    // which thread — discriminates (e1) stuck-before-`build()` from
    // (b) stuck-inside-`build()`.
    let build_thread_id = std::thread::current().id();
    log_startup(
        log,
        &format!("progress window {label}: build() entered on thread {build_thread_id:?}"),
    );
    match builder.build() {
        Ok(window) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            // Round 58 (§9.3): the window's URL is no longer read
            // back with the `url()` getter — that getter blocks a
            // worker with no timeout, and the URL is an input, not
            // a discovery: the builder loaded `index.html`.
            log_startup(
                log,
                &format!(
                    "progress window {label} created: index.html in {elapsed_ms}ms on thread {build_thread_id:?}"
                ),
            );
            if elapsed_ms > 5_000 {
                log_startup(
                    log,
                    &format!(
                        "progress window {label} build took {}ms — slow creation",
                        elapsed_ms
                    ),
                );
            }
            // Register the heartbeat entry at creation (§9.1b): a
            // window whose JS never runs at all (renderer dead
            // from the start) would otherwise never ping, so the
            // monitor would have nothing to watch and never log
            // "lost". The first ping updates the stamp; the entry
            // here only starts the clock. Mark creation complete
            // (§9.2): the monitor may auto-reload this window.
            if let Ok(mut map) = heartbeats.lock() {
                let hb = map
                    .entry(label.clone())
                    .or_insert_with(|| WebviewHeartbeat::new(epoch_ms()));
                hb.created_ok = true;
            }
            register_process_failed(app, &label, heartbeats, manager);
            // Round 71 (C3) — the window was built hidden; the
            // frontend shows it when ready, and this fallback
            // guarantees it appears even if that signal never
            // arrives.
            spawn_show_fallback(&window);
            Ok(window)
        }
        Err(e) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "progress window {label} FAILED to create after {elapsed_ms}ms on thread {build_thread_id:?}: {e}"
                ),
            );
            Err(e.to_string())
        }
    }
}

/// Open (or focus, when it already exists) the separate progress
/// window for a download. Errors when the download does not exist.
/// Round 74 (1): the check-and-build holds the window-build mutex
/// — two concurrent opens (auto-open + double-click) used to race
/// past the already-open check and build two windows for one
/// download (the second overwrote the first in Tauri's manager,
/// orphaning it on screen).
#[tauri::command]
async fn open_progress_window(
    app: AppHandle,
    state: State<'_, AppState>,
    download_id: String,
) -> Result<(), String> {
    // Round 55 (defect 2, diagnostics): the window opened with the
    // right title but rendered blank white, and the code path looks
    // correct — log the creation (and any failure) to the startup
    // log, so the next live test can say whether the window was
    // even created. Devtools are enabled on this window only, so
    // its console errors can be captured in the test build.
    let _guard = lock_window_build();
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let label = progress_window_label(&download_id);
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second copy.
        // `show()` first: a window hidden to the tray (Round 65)
        // does not come back on `set_focus()` alone.
        let _ = window.show();
        window.set_focus().map_err(|e| e.to_string())?;
        log_startup(
            log,
            &format!("progress window {label} already open — focused"),
        );
        return Ok(());
    }
    build_progress_window(
        &app,
        &state.manager,
        &download_id,
        &state.webview_heartbeats,
    )?;
    Ok(())
}

/// Label of a download's "Download complete" dialog window
/// (Round 66): `complete-<download id>`, mirroring
/// `progress-<download id>`.
fn complete_window_label(download_id: &str) -> String {
    format!("complete-{download_id}")
}

/// Build one "Download complete" dialog window (Round 66): a
/// separate OS window like the progress window — same bundle,
/// frameless with the shared `TitleBar.tsx`, the real Sonic logo
/// in the title bar (the window icon) — but a dialog, not a
/// progress view: fixed size (~560x440, the approved mockup),
/// not resizable, and with no heartbeat registration and no
/// auto-reload (it is transient — a dead renderer there just
/// leaves a window the user closes; `main.tsx` sends no
/// heartbeat for it, so the monitor never sees it). The download
/// id arrives through an initialization script
/// (`window.__COMPLETE_ID__`), the same Round 57 mechanism as
/// the progress window, with the `#complete=<id>` hash as the
/// fallback.
fn build_complete_dialog_window(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    download_id: &str,
) -> Result<WebviewWindow, String> {
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    let _row = manager
        .get_download(download_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {download_id}"))?;
    let label = complete_window_label(download_id);
    // Round 74 (1) — build re-check (see `build_progress_window`):
    // focus the existing dialog instead of a second one.
    if let Some(existing) = app.get_webview_window(&label) {
        log_startup(
            log,
            &format!("complete dialog {label} already open — focused (build re-check)"),
        );
        show_window_ready(&existing);
        return Ok(existing);
    }
    let url = WebviewUrl::App("index.html".into());
    let init_script = format!("window.__COMPLETE_ID__ = {download_id:?};");
    log_startup(
        log,
        &format!("creating complete dialog {label}: index.html (id via init script)"),
    );
    // Round 58 (§9.1): share the main window's WebView2
    // environment instead of creating a fresh one per window —
    // a fresh environment creation waits unbounded on the main
    // thread (`wait_with_pump`), the Round 58 wedge. Windows
    // only; other platforms build exactly as before.
    #[cfg(target_os = "windows")]
    let shared_environment = main_webview_environment(app, log);
    #[cfg(target_os = "windows")]
    if shared_environment.is_some() {
        log_startup(
            log,
            &format!("complete dialog {label}: sharing main WebView2 environment"),
        );
    }
    let build_started = std::time::Instant::now();
    let mut builder = WebviewWindowBuilder::new(app, &label, url)
        .initialization_script(init_script)
        .title("Download complete")
        .decorations(false)
        .inner_size(560.0, 440.0)
        .min_inner_size(520.0, 400.0)
        .resizable(false)
        .center()
        .devtools(true)
        // Round 71 (C1/C3) — #0B0F24 first paint + hidden until
        // the frontend signals ready (see the progress window).
        .background_color(tauri::window::Color(11, 15, 36, 255))
        .visible(false);
    #[cfg(target_os = "windows")]
    if let Some(env) = shared_environment {
        builder = builder.with_environment(env);
    }
    let build_thread_id = std::thread::current().id();
    log_startup(
        log,
        &format!("complete dialog {label}: build() entered on thread {build_thread_id:?}"),
    );
    match builder.build() {
        Ok(window) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "complete dialog {label} created: index.html in {elapsed_ms}ms on thread {build_thread_id:?}"
                ),
            );
            // Round 71 (C3) — built hidden; shown when the
            // frontend is ready (3 s fallback).
            spawn_show_fallback(&window);
            Ok(window)
        }
        Err(e) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "complete dialog {label} FAILED to create after {elapsed_ms}ms on thread {build_thread_id:?}: {e}"
                ),
            );
            Err(e.to_string())
        }
    }
}

/// Open (or focus, when it already exists) the "Download
/// complete" dialog for a finished download (Round 66). The
/// main window calls this when a download flips to 'complete';
/// the progress window closes itself at the same moment, so
/// the dialog replaces it. Errors when the download does not
/// exist.
#[tauri::command]
async fn open_complete_dialog(
    app: AppHandle,
    state: State<'_, AppState>,
    download_id: String,
) -> Result<(), String> {
    // Round 74 (1): the check-and-build holds the window-build
    // mutex (see `open_progress_window`).
    let _guard = lock_window_build();
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let label = complete_window_label(&download_id);
    // Round 87 (9) — the PERSISTED setting is authoritative
    // here, before any window is created: the main window's
    // in-memory settings are fetched once at mount, so a "Don't
    // show this dialog again" checked in a dialog window (a
    // different OS window writing the same Settings store) never
    // reached it, and the next completion still popped a dialog
    // in the same session. A settings read that fails falls
    // through to the old behaviour (show) — a read failure must
    // not silently eat completions.
    match state.manager.get_settings() {
        Ok(settings) => {
            if !settings.show_download_complete_dialog {
                log_startup(
                    log,
                    &format!(
                        "complete dialog {label}: skipped — 'Don't show this dialog again' is on"
                    ),
                );
                return Ok(());
            }
        }
        Err(e) => {
            log_startup(
                log,
                &format!("complete dialog {label}: settings read failed ({e}) — showing anyway"),
            );
        }
    }
    // Round 76 (3) — the close is authoritative: the progress
    // window must not survive the complete dialog. The progress
    // window closes itself on its own poll, but that races this
    // open (and never fires at all for a download that was
    // already complete when the window opened), so both windows
    // stayed open together. Close it here, before building (or
    // focusing, or queueing into) the dialog.
    let progress_label = progress_window_label(&download_id);
    if let Some(progress) = app.get_webview_window(&progress_label) {
        let _ = progress.close();
        log_startup(
            log,
            &format!("complete dialog {label}: closed progress window {progress_label}"),
        );
    }
    // Round 87 (10) — ONE complete dialog at a time, like the
    // R85 error-dialog queue. Several downloads finishing
    // together used to stack one OS window each, so one Close
    // click only dismissed the top copy. If any complete dialog
    // window already exists, queue this completion INTO it
    // instead of opening another: the dialog shows "N more
    // completed" and Close steps through them, with "Dismiss
    // all" to clear the queue. The frontend listens for
    // `complete-dialog-queued` (see DownloadCompleteDialog).
    // The check runs under the window-build mutex, so two
    // completions in the same poll cannot race past each other
    // into two windows.
    if let Some(existing) = app
        .webview_windows()
        .values()
        .find(|w| w.label().starts_with("complete-"))
    {
        let _ = existing.emit(
            "complete-dialog-queued",
            serde_json::json!({ "download_id": download_id }),
        );
        show_window_ready(existing);
        log_startup(
            log,
            &format!(
                "complete dialog {} already open — queued {download_id} into it",
                existing.label()
            ),
        );
        return Ok(());
    }
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second
        // copy (the main window and a progress window can both
        // notice the same completion). Round 74 (13): the dialog
        // pops in front over whatever the user has open.
        show_window_ready(&window);
        log_startup(
            log,
            &format!("complete dialog {label} already open — focused"),
        );
        return Ok(());
    }
    build_complete_dialog_window(&app, &state.manager, &download_id)?;
    Ok(())
}

/// Label of a download's "Download failed" dialog window
/// (Round 68): `error-<download id>`, mirroring
/// `complete-<download id>`.
fn error_window_label(download_id: &str) -> String {
    format!("error-{download_id}")
}

/// Build one "Download failed" dialog window (Round 68): a
/// separate OS window like the "Download complete" dialog — the
/// same bundle, frameless with the shared `TitleBar.tsx`, the
/// real Sonic logo in the title bar — but the title is the
/// FILENAME (like IDM's error popup, whose title bar is the
/// file's name), fixed size (~520x360, the approved mockup),
/// not resizable, and with no heartbeat registration and no
/// auto-reload (transient, like the complete dialog). The
/// download id arrives through an initialization script
/// (`window.__ERROR_ID__`), with the `#error=<id>` hash as the
/// fallback.
fn build_error_dialog_window(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    download_id: &str,
) -> Result<WebviewWindow, String> {
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    let row = manager
        .get_download(download_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {download_id}"))?;
    let label = error_window_label(download_id);
    // Round 74 (1) — build re-check (see `build_progress_window`):
    // focus the existing dialog instead of a second one.
    if let Some(existing) = app.get_webview_window(&label) {
        log_startup(
            log,
            &format!("error dialog {label} already open — focused (build re-check)"),
        );
        show_window_ready(&existing);
        return Ok(existing);
    }
    let url = WebviewUrl::App("index.html".into());
    let init_script = format!("window.__ERROR_ID__ = {download_id:?};");
    log_startup(
        log,
        &format!("creating error dialog {label}: index.html (id via init script)"),
    );
    // Round 58 (§9.1): share the main window's WebView2
    // environment instead of creating a fresh one per window —
    // a fresh environment creation waits unbounded on the main
    // thread (`wait_with_pump`), the Round 58 wedge. Windows
    // only; other platforms build exactly as before.
    #[cfg(target_os = "windows")]
    let shared_environment = main_webview_environment(app, log);
    #[cfg(target_os = "windows")]
    if shared_environment.is_some() {
        log_startup(
            log,
            &format!("error dialog {label}: sharing main WebView2 environment"),
        );
    }
    let build_started = std::time::Instant::now();
    let mut builder = WebviewWindowBuilder::new(app, &label, url)
        .initialization_script(init_script)
        .title(row.filename.clone())
        .decorations(false)
        .inner_size(520.0, 360.0)
        .min_inner_size(480.0, 320.0)
        .resizable(false)
        .center()
        .devtools(true)
        // Round 71 (C1/C3) — #0B0F24 first paint + hidden until
        // the frontend signals ready (see the progress window).
        .background_color(tauri::window::Color(11, 15, 36, 255))
        .visible(false);
    #[cfg(target_os = "windows")]
    if let Some(env) = shared_environment {
        builder = builder.with_environment(env);
    }
    let build_thread_id = std::thread::current().id();
    log_startup(
        log,
        &format!("error dialog {label}: build() entered on thread {build_thread_id:?}"),
    );
    match builder.build() {
        Ok(window) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "error dialog {label} created: index.html in {elapsed_ms}ms on thread {build_thread_id:?}"
                ),
            );
            // Round 71 (C3) — built hidden; shown when the
            // frontend is ready (3 s fallback).
            spawn_show_fallback(&window);
            Ok(window)
        }
        Err(e) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "error dialog {label} FAILED to create after {elapsed_ms}ms on thread {build_thread_id:?}: {e}"
                ),
            );
            Err(e.to_string())
        }
    }
}

/// Open (or focus, when it already exists) the "Download
/// failed" dialog for a failed download (Round 68). The main
/// window calls this when a download flips to 'error'; the
/// progress window stays open behind it (IDM shows the same
/// popup over its progress window), and pressing OK returns
/// there — Resume continues from where the download stopped.
/// Errors when the download does not exist.
#[tauri::command]
async fn open_error_dialog(
    app: AppHandle,
    state: State<'_, AppState>,
    download_id: String,
) -> Result<(), String> {
    // Round 74 (1): the check-and-build holds the window-build
    // mutex (see `open_progress_window`).
    let _guard = lock_window_build();
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let label = error_window_label(&download_id);
    // Round 85 — ONE error dialog at a time. The R84 retest
    // found OK "sometimes needs 2–3 clicks": every failed
    // download opened its OWN stacked window (one per id), so
    // one OK click only dismissed the top copy. If any error
    // dialog window already exists, queue this failure INTO it
    // instead of opening another: the dialog shows "N more
    // errors" and OK steps through them, with "Dismiss all" to
    // clear the queue. The frontend listens for
    // `error-dialog-queued` (see DownloadErrorDialog). The
    // check runs under the window-build mutex, so two failures
    // in the same poll cannot race past each other into two
    // windows.
    if let Some(existing) = app
        .webview_windows()
        .values()
        .find(|w| w.label().starts_with("error-"))
    {
        let _ = existing.emit(
            "error-dialog-queued",
            serde_json::json!({ "download_id": download_id }),
        );
        show_window_ready(existing);
        log_startup(
            log,
            &format!(
                "error dialog {} already open — queued {download_id} into it",
                existing.label()
            ),
        );
        return Ok(());
    }
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second
        // copy (the main window can notice the same failure
        // twice across polls). Round 74 (13): the dialog pops in
        // front over whatever the user has open.
        show_window_ready(&window);
        log_startup(log, &format!("error dialog {label} already open — focused"));
        return Ok(());
    }
    build_error_dialog_window(&app, &state.manager, &download_id)?;
    Ok(())
}

/// Label of a pending link's standalone Add URL window
/// (Round 74, item 13): `add-url-<pending link id>`, mirroring
/// `complete-<download id>`.
fn add_url_window_label(link_id: &str) -> String {
    format!("add-url-{link_id}")
}

/// Build one standalone Add URL window (Round 74, item 13): a
/// separate OS window like the "Download complete" / "Download
/// failed" dialogs — the same bundle, frameless with the shared
/// `TitleBar.tsx` — but for a pending browser-extension or
/// clipboard capture, NOT tied to the main window: it can appear
/// on its own while the main window is hidden to the tray, and it
/// pops in front (see `show_window_ready`). Preferred size
/// ~560x520, the Add URL dialog's size, with no heartbeat
/// registration and no auto-reload (transient, like the
/// complete/error dialogs). The pending link id arrives
/// through an initialization script (`window.__ADD_URL_ID__`),
/// the same Round 57 mechanism as the progress window, with the
/// `#add-url=<id>` hash as the frontend's fallback. The window
/// title is the file name from the link (the user's correction:
/// the dialog shows the file name at the top instead of
/// "Add URL", and the URL field is locked) — "Add URL" when the
/// link has no file name to show.
///
/// Round 85 — RESIZABLE, with a small minimum (460x360) instead
/// of the Round 74 fixed 560x520: the R84 retest found the
/// dialog "not draggable and taller than a 500px viewport
/// (buttons below the fold)". A fixed 520-tall window cannot fit
/// a ~500px viewport at all — the footer is unreachable, and
/// the minimum (which equalled the size) forbade shrinking it.
/// The dialog itself is a flex column with a scrolling body and
/// a shrink-0 footer (see AddDownloadDialog), so a smaller
/// window stays usable: the body scrolls, Start Download stays
/// visible. The preferred size is unchanged, so normal screens
/// see exactly the same window; this only unblocks small (RDP)
/// viewports. Drag still comes from the TitleBar's
/// `data-tauri-drag-region` (Round 76 (1)).
fn build_add_url_window(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    link_id: &str,
) -> Result<WebviewWindow, String> {
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    let row = manager
        .store()
        .get_pending_link(link_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Pending link not found: {link_id}"))?;
    let label = add_url_window_label(link_id);
    // Round 74 (1) — build re-check (see `build_progress_window`):
    // focus the existing window instead of a second one.
    if let Some(existing) = app.get_webview_window(&label) {
        log_startup(
            log,
            &format!("add URL window {label} already open — focused (build re-check)"),
        );
        show_window_ready(&existing);
        return Ok(existing);
    }
    let url = WebviewUrl::App("index.html".into());
    let init_script = format!("window.__ADD_URL_ID__ = {link_id:?};");
    // File name from the link for the title bar: the last URL
    // segment, without any query string.
    let title = row
        .url
        .split('?')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("Add URL")
        .to_string();
    log_startup(
        log,
        &format!("creating add URL window {label}: index.html (id via init script)"),
    );
    // Round 58 (§9.1): share the main window's WebView2
    // environment instead of creating a fresh one per window —
    // a fresh environment creation waits unbounded on the main
    // thread (`wait_with_pump`), the Round 58 wedge. Windows
    // only; other platforms build exactly as before.
    #[cfg(target_os = "windows")]
    let shared_environment = main_webview_environment(app, log);
    #[cfg(target_os = "windows")]
    if shared_environment.is_some() {
        log_startup(
            log,
            &format!("add URL window {label}: sharing main WebView2 environment"),
        );
    }
    let build_started = std::time::Instant::now();
    let mut builder = WebviewWindowBuilder::new(app, &label, url)
        .initialization_script(init_script)
        .title(title)
        .decorations(false)
        .inner_size(560.0, 520.0)
        .min_inner_size(460.0, 360.0)
        .resizable(true)
        .center()
        .devtools(true)
        // Round 71 (C1/C3) — #0B0F24 first paint + hidden until
        // the frontend signals ready (see the progress window).
        .background_color(tauri::window::Color(11, 15, 36, 255))
        .visible(false);
    #[cfg(target_os = "windows")]
    if let Some(env) = shared_environment {
        builder = builder.with_environment(env);
    }
    let build_thread_id = std::thread::current().id();
    log_startup(
        log,
        &format!("add URL window {label}: build() entered on thread {build_thread_id:?}"),
    );
    match builder.build() {
        Ok(window) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "add URL window {label} created: index.html in {elapsed_ms}ms on thread {build_thread_id:?}"
                ),
            );
            // Round 71 (C3) — built hidden; shown when the
            // frontend is ready (3 s fallback).
            spawn_show_fallback(&window);
            Ok(window)
        }
        Err(e) => {
            let elapsed_ms = build_started.elapsed().as_millis();
            log_startup(
                log,
                &format!(
                    "add URL window {label} FAILED to create after {elapsed_ms}ms on thread {build_thread_id:?}: {e}"
                ),
            );
            Err(e.to_string())
        }
    }
}

/// Open (or focus, when it already exists) the standalone Add URL
/// window for a pending browser-extension or clipboard capture
/// (Round 74, item 13). The window is independent of the main
/// window and pops in front over whatever the user has open, so
/// a captured link never waits behind the browser. Errors when
/// the pending link does not exist.
#[tauri::command]
async fn open_add_url_window(
    app: AppHandle,
    state: State<'_, AppState>,
    link_id: String,
) -> Result<(), String> {
    // Round 74 (1): the check-and-build holds the window-build
    // mutex (see `open_progress_window`).
    let _guard = lock_window_build();
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let label = add_url_window_label(&link_id);
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second
        // copy. Round 74 (13): the window pops in front over
        // whatever the user has open.
        show_window_ready(&window);
        log_startup(
            log,
            &format!("add URL window {label} already open — focused"),
        );
        return Ok(());
    }
    build_add_url_window(&app, &state.manager, &link_id)?;
    Ok(())
}

/// Round 74 (13) — one pending browser-extension / clipboard
/// capture row, for the standalone Add URL window to render
/// (file name, URL, referrer). A thin wrapper over the store's
/// existing getter, reached through the manager like the other
/// pending-link commands.
#[tauri::command]
fn get_pending_link(
    state: State<'_, AppState>,
    link_id: String,
) -> Result<Option<PendingLinkRow>, String> {
    state
        .manager
        .store()
        .get_pending_link(&link_id)
        .map_err(|e| e.to_string())
}

/// Round 81 — enqueue a clipboard-copied link as a pending
/// capture (`source = "browser"`), so it opens in its own
/// standalone Add URL window like an extension capture. Until
/// now the frontend opened the Add URL dialog directly inside
/// the MAIN window for a clipboard offer — and when the main
/// window was hidden to the tray, the user saw nothing on top
/// and had to "go into the program" to find it. The main
/// window's 1 s pending-captures poll picks the new row up and
/// the capture queue opens the standalone window, which pops
/// in front (see `show_window_ready`). The URL was already
/// validated by the clipboard offer check
/// (`check_clipboard_offer`); the enqueue re-validates scheme
/// and excluded sites, so a failure here is a real refusal.
#[tauri::command]
fn enqueue_clipboard_capture(
    state: State<'_, AppState>,
    url: String,
) -> Result<PendingLinkRow, String> {
    state
        .manager
        .store()
        .enqueue_pending_link(&url, None, "browser")
        .map_err(|e| e.to_string())
}

/// Round 62 — per-download completion options (progress window
/// Options tab): "Close program after download" and "Shut down
/// computer after download". Stored in memory in AppState; the
/// progress window reads them back on mount so a reopened window
/// shows the checked state. The window itself acts on them when
/// the download completes (it is the only view that knows the
/// options were checked for this run).
#[tauri::command]
fn set_download_completion_options(
    state: State<'_, AppState>,
    download_id: String,
    close_after: bool,
    shutdown_after: bool,
) -> Result<(), String> {
    let Ok(mut map) = state.download_completion.lock() else {
        return Err("completion options are unavailable".to_string());
    };
    map.insert(
        download_id,
        DownloadCompletionOptions {
            close_after,
            shutdown_after,
        },
    );
    Ok(())
}

#[tauri::command]
fn get_download_completion_options(
    state: State<'_, AppState>,
    download_id: String,
) -> Result<DownloadCompletionOptions, String> {
    let Ok(map) = state.download_completion.lock() else {
        return Ok(DownloadCompletionOptions::default());
    };
    Ok(map.get(&download_id).copied().unwrap_or_default())
}

/// Round 62 — "Close program after download": exit the whole app.
/// The progress window calls this when its download completes
/// with the option checked.
#[tauri::command]
fn exit_app(app: AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Round 62 — "Shut down computer after download": the Windows
/// shutdown command, the same call the engine's queue "shutdown"
/// action makes (engine queue.rs). Windows only; anywhere else
/// it is an error and the window says so instead of pretending
/// the PC will shut down.
#[tauri::command]
fn shutdown_computer() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("shutdown")
            .args(["/s", "/t", "0"])
            // Round 70 — no console flash for the shutdown call.
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Shutting down the computer is only available on Windows".to_string())
    }
}

/// Round 62 — free bytes on the volume holding `path` (the main
/// window's status bar shows the download drive's free space).
/// Windows only, via GetDiskFreeSpaceExW from kernel32 (a small
/// extern block — no extra crate); anywhere else it is an error
/// and the UI hides the figure.
#[tauri::command]
fn get_free_disk_space(path: String) -> Result<u64, String> {
    #[cfg(target_os = "windows")]
    {
        free_disk_space_windows(&path)
            .ok_or_else(|| format!("Could not read free space for {path}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err("Free disk space is only available on Windows".to_string())
    }
}

#[cfg(target_os = "windows")]
fn free_disk_space_windows(path: &str) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lp_directory_name: *const u16,
            lp_free_bytes_available: *mut u64,
            lp_total_number_of_bytes: *mut u64,
            lp_total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes as *mut u64,
            &mut total_bytes as *mut u64,
            &mut total_free_bytes as *mut u64,
        )
    };
    if ok == 0 {
        None
    } else {
        Some(free_bytes)
    }
}

/// The latest progress event for a download (see
/// `DownloadManager::last_progress`). The progress window polls
/// this — the frontend `download-progress` listener never fires
/// in the built app (BUG-044).
#[tauri::command]
fn get_progress_snapshot(
    state: State<'_, AppState>,
    download_id: String,
) -> Result<Option<fetchline_engine::ProgressEvent>, String> {
    Ok(state.manager.last_progress(&download_id))
}

/// Webview heartbeat (§7.4, Round 56): each window's JS pings
/// this every 5 s with its label (`main`,
/// `progress-<download id>`). The monitor thread in `setup`
/// logs "heartbeat lost" when a window's pings stop for 30 s —
/// the renderer may be dead even though the window still exists
/// (the r55 live test: both windows blank white, no console
/// data, nothing in the log). A ping that arrives after a loss
/// clears the flag, so a recovered window logs "resumed". The
/// command never fails: a heartbeat must never surface as an
/// error to the window that sends it.
#[tauri::command]
fn webview_heartbeat(state: State<'_, AppState>, label: String) -> Result<(), String> {
    let now = epoch_ms();
    let Ok(mut map) = state.webview_heartbeats.lock() else {
        return Ok(());
    };
    match map.get_mut(&label) {
        Some(hb) => {
            hb.last_seen_ms = now;
        }
        None => {
            map.insert(label, WebviewHeartbeat::new(now));
        }
    }
    Ok(())
}

/// Reload a window's webview (§7.4, Round 56; §9.1a, Round 57):
/// the recovery action the "Backend not responding" banner
/// offers. The main window is reloaded in place with the NATIVE
/// `WebviewWindow::reload()` — the old `eval("window.location.
/// reload()")` needed live JS + live IPC, exactly what is dead
/// when a reload is needed; a progress window is closed and
/// rebuilt from scratch, which also replaces a dead renderer
/// process with a fresh one. This is the user-initiated reload,
/// so the 5 min auto-reload cooldown does NOT apply. A reload
/// that fails (dead renderer) returns the error — the caller
/// shows it instead of pretending the window recovered.
#[tauri::command]
async fn reload_webview_window(
    app: AppHandle,
    state: State<'_, AppState>,
    label: String,
) -> Result<(), String> {
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    log_startup(log, &format!("reload requested for webview {label}"));
    reload_webview_now(&app, &state.manager, &label, &state.webview_heartbeats)
}

/// Antivirus hook (S14): run the configured scanner against a
/// finished download and record the result. A threat, a failure
/// and a timeout are results the dialog shows, not errors.
#[tauri::command]
async fn run_antivirus_scan(state: State<'_, AppState>, id: String) -> Result<ScanResult, String> {
    state
        .manager
        .run_antivirus_scan(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Antivirus hook (S14): the stored scan result for a download,
/// or `None` when it was never scanned.
#[tauri::command]
fn get_scan_result(state: State<'_, AppState>, id: String) -> Result<Option<ScanResult>, String> {
    state
        .manager
        .get_scan_result(&id)
        .map_err(|e| e.to_string())
}

/// Antivirus hook (S14): the Settings > Downloads "Test" button —
/// run the command typed in the dialog against a small throwaway
/// file and report the result. The scanner itself is external;
/// Fetchline never bundles one.
#[tauri::command]
async fn test_antivirus_scanner(
    state: State<'_, AppState>,
    command: String,
    timeout_secs: i64,
) -> Result<ScanResult, String> {
    state
        .manager
        .test_antivirus_scanner(&command, timeout_secs)
        .await
        .map_err(|e| e.to_string())
}

/// Antivirus hook (S14): move a file the scanner flagged as a
/// threat into a `Quarantine` folder. Explicit user action only —
/// nothing ever calls this on its own. Returns the new path.
#[tauri::command]
fn quarantine_download(state: State<'_, AppState>, id: String) -> Result<String, String> {
    state
        .manager
        .quarantine_download(&id)
        .map_err(|e| e.to_string())
}

/// Antivirus hook (S14): open a finished download in its default
/// application. Refused when the scanner flagged the file as a
/// threat, so a threat is never opened from the dialog.
#[tauri::command]
fn open_file(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.open_file(&id).map_err(|e| e.to_string())
}

/// Round 65 — "Open with..." (right-click menu): show the Windows
/// "Open with" dialog for a download's file, via
/// `rundll32 shell32.dll,OpenAs_RunDLL <path>`. The file must
/// exist on disk. Windows only; anywhere else it is an error and
/// the menu item is disabled by the frontend's state rules.
#[tauri::command]
fn open_file_with(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let row = state
        .manager
        .get_download(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {id}"))?;
    if !std::path::Path::new(&row.save_path).exists() {
        return Err(format!("File not found: {}", row.save_path));
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .args(["shell32.dll,OpenAs_RunDLL", &row.save_path])
            // Round 70 — no console flash for the Open With dialog.
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Open with is only available on Windows".to_string())
    }
}

#[tauri::command]
async fn delete_download(
    state: State<'_, AppState>,
    id: String,
    delete_file: bool,
) -> Result<(), String> {
    // Slice behaviour: remove the row from the list. `delete_file` also
    // removes the finished file from disk when it exists.
    let manager = &state.manager;
    let row = manager
        .get_download(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {id}"))?;
    if delete_file {
        let _ = std::fs::remove_file(&row.save_path);
    }
    manager
        .store()
        .set_status(&id, "deleted", None)
        .map_err(|e| e.to_string())
}

/// Command-entry trace (§7.1e, Round 56): sampled logging of the
/// two commands the UI polls/uses most, so a wedged invoke path
/// is visible in startup.log. `list_downloads` is polled every
/// second — logging every call would drown the file, so only
/// every 100th call is logged, plus any call slower than 1 s
/// (the interesting case). `add_download` is rare, so every call
/// logs its entry and its exit. A call whose entry never gets an
/// exit is a call that never returned. The log path is resolved
/// per call from the app handle, so the trace does not depend on
/// any state a wedged runtime could hide.
static LIST_DOWNLOADS_CALLS: AtomicU64 = AtomicU64::new(0);
static ADD_DOWNLOAD_CALLS: AtomicU64 = AtomicU64::new(0);

fn trace_command_exit(
    log: Option<&std::path::Path>,
    name: &str,
    n: u64,
    started: std::time::Instant,
    ok: bool,
) {
    log_startup(
        log,
        &format!(
            "cmd {name} #{n} exit in {}ms ({})",
            started.elapsed().as_millis(),
            if ok { "ok" } else { "err" }
        ),
    );
}

#[tauri::command]
async fn list_downloads(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<DownloadRow>, String> {
    let n = LIST_DOWNLOADS_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let started = std::time::Instant::now();
    let log = startup_log_path_from_app(&app);
    // Round 57 (§9.2e): log the ENTRY as well as the exit (same
    // sampling) — a hung call leaves an entry-without-exit trace,
    // which is the only way to tell a wedged invoke from a poll
    // that simply never ran. The entry line carries the call
    // number so it pairs with its exit line.
    if n % 100 == 0 {
        log_startup(log.as_deref(), &format!("cmd list_downloads #{n} entry"));
    }
    let result = state.manager.list_downloads().map_err(|e| e.to_string());
    // Sampled (§7.1e): every 100th call, or any call slower than
    // 1 s — a wedged invoke shows up as a missing sampled exit.
    if n % 100 == 0 || started.elapsed() > Duration::from_secs(1) {
        trace_command_exit(log.as_deref(), "list_downloads", n, started, result.is_ok());
    }
    result
}

/// Download history (Round 53): finished downloads, newest
/// first. `query` filters by filename or URL; empty = everything.
#[tauri::command]
async fn list_download_history(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<HistoryRow>, String> {
    state
        .manager
        .list_history(&query)
        .map_err(|e| e.to_string())
}

/// Download history (Round 53): empty the whole history.
/// Downloads in the main table are untouched.
#[tauri::command]
async fn clear_download_history(state: State<'_, AppState>) -> Result<(), String> {
    state.manager.clear_history().map_err(|e| e.to_string())
}

/// Download history (Round 53): re-download a finished file from
/// its history entry — a fresh download from the stored URL, into
/// the same folder, under the same name and category.
#[tauri::command]
async fn redownload_from_history(
    state: State<'_, AppState>,
    id: String,
) -> Result<(String, ProbeInfo), String> {
    state
        .manager
        .redownload_from_history(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Download history (Round 53): open the folder a history
/// entry's file lives in.
#[tauri::command]
async fn open_history_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .open_history_folder(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_categories(state: State<'_, AppState>) -> Result<Vec<CategoryRow>, String> {
    // Architecture.md names this command `get_categories`; it is the
    // category list (there is no separate `list_categories`).
    state
        .manager
        .store()
        .list_categories()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn create_category(
    state: State<'_, AppState>,
    name: String,
    folder: String,
    extensions: Vec<String>,
) -> Result<CategoryRow, String> {
    state
        .manager
        .create_category(&name, &folder, &extensions)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_category(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .delete_category(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn move_to_category(
    state: State<'_, AppState>,
    download_id: String,
    category_id: String,
) -> Result<(), String> {
    state
        .manager
        .move_to_category(&download_id, &category_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn create_queue(state: State<'_, AppState>, name: String) -> Result<QueueRow, String> {
    state.manager.create_queue(&name).map_err(|e| e.to_string())
}

/// Delete a custom queue (Round 74, item 6): the frontend sidebar's
/// delete button had no command to call — only `create_queue` /
/// `update_queue` existed, and `update_queue` takes no name. The
/// queue's downloads move back to the main queue (engine-side).
#[tauri::command]
async fn delete_queue(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .delete_queue(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Rename a custom queue (Round 74, item 6): same gap as
/// `delete_queue` — the sidebar's rename had no command to call.
#[tauri::command]
async fn rename_queue(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<QueueRow, String> {
    state
        .manager
        .rename_queue(&id, &name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn update_queue(
    state: State<'_, AppState>,
    id: String,
    schedule_type: String,
    start_at: Option<String>,
    stop_at: Option<String>,
    start_on_startup: bool,
    on_complete_action: String,
    on_complete_program: Option<String>,
    sync_interval_minutes: Option<i64>,
) -> Result<QueueRow, String> {
    state
        .manager
        .update_queue(
            &id,
            &schedule_type,
            start_at.as_deref(),
            stop_at.as_deref(),
            start_on_startup,
            &on_complete_action,
            on_complete_program.as_deref(),
            sync_interval_minutes,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Periodic synchronisation (S05/F04): sync a queue now, outside its
/// schedule, and wait for the per-file result.
#[tauri::command]
async fn sync_queue_now(state: State<'_, AppState>, id: String) -> Result<SyncResult, String> {
    state
        .manager
        .run_sync_queue(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_queues(state: State<'_, AppState>) -> Result<Vec<QueueRow>, String> {
    state.manager.list_queues().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_queue(state: State<'_, AppState>, id: String) -> Result<Option<QueueRow>, String> {
    state
        .manager
        .get_queue(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_queue_files(
    state: State<'_, AppState>,
    queue_id: String,
) -> Result<Vec<DownloadRow>, String> {
    state
        .manager
        .list_queue_files(&queue_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_to_queue(
    state: State<'_, AppState>,
    download_id: String,
    queue_id: String,
) -> Result<(), String> {
    state
        .manager
        .add_to_queue(&download_id, &queue_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_from_queue(state: State<'_, AppState>, download_id: String) -> Result<(), String> {
    state
        .manager
        .remove_from_queue(&download_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reorder_queue(
    state: State<'_, AppState>,
    queue_id: String,
    download_id: String,
    new_index: usize,
) -> Result<(), String> {
    state
        .manager
        .reorder_queue(&queue_id, &download_id, new_index)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_queue(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .start_queue(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_queue(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .stop_queue(&id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn execute_queue_action(state: State<'_, AppState>, queue_id: String) -> Result<(), String> {
    state
        .manager
        .execute_queue_action(&queue_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn dismiss_queue_action(state: State<'_, AppState>, queue_id: String) -> Result<(), String> {
    state
        .manager
        .dismiss_queue_action(&queue_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_page_links(
    state: State<'_, AppState>,
    page_url: String,
) -> Result<Vec<PageLink>, String> {
    state
        .manager
        .fetch_page_links(&page_url)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_batch(
    state: State<'_, AppState>,
    urls: Vec<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    start_now: bool,
) -> Result<BatchResult, String> {
    state
        .manager
        .add_batch_downloads(&urls, folder, category, queue, start_now)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_all_links(
    state: State<'_, AppState>,
    page_url: String,
    links: Vec<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    start_now: bool,
) -> Result<BatchResult, String> {
    state
        .manager
        .download_all_links(&page_url, links, folder, category, queue, start_now)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_pending_action(
    state: State<'_, AppState>,
    queue_id: String,
) -> Result<Option<PendingAction>, String> {
    Ok(state.manager.pending_action(&queue_id).await)
}

// -- settings (S09), proxy, speed limit (S12) ---------------------------

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let settings = state.manager.get_settings().map_err(|e| e.to_string())?;
    // BUG-029: the frontend calls this once on mount, so the first
    // success proves the webview loaded and reached the backend.
    // Logged once per process — settings are re-read often.
    static FRONTEND_SEEN: std::sync::Once = std::sync::Once::new();
    FRONTEND_SEEN.call_once(|| {
        startup_log_line(
            &startup_log_path_early(),
            "frontend reached backend: get_settings ok",
        );
    });
    Ok(settings)
}

/// Save the settings and apply them live (connection count, speed
/// cap, proxy, retries — see `DownloadManager::apply_settings`).
#[tauri::command]
async fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, String> {
    state
        .manager
        .update_settings(&settings)
        .map_err(|e| e.to_string())
}

/// Change only the global speed cap (architecture.md
/// `set_speed_limit`). `None` (or 0) means unlimited.
#[tauri::command]
async fn set_speed_limit(
    state: State<'_, AppState>,
    kbps: Option<i64>,
) -> Result<Settings, String> {
    state
        .manager
        .set_speed_limit(kbps)
        .map_err(|e| e.to_string())
}

/// The stored speed cap in KB/s (`None` = unlimited).
#[tauri::command]
async fn get_speed_limit(state: State<'_, AppState>) -> Result<Option<i64>, String> {
    state.manager.get_speed_limit().map_err(|e| e.to_string())
}

/// Set the download quota (architecture.md `set_download_limit`):
/// X MB every Y hours, enabled. The live window is untouched —
/// enabling a quota never resets usage already counted.
#[tauri::command]
async fn set_download_limit(
    state: State<'_, AppState>,
    mb: i64,
    hours: i64,
) -> Result<Settings, String> {
    state
        .manager
        .set_download_limit(mb, hours)
        .map_err(|e| e.to_string())
}

/// The download quota's live state (S12): limit, usage, window
/// end, whether it is reached, how many downloads it paused.
#[tauri::command]
async fn get_quota_status(state: State<'_, AppState>) -> Result<QuotaStatus, String> {
    state.manager.get_quota_status().map_err(|e| e.to_string())
}

/// Reset the quota now (S12): start a fresh window at once and
/// resume the downloads the quota paused.
#[tauri::command]
async fn reset_quota(state: State<'_, AppState>) -> Result<QuotaStatus, String> {
    state.manager.reset_quota().await.map_err(|e| e.to_string())
}

/// Set (or clear) one download's per-download speed cap in KB/s
/// (S12). `None` (or 0) means no per-download cap — the global cap
/// still applies. Live-changeable on a running download.
#[tauri::command]
async fn set_download_speed_limit(
    state: State<'_, AppState>,
    id: String,
    kbps: Option<i64>,
) -> Result<(), String> {
    state
        .manager
        .set_download_speed_limit(&id, kbps)
        .map_err(|e| e.to_string())
}

/// The saved proxy row, if any. Never contains the password — only
/// whether one is stored.
#[tauri::command]
async fn get_proxy_config(state: State<'_, AppState>) -> Result<Option<ProxyConfigRow>, String> {
    state.manager.get_proxy_config().map_err(|e| e.to_string())
}

/// Save the proxy (password: `None` keeps the stored one) and
/// rebuild the HTTP client so it is used at once.
#[tauri::command]
async fn save_proxy_config(
    state: State<'_, AppState>,
    input: ProxyConfigInput,
) -> Result<ProxyConfigRow, String> {
    state
        .manager
        .save_proxy_config(&input)
        .map_err(|e| e.to_string())
}

/// Probe a URL through a proxy that has NOT been saved yet (the Test
/// button in the Proxy tab). Returns the HTTP status on a 2xx answer;
/// a 407, any other non-2xx status, an FTP proxy tested against an
/// http(s) URL, or a proxy scoped to another scheme is an error,
/// never a success (BUG-009/010). An ftp:// test URL is probed
/// through a draft FTP proxy (SIZE) and returns 200 on success.
/// Kerberos (Negotiate/SPNEGO), NTLM and Basic are attempted for
/// http:// URLs in the order the saved auth setting allows
/// (default: Kerberos, then NTLM, then Basic); https:// through a
/// Kerberos or NTLM proxy and a Kerberos-only proxy with no ticket
/// are clear errors naming the scheme.
#[tauri::command]
async fn test_proxy(
    state: State<'_, AppState>,
    input: ProxyConfigInput,
    test_url: String,
) -> Result<u16, String> {
    state
        .manager
        .test_proxy(&input, &test_url)
        .await
        .map_err(|e| e.to_string())
}

/// Site logins (S09 Site logins tab). Rows never contain a
/// password — only `has_password`. `password: None` on save keeps
/// the stored password; a value sets it (protected via secret.rs).
#[tauri::command]
async fn list_site_logins(state: State<'_, AppState>) -> Result<Vec<SiteLoginRow>, String> {
    state.manager.list_site_logins().map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_site_login(
    state: State<'_, AppState>,
    input: SiteLoginInput,
) -> Result<SiteLoginRow, String> {
    state
        .manager
        .save_site_login(&input)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_site_login(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .delete_site_login(&id)
        .map_err(|e| e.to_string())
}

/// Sounds (S09 Sounds tab): one setting per event. The engine
/// emits the event and plays the WAV on Windows only.
#[tauri::command]
async fn get_sound_settings(state: State<'_, AppState>) -> Result<Vec<SoundSetting>, String> {
    state
        .manager
        .get_sound_settings()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_sound_settings(
    state: State<'_, AppState>,
    inputs: Vec<SoundSettingInput>,
) -> Result<Vec<SoundSetting>, String> {
    state
        .manager
        .save_sound_settings(&inputs)
        .map_err(|e| e.to_string())
}

/// Play a WAV file (the Sounds tab's Test button). Validates the
/// file first; on this Linux VM playback is Windows-only, so the
/// honest result here is the validation, not audio.
#[tauri::command]
async fn test_sound_file(state: State<'_, AppState>, path: String) -> Result<String, String> {
    state
        .manager
        .test_sound_file(&path)
        .map_err(|e| e.to_string())
}

/// Round 66 — the built-in "Download complete" chime,
/// synthesized at runtime (no binary asset to ship through the
/// connector, which cannot push binaries): two sine tones —
/// A5 (880 Hz) for 0.22 s, then D6 (1174.66 Hz) for 0.32 s —
/// 22050 Hz 16-bit mono PCM with a short attack and an
/// exponential decay, so it sounds like a chime, not a beep.
/// The bytes are deterministic: the same code always makes the
/// same WAV.
fn builtin_complete_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 22_050;
    const TONES: [(f64, f64); 2] = [(880.0, 0.22), (1174.66, 0.32)];
    let mut samples: Vec<i16> = Vec::new();
    for (freq, seconds) in TONES {
        let count = (seconds * SAMPLE_RATE as f64) as usize;
        for n in 0..count {
            let t = n as f64 / SAMPLE_RATE as f64;
            // 10 ms linear attack, then exponential decay —
            // no click at the start, no abrupt cut at the end.
            let attack = (t / 0.01).min(1.0);
            let decay = (-3.0 * t / seconds).exp();
            let value = (freq * 2.0 * std::f64::consts::PI * t).sin() * attack * decay;
            samples.push((value * 0.8 * i16::MAX as f64) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Round 68 — the built-in "Download failed" alert, synthesized
/// at runtime exactly like `builtin_complete_wav()` (same sample
/// rate, same attack/decay envelope, deterministic bytes) but
/// DESCENDING, so the ear tells failure from completion at once:
/// 740 Hz for 0.16 s, then 494 Hz for 0.32 s. The user heard this
/// render (`workspace/sonic-error-alert.wav`) and approved it over
/// the Windows error sounds.
fn builtin_error_wav() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 22_050;
    const TONES: [(f64, f64); 2] = [(740.0, 0.16), (494.0, 0.32)];
    let mut samples: Vec<i16> = Vec::new();
    for (freq, seconds) in TONES {
        let count = (seconds * SAMPLE_RATE as f64) as usize;
        for n in 0..count {
            let t = n as f64 / SAMPLE_RATE as f64;
            // 10 ms linear attack, then exponential decay —
            // no click at the start, no abrupt cut at the end.
            let attack = (t / 0.01).min(1.0);
            let decay = (-3.0 * t / seconds).exp();
            let value = (freq * 2.0 * std::f64::consts::PI * t).sin() * attack * decay;
            samples.push((value * 0.8 * i16::MAX as f64) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Round 68 — one shared playback path for both notification
/// sounds (completion + error). Order: (0) the master sounds
/// switch in Settings — off means silence, no matter what else
/// is configured; (1) when the user configured a sound for the
/// event, the engine already played it at the event
/// (manager.rs `emit_sound`), so this is a no-op and the sound
/// never plays twice; (2) a bundled asset for the event
/// (`tada.wav` for completion — shipped in the app so nothing
/// external is needed), resolved from the Tauri resources and
/// played straight from there; (3) the synthesized built-in,
/// written to the app data dir (temp dir when that is
/// unavailable) and played with the SoundPlayer; (4) when that
/// fails, the Windows default Asterisk sound. Best-effort — a
/// failure is an error to the caller, and the dialogs swallow
/// it (a silent dialog is better than an error banner).
/// Windows only, like every other sound in the app.
#[cfg(target_os = "windows")]
fn play_notification_sound(
    app: &AppHandle,
    state: &State<'_, AppState>,
    event: &str,
    builtin_asset: Option<&str>,
    builtin_file: &str,
    builtin_wav: Vec<u8>,
    builtin_desc: &str,
) -> Result<String, String> {
    if let Ok(settings) = state.manager.get_settings() {
        if !settings.sounds_enabled {
            return Ok("sounds are turned off in Settings".to_string());
        }
    }
    if let Ok(Some(file)) = state.manager.resolve_sound(event) {
        if std::path::Path::new(&file).is_file() {
            return Ok(format!(
                "user-configured {event} sound (already played by the engine)"
            ));
        }
    }
    if let Some(asset) = builtin_asset {
        if let Ok(path) = app
            .path()
            .resolve(asset, tauri::path::BaseDirectory::Resource)
        {
            if path.is_file() {
                return match play_wav_with_sound_player(&path) {
                    Ok(()) => Ok(format!("bundled {asset}")),
                    Err(_) => play_default_system_sound().map(|()| {
                        "Windows default sound (bundled sound could not play)".to_string()
                    }),
                };
            }
        }
    }
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    let path = dir.join(builtin_file);
    if std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, builtin_wav))
        .is_err()
    {
        // Writing the built-in failed — fall back to the
        // Windows default sound instead of silence.
        return play_default_system_sound()
            .map(|()| "Windows default sound (built-in sound could not be written)".to_string());
    }
    match play_wav_with_sound_player(&path) {
        Ok(()) => Ok(builtin_desc.to_string()),
        Err(_) => play_default_system_sound()
            .map(|()| "Windows default sound (built-in sound could not play)".to_string()),
    }
}
#[cfg(target_os = "windows")]
fn play_wav_with_sound_player(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let escaped = path.to_string_lossy().replace('\'', "''");
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(New-Object Media.SoundPlayer '{escaped}').PlaySync()"),
        ])
        // Round 70 — without this the player flashes a black console
        // window on every sound (a failed download plays the error
        // sound twice: engine + error dialog).
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Windows default Asterisk sound, the last-resort completion
/// sound when the built-in chime cannot be written or played.
#[cfg(target_os = "windows")]
fn play_default_system_sound() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "[System.Media.SystemSounds]::Asterisk.Play()",
        ])
        // Round 70 — see play_wav_with_sound_player: no console flash.
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Round 66 — play the completion sound when the "Download
/// complete" dialog shows. Round 68: the built-in is the bundled
/// Windows "tada" (`assets/tada.wav`, shipped in the app), with
/// the synthesized chime as the fallback when the asset is
/// missing — the user picked tada and ordered it bundled so
/// nothing external is needed. The shared
/// `play_notification_sound` helper owns the order (master
/// switch → user-configured file → bundled tada → chime →
/// Windows default).
#[tauri::command]
async fn play_completion_sound(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        play_notification_sound(
            &app,
            &state,
            "download_complete",
            Some("tada.wav"),
            "complete.wav",
            builtin_complete_wav(),
            "built-in download-complete chime",
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&app, &state);
        Err("Sound playback is only available on Windows".to_string())
    }
}

/// Round 68 — play the error alert when the "Download failed"
/// dialog shows. Same shared path as the completion sound: the
/// master switch, then the user-configured `download_failed`
/// file (the engine already played it), then the synthesized
/// descending alert, then the Windows default sound.
#[tauri::command]
async fn play_error_sound(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        play_notification_sound(
            &app,
            &state,
            "download_failed",
            None,
            "error.wav",
            builtin_error_wav(),
            "built-in download-failed alert",
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (&app, &state);
        Err("Sound playback is only available on Windows".to_string())
    }
}

/// Round 68 — taskbar progress (Windows): show the overall
/// download progress on the main window's taskbar button, and
/// put the file / speed / time-left summary in the native window
/// title — that is what the taskbar tooltip shows (Windows has
/// no API for a custom taskbar tooltip). `progress` is 0–100 or
/// None to clear the bar; `status` is "normal" | "paused" |
/// "error". The frontend calls this from App.tsx whenever the
/// rounded percent or the summary text changes — never more than
/// once per refresh poll. Best-effort: a missing main window is
/// a no-op, and every call is swallowed by the caller.
#[tauri::command]
fn set_taskbar_progress(
    app: AppHandle,
    progress: Option<u64>,
    status: Option<String>,
    title: Option<String>,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let state = match (progress, status.as_deref()) {
        (Some(p), Some("paused")) => tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::Paused),
            progress: Some(p.min(100)),
        },
        (Some(p), Some("error")) => tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::Error),
            progress: Some(p.min(100)),
        },
        (Some(p), _) => tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::Normal),
            progress: Some(p.min(100)),
        },
        (None, _) => tauri::window::ProgressBarState {
            status: Some(tauri::window::ProgressBarStatus::None),
            progress: None,
        },
    };
    let _ = window.set_progress_bar(state);
    if let Some(title) = title {
        let _ = window.set_title(&title);
    }
    Ok(())
}

/// Change a category's save folder and/or file extensions
/// (S09 Save To + File Types tabs).
#[tauri::command]
async fn update_category(
    state: State<'_, AppState>,
    id: String,
    folder: Option<String>,
    extensions: Option<Vec<String>>,
) -> Result<CategoryRow, String> {
    state
        .manager
        .update_category(&id, folder.as_deref(), extensions.as_deref())
        .map_err(|e| e.to_string())
}

/// Video grabber (S08 / F05): scan a page for downloadable
/// video/audio, for the desktop video panel. Only openly
/// downloadable streams are returned; dead links carry their probe
/// error, and an .m3u8 playlist is flagged live.
#[tauri::command]
async fn detect_page_videos(
    state: State<'_, AppState>,
    page_url: String,
) -> Result<Vec<DetectedVideo>, String> {
    state
        .manager
        .detect_page_videos(&page_url)
        .await
        .map_err(|e| e.to_string())
}

/// Update check (S09, "Quick update check"): Help > Check for
/// updates. Fetches the version manifest from the configured
/// update server (Fetchline's own; see engine/src/update.rs) and
/// reports whether a newer version exists. Never installs
/// anything — the UI shows the result and asks the user. The
/// Tauri updater plugin is NOT configured in this build, so
/// there is no in-app install yet; the result says so
/// (`updater_configured: false`) and the dialog offers the
/// installer address instead.
#[tauri::command]
async fn check_for_updates(state: State<'_, AppState>) -> Result<UpdateCheckResult, String> {
    state
        .manager
        .check_for_updates()
        .await
        .map_err(|e| e.to_string())
}

/// Drain the browser-extension pending-links queue (S13): every
/// link the native host enqueued is put through the real
/// `add_download` path. Milestone 2: the drain STARTS each created
/// download at once when `settings.browser_start_drained` is on
/// (the default); with it off the downloads are left `queued`.
/// Round 67: the drain only handles 'download_all' rows now —
/// 'browser' and 'video' captures are interactive and wait for
/// the Add URL dialog (`confirm_pending_link`).
#[tauri::command]
async fn drain_pending_links(state: State<'_, AppState>) -> Result<DrainSummary, String> {
    state
        .manager
        .drain_pending_links()
        .await
        .map_err(|e| e.to_string())
}

/// Round 67 — the Add URL dialog's queue: pending
/// browser-extension rows, oldest first. The app polls these and
/// opens the dialog for each browser/video capture in turn.
#[tauri::command]
fn list_pending_links(state: State<'_, AppState>) -> Result<Vec<PendingLinkRow>, String> {
    state
        .manager
        .list_pending_links()
        .map_err(|e| e.to_string())
}

/// Round 67 — decide one pending row from the Add URL dialog:
/// 'accepted' with the created download id, or 'rejected' with
/// the reason (Cancel / close dismisses a capture).
#[tauri::command]
fn resolve_pending_link(
    state: State<'_, AppState>,
    id: String,
    status: String,
    download_id: Option<String>,
    error: Option<String>,
) -> Result<(), String> {
    state
        .manager
        .resolve_pending_link(&id, &status, download_id.as_deref(), error.as_deref())
        .map_err(|e| e.to_string())
}

/// Round 67 — probe a URL for the Add URL dialog (file size and
/// name, for the cyan size on the remember-path row). `cookies`
/// is a browser capture's stored cookies JSON, or `None` — a
/// cookie-gated link probes 401 without them. `headers` is the
/// resolved video's headers JSON (User-Agent / Referer from
/// yt-dlp's `http_headers`), or `None` — a stream host that
/// expects them answers the global "Fetchline/0.1" UA with 403.
#[tauri::command]
async fn probe_url(
    state: State<'_, AppState>,
    url: String,
    cookies: Option<String>,
    headers: Option<String>,
) -> Result<ProbeInfo, String> {
    state
        .manager
        .probe_url_for_capture_with_headers(&url, cookies.as_deref(), headers.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Round 78 — where the yt-dlp sidecars live for this app run:
/// the AppData bin dir first (where updates install), then next
/// to the app exe. The paths are returned through
/// `clean_windows_path`, so the qjs path passed to
/// `--js-runtimes` is one yt-dlp accepts (no `\\?\` prefix).
fn ytdlp_paths() -> fetchline_engine::ytdlp::YtDlpPaths {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    fetchline_engine::ytdlp::locate_sidecars(&exe_dir)
}

/// Round 78 — what the Settings UI shows for the video
/// component: the installed yt-dlp version (if any), its path,
/// and whether the QuickJS solver and ffmpeg are present.
#[derive(serde::Serialize)]
struct YtdlpStatus {
    version: Option<String>,
    path: String,
    qjs: bool,
    ffmpeg: bool,
}

/// Round 78 — the installed yt-dlp's version and sidecar
/// presence, for the Settings "Video downloads" section.
#[tauri::command]
async fn ytdlp_version() -> Result<YtdlpStatus, String> {
    let paths = ytdlp_paths();
    Ok(YtdlpStatus {
        version: fetchline_engine::ytdlp::ytdlp_version(&paths).await,
        path: clean_windows_path(&paths.ytdlp),
        qjs: paths.qjs.is_some(),
        // The merge tool is only whole when BOTH ffmpeg and
        // ffprobe are present — yt-dlp's merge needs both.
        ffmpeg: paths.ffmpeg.is_some() && paths.ffprobe.is_some(),
    })
}

/// Round 78 — the result of resolving a platform video page
/// from the app itself (a YouTube URL pasted into Add URL):
/// the title and the trimmed quality list, exactly like the
/// host's `resolved` reply. No cookies — the app has no browser
/// session; a login-gated video resolves through the extension
/// instead. The hidden extractor-args override from Settings is
/// honoured when set.
#[derive(serde::Serialize)]
struct YtdlpResolveResult {
    title: String,
    is_live: bool,
    degraded: bool,
    warning: Option<String>,
    formats: Vec<fetchline_engine::ytdlp::QualityRow>,
}

#[tauri::command]
async fn ytdlp_resolve(
    state: State<'_, AppState>,
    url: String,
) -> Result<YtdlpResolveResult, String> {
    let settings = state
        .manager
        .store()
        .get_settings()
        .map_err(|e| e.to_string())?;
    if !settings.platform_downloads_enabled {
        return Err("Platform downloads are turned off in Fetchline settings".to_string());
    }
    let opts = fetchline_engine::ytdlp::ResolveOptions {
        paths: ytdlp_paths(),
        cookies: Vec::new(),
        extractor_args: settings.ytdlp_extractor_args,
        timeout_secs: fetchline_engine::ytdlp::RESOLVE_TIMEOUT_SECS,
    };
    let video = fetchline_engine::ytdlp::resolve(&url, &opts)
        .await
        .map_err(|e| e.user_message())?;
    // The rows are computed before any field moves out of the
    // video (a struct literal cannot borrow the whole value
    // after a field has moved).
    let formats = fetchline_engine::ytdlp::quality_rows(&video);
    Ok(YtdlpResolveResult {
        title: video.title,
        is_live: video.is_live,
        degraded: video.degraded,
        warning: video.warnings.into_iter().next(),
        formats,
    })
}

/// Round 78 — download the newest yt-dlp.exe from its GitHub
/// release into the AppData bin dir, verified against the
/// release's published SHA-256 checksums (a failed checksum is
/// refused, never installed). Returns the fresh status, so the
/// Settings UI can show the new version at once.
#[tauri::command]
async fn ytdlp_update(state: State<'_, AppState>) -> Result<YtdlpStatus, String> {
    let proxy = state.manager.proxy_config();
    fetchline_engine::ytdlp::update_ytdlp(proxy.as_ref())
        .await
        .map_err(|e| e.user_message())?;
    let paths = ytdlp_paths();
    Ok(YtdlpStatus {
        version: fetchline_engine::ytdlp::ytdlp_version(&paths).await,
        path: clean_windows_path(&paths.ytdlp),
        qjs: paths.qjs.is_some(),
        // The merge tool is only whole when BOTH ffmpeg and
        // ffprobe are present — yt-dlp's merge needs both.
        ffmpeg: paths.ffmpeg.is_some() && paths.ffprobe.is_some(),
    })
}

/// Round 67 — set the Add URL dialog's Description note on a
/// download (`None` / empty clears it).
#[tauri::command]
fn set_download_description(
    state: State<'_, AppState>,
    id: String,
    description: Option<String>,
) -> Result<(), String> {
    state
        .manager
        .set_download_description(&id, description.as_deref())
        .map_err(|e| e.to_string())
}

/// Round 67 — confirm one pending browser/video capture through
/// the real add path, with the Add URL dialog's choices (like
/// IDM: the download only starts when the user presses Start
/// Download). The row's cookies ride the probe and the segments;
/// the speed cap is applied before the start.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn confirm_pending_link(
    state: State<'_, AppState>,
    id: String,
    filename: Option<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    speed_limit_kbps: Option<i64>,
    start_now: bool,
) -> Result<(String, ProbeInfo), String> {
    state
        .manager
        .confirm_pending_link(
            &id,
            filename,
            folder,
            category,
            queue,
            speed_limit_kbps,
            start_now,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Round 79 — add a download from a resolved platform video
/// (the manual Add URL dialog): the dialog resolved the page
/// with `ytdlp_resolve` and picked one quality row; the engine
/// creates the download from the row's stream URLs — never
/// from the page URL (a watch-page URL probes size 0 with no
/// ranges). Returns the created download id and its probe info.
/// Round 85 — `cookies` is the browser's consented `Cookie`
/// header for the page when the caller has one (a capture
/// flow); the manual dialog passes none (cookieless).
#[tauri::command]
async fn add_resolved_video_download(
    state: State<'_, AppState>,
    page_url: String,
    row: fetchline_engine::ytdlp::QualityRow,
    filename: Option<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
    speed_limit_kbps: Option<i64>,
    start_now: bool,
    cookies: Option<String>,
) -> Result<(String, ProbeInfo), String> {
    state
        .manager
        .add_resolved_video_download(
            &page_url,
            &row,
            filename,
            folder,
            category,
            queue,
            speed_limit_kbps,
            start_now,
            cookies,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Site grabber (S06 / F06): create a project (wizard steps 1–3).
/// The engine validates the inputs — depth over 5, a page cap over
/// 1000, no file types, an unknown queue, a bad completion action,
/// or an excluded start site are all rejected with a clear error.
/// `queue_id` is the queue grabbed files go to (the main queue
/// when omitted); `on_complete_action` reuses the existing queue
/// completion actions and is armed only when the run actually
/// downloaded something (Milestone 4).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn create_grabber_project(
    state: State<'_, AppState>,
    name: String,
    start_url: String,
    types: Vec<String>,
    depth: i64,
    max_pages: i64,
    mode: String,
    schedule_start_at: Option<String>,
    queue_id: Option<String>,
    on_complete_action: String,
    on_complete_program: Option<String>,
) -> Result<GrabberProjectRow, String> {
    state
        .manager
        .create_grabber_project_full(
            &name,
            &start_url,
            &types,
            depth,
            max_pages,
            &mode,
            schedule_start_at.as_deref(),
            queue_id.as_deref(),
            &on_complete_action,
            on_complete_program.as_deref(),
        )
        .map_err(|e| e.to_string())
}

/// Start a grabber run in the background; the UI polls
/// `get_grabber_project` for progress.
#[tauri::command]
async fn run_grabber(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .run_grabber(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Ask a running grabber to stop after the current page / file.
#[tauri::command]
async fn stop_grabber(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .stop_grabber(&id)
        .await
        .map_err(|e| e.to_string())
}

/// One grabber project with its live progress and matched files.
#[tauri::command]
async fn get_grabber_project(
    state: State<'_, AppState>,
    id: String,
) -> Result<GrabberProjectRow, String> {
    state
        .manager
        .get_grabber_project(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Site grabber project not found: {id}"))
}

/// All grabber projects, newest first.
#[tauri::command]
async fn list_grabber_projects(
    state: State<'_, AppState>,
) -> Result<Vec<GrabberProjectRow>, String> {
    state
        .manager
        .list_grabber_projects()
        .map_err(|e| e.to_string())
}

/// Rename a grabber project (Round 83 — the R82 retest
/// confirmed projects could neither be renamed nor deleted).
/// An empty name is rejected; an unknown id is NotFound.
/// Renaming a running project is allowed — the run keys on
/// the id, the name is display-only.
#[tauri::command]
async fn rename_grabber_project(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<GrabberProjectRow, String> {
    state
        .manager
        .rename_grabber_project(&id, &name)
        .map_err(|e| e.to_string())
}

/// Delete a grabber project (Round 83). Refused while the
/// project is running; its downloads are kept, untagged.
#[tauri::command]
async fn delete_grabber_project(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .manager
        .delete_grabber_project(&id)
        .await
        .map_err(|e| e.to_string())
}

/// Round 85 — restore the main window from the tray: a window
/// hidden to the tray (Round 65) is `show()`n; a window the OS
/// minimized (taskbar minimize, or Windows minimizing it) needs
/// `unminimize()` FIRST — `show()` alone leaves a minimized
/// window minimized, which is why the R84 retest's taskbar
/// restore did nothing while tray double-click (which fires
/// Click Up) worked. Order: unminimize, show, focus. Every
/// call is best-effort — a missing main window is not an error.
fn restore_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Round 65 — system tray, like IDM: a tray icon (the app logo)
/// lives by the clock while the app runs. Left-click shows the
/// main window again; right-click opens the menu (Show / Exit).
/// The menu events live here so the tray works even when every
/// window is hidden. Built best-effort in setup: if the icon or
/// the tray cannot be created the app still runs — the tray is a
/// convenience, not a requirement.
///
/// Round 86 — minimize now goes to the TASKBAR (a real
/// `minimize()` in TitleBar.tsx), superseding the Round 65
/// hide-on-minimize: the old behaviour failed the
/// taskbar-restore check in four consecutive rounds (R76, R83,
/// R84, R85). Only X hides to the tray now — see the
/// CloseRequested handler below, which is unchanged. Exactly
/// ONE tray icon remains.
fn build_system_tray(app: &AppHandle) -> Result<(), String> {
    let Some(icon) = app.default_window_icon() else {
        return Err("no default window icon for the tray".to_string());
    };
    let menu = MenuBuilder::new(app)
        .items(&[
            &MenuItemBuilder::with_id("tray-show", "Show")
                .build(app)
                .map_err(|e| e.to_string())?,
            &MenuItemBuilder::with_id("tray-exit", "Exit")
                .build(app)
                .map_err(|e| e.to_string())?,
        ])
        .build()
        .map_err(|e| e.to_string())?;
    TrayIconBuilder::new()
        .menu(&menu)
        .icon(icon.clone())
        .tooltip("Sonic Manager")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event: MenuEvent| match event.id().as_ref() {
            "tray-show" => {
                restore_main_window(app);
            }
            "tray-exit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click (release) and left double-click both
            // restore the main window. Right-click pops the menu
            // (handled by the tray itself); no action is needed
            // here for it.
            let app = tray.app_handle();
            let log = startup_log_path_from_app(app);
            let log = log.as_deref();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    log_startup(log, "tray: left click (up) — restoring main window");
                    restore_main_window(app);
                }
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    log_startup(log, "tray: left double-click — restoring main window");
                    restore_main_window(app);
                }
                TrayIconEvent::Click {
                    button,
                    button_state,
                    ..
                } => {
                    log_startup(
                        log,
                        &format!("tray: click {button:?} {button_state:?} — ignored"),
                    );
                }
                TrayIconEvent::DoubleClick { button, .. } => {
                    log_startup(log, &format!("tray: double-click {button:?} — ignored"));
                }
                TrayIconEvent::Enter { .. } => {
                    log_startup(log, "tray: mouse enter");
                }
                TrayIconEvent::Leave { .. } => {
                    log_startup(log, "tray: mouse leave");
                }
                // Move fires constantly while the cursor crosses
                // the icon — logging it would flood startup.log.
                TrayIconEvent::Move { .. } => {}
                // TrayIconEvent is #[non_exhaustive] (tauri 2.12.5):
                // a wildcard arm is required for the match to compile.
                _ => {}
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Round 69 (a) — register the browser extension's native messaging
/// host on startup (Windows only, best-effort).
///
/// Root cause of the Round 68 retest's "Not connected": the repo
/// manifest hardcodes `C:\Program Files\Sonic Manager\fetchline-host.exe`
/// (the MSI install path), and nothing registers the host except the
/// installer (the NSIS post-install script / MSI custom action). The
/// VPS runs a dev build that was never installed, so Chrome had no
/// `HKCU\...\NativeMessagingHosts\com.fetchline.host` key at all —
/// the popup's ping could never reach the host even though the host
/// binary itself is fine (it answers `pong` to a framed ping, and
/// parses native messages on the VPS).
///
/// So the app registers itself on every launch: it writes the
/// manifest (real host-exe path, UTF-8 WITHOUT a BOM — Chrome
/// rejects a BOM-prefixed manifest, which is why the NSIS script
/// writes it that way too) into the app data dir, and points the
/// Chrome + Edge (+ Firefox) `NativeMessagingHosts` registry keys
/// at it. Registration is idempotent — a key that already points
/// at the same manifest is left alone — and any failure is logged,
/// never fatal: a missing host registration must not stop the app
/// from starting.
#[cfg(target_os = "windows")]
/// Round 71 (A2) — strip the verbatim `\\?\` prefix that
/// `std::fs::canonicalize` puts on Windows paths, before the path
/// is written into a native-host manifest or the registry.
/// Chrome never launched the host from a manifest whose "path"
/// started with `\\?\` (host.log did not exist until a manual run
/// used the plain path). Also handles `\\?\UNC\server\share`
/// -> `\\server\share`. A path without the prefix is returned
/// unchanged (on non-Windows this is a no-op in practice).
fn clean_windows_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

fn register_native_host_best_effort(app: &AppHandle, log: Option<&std::path::Path>) {
    use std::os::windows::process::CommandExt;
    const EXTENSION_ID: &str = "mioamdgogndabpcjbdekggcnlkkfpmbj";
    const FIREFOX_EXTENSION_ID: &str = "fetchline@fetchline.app";

    // Where the host exe lives: next to the app exe in an installed
    // build, or under src-tauri/binaries in a dev build (the exe
    // sits in src-tauri/target/release, two levels below the app
    // dir). First existing candidate wins.
    let exe_dir = match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(dir) => dir.to_path_buf(),
            None => {
                log_startup(log, "native host: cannot find the app exe directory");
                return;
            }
        },
        Err(e) => {
            log_startup(log, &format!("native host: current_exe failed: {e}"));
            return;
        }
    };
    let candidates = [
        exe_dir.join("fetchline-host.exe"),
        // Round 70 — the portable ZIP ships the host beside the app
        // exe under its full triple name; the first ZIP missed this
        // candidate and the extension reported "host not found".
        exe_dir.join("fetchline-host-x86_64-pc-windows-msvc.exe"),
        exe_dir.join("binaries").join("fetchline-host.exe"),
        exe_dir
            .join("binaries")
            .join("fetchline-host-x86_64-pc-windows-msvc.exe"),
        exe_dir
            .join("..")
            .join("..")
            .join("binaries")
            .join("fetchline-host-x86_64-pc-windows-msvc.exe"),
    ];
    let host_exe = match candidates.iter().find(|p| p.is_file()) {
        Some(p) => p.clone(),
        None => {
            log_startup(
                log,
                "native host: fetchline-host.exe not found next to the app binary; extension host not registered",
            );
            return;
        }
    };
    // Canonicalize so the manifest holds a clean absolute path
    // (no `..\..` components from the dev-build candidate).
    let host_exe = std::fs::canonicalize(&host_exe).unwrap_or(host_exe);
    // Round 71 (A2) — `canonicalize` returns a verbatim path
    // (`\\?\C:\...`). Chrome never launched the host from a
    // manifest with that prefix: host.log did not exist until a
    // manual run used the plain path, and the manual framed ping
    // answered pong — so the failure is at LAUNCH, and the
    // `\\?\` prefix is the prime suspect. Strip it before the
    // path is written anywhere (manifest JSON and registry).
    // Also handles the `\\?\UNC\server\share` form.
    let host_path = clean_windows_path(&host_exe);
    log_startup(
        log,
        &format!("native host: host exe resolved to {host_path}"),
    );

    // Manifests live in the app data dir, next to startup.log —
    // a place the app owns in every layout (installed or dev).
    // `fs::write` of a String is UTF-8 without a BOM by
    // construction, which is what Chrome requires.
    let manifest_dir = match app.path().app_data_dir() {
        Ok(dir) => dir.join("fetchline").join("native-host"),
        Err(e) => {
            log_startup(log, &format!("native host: app_data_dir failed: {e}"));
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&manifest_dir) {
        log_startup(
            log,
            &format!("native host: cannot create {}: {e}", manifest_dir.display()),
        );
        return;
    }
    let chrome_manifest = manifest_dir.join("com.fetchline.host.json");
    let firefox_manifest = manifest_dir.join("com.fetchline.host.firefox.json");
    let chrome_json = serde_json::json!({
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
        "description": "Sonic Manager native messaging host — lets the Sonic Manager browser extension send links to the Sonic Manager desktop app",
        "name": "com.fetchline.host",
        "path": host_path,
        "type": "stdio"
    });
    let firefox_json = serde_json::json!({
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
        "description": "Sonic Manager native messaging host — lets the Sonic Manager browser extension send links to the Sonic Manager desktop app",
        "name": "com.fetchline.host",
        "path": host_path,
        "type": "stdio"
    });
    for (path, json) in [
        (&chrome_manifest, &chrome_json),
        (&firefox_manifest, &firefox_json),
    ] {
        let text = match serde_json::to_string_pretty(json) {
            Ok(t) => t,
            Err(e) => {
                log_startup(log, &format!("native host: manifest JSON failed: {e}"));
                return;
            }
        };
        if let Err(e) = std::fs::write(path, text) {
            log_startup(
                log,
                &format!("native host: cannot write {}: {e}", path.display()),
            );
            return;
        }
    }
    log_startup(
        log,
        &format!("native host: manifest written for {host_path}"),
    );

    // Point each browser's NativeMessagingHosts key at the
    // manifest. `reg query` first: a key already pointing at the
    // same manifest is left alone, so a normal launch spawns no
    // writes at all.
    let chrome_manifest_str = clean_windows_path(&chrome_manifest);
    let firefox_manifest_str = clean_windows_path(&firefox_manifest);
    let keys: [(&str, &str, &str); 3] = [
        (
            "Google Chrome",
            r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.fetchline.host",
            &chrome_manifest_str,
        ),
        (
            "Microsoft Edge",
            r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.fetchline.host",
            &chrome_manifest_str,
        ),
        (
            "Mozilla Firefox",
            r"HKCU\Software\Mozilla\NativeMessagingHosts\com.fetchline.host",
            &firefox_manifest_str,
        ),
    ];
    for (browser, key, manifest) in keys {
        let already = std::process::Command::new("reg")
            .args(["query", key, "/ve"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|out| {
                out.status.success() && String::from_utf8_lossy(&out.stdout).contains(manifest)
            })
            .unwrap_or(false);
        if already {
            log_startup(log, &format!("native host: {browser} already registered"));
            continue;
        }
        match std::process::Command::new("reg")
            .args(["add", key, "/ve", "/t", "REG_SZ", "/d", manifest, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => log_startup(
                log,
                &format!("native host: {browser} registered -> {manifest}"),
            ),
            Ok(out) => log_startup(
                log,
                &format!(
                    "native host: {browser} registration failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ),
            Err(e) => log_startup(
                log,
                &format!("native host: {browser} registration failed to run reg.exe: {e}"),
            ),
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Round 75 — the app-alive mutex: created at the very start of
    // run(), before any window exists, and held for the process
    // lifetime (Tauri managed state, in setup below). The
    // extension's host probes it, so the popup's "Connected" means
    // the APP is running — not just that the standalone host
    // answered a ping. A second launch opens the SAME named mutex
    // and gets its own handle; single-instance (Round 74) exits
    // that process before setup, closing only its own handle — the
    // primary instance's mutex survives. Off Windows this is None
    // (no named mutex) and the popup keeps its old behaviour.
    let app_alive_mutex = fetchline_engine::native_host::create_app_alive_mutex();
    // BUG-029: a panic on the main thread exits silently in a
    // `windows_subsystem` release build (no console, no WER entry),
    // so record every panic — payload and location — in the startup
    // log. The hook resolves the log path from the environment
    // because a panic can happen before any AppHandle exists, and
    // it never panics itself: all IO is best-effort, and the message
    // also goes to stderr so nothing is lost where a console exists.
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let line = format!("PANIC at {location}: {payload}");
        startup_log_line(&startup_log_path_early(), &line);
        eprintln!("{line}");
    }));
    startup_log_line(&startup_log_path_early(), "main entered");
    match tauri::webview_version() {
        Ok(v) => startup_log_line(&startup_log_path_early(), &format!("webview version: {v}")),
        Err(e) => startup_log_line(
            &startup_log_path_early(),
            &format!("webview version: ERROR {e:?}"),
        ),
    }
    // Round 57 (§9.2c): give the Tauri runtime 4 workers instead
    // of the default 2 on this 2-vCPU VPS. MITIGATION ONLY — it
    // raises the number of simultaneous blocking calls needed to
    // wedge the runtime from 2 to 4; it is not immunity, and the
    // clipboard read (§9.2a) and the checkpoint calls (§9.2b) are
    // moved off the workers anyway. Installed before the Builder
    // creates the runtime. `set` keeps only the handle, so the
    // Runtime itself must be leaked — dropping it would shut the
    // runtime down (see `tauri::async_runtime::set`'s docs).
    // Leaking is the correct lifetime here: it runs until the
    // process exits, like the default runtime Tauri builds.
    match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
    {
        Ok(rt) => {
            tauri::async_runtime::set(rt.handle().clone());
            std::mem::forget(rt);
        }
        Err(e) => startup_log_line(
            &startup_log_path_early(),
            &format!("custom runtime build failed, using default: {e}"),
        ),
    }
    // Round 56 (§7.1b): build the app first, then run it, so a
    // clean shutdown logs its own line. `app.run()` returns only
    // when the app exits — the line after it is the shutdown
    // marker, and a log that ends without it means the process
    // died (panic, kill, crash) rather than exiting cleanly. The
    // heartbeat thread's last beat brackets the lifetime from the
    // other side. A build failure keeps the old behaviour (log +
    // panic); it used to be reported as "run failed" by the
    // Builder::run wrapper, which this split replaces.
    let app = match tauri::Builder::default()
        // Round 67 — the Add URL dialog's "..." browse button
        // needs the native folder picker.
        .plugin(tauri_plugin_dialog::init())
        // Round 74 (9) — single instance: a second launch hands
        // its arguments to the running instance and exits at once
        // (named mutex + WM_COPYDATA on Windows, inside the
        // plugin); the running instance shows + focuses its main
        // window instead of opening a second copy. A second
        // launch never quits the first instance — the tray's
        // Exit is still the way out.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(move |app| {
            let log = startup_log_path_from_app(app.handle());
            let log = log.as_deref();
            log_startup(log, "setup start");
            // Round 75 — hold the app-alive mutex for the process
            // lifetime (see run() above). A crash releases it: the
            // OS destroys a named mutex when its last handle
            // closes, so the popup stops saying "Connected" the
            // moment the app dies.
            if let Some(guard) = app_alive_mutex {
                app.manage(guard);
            }
            // Round 61 (§13 confirm): log the main thread id so it
            // can be compared against the Round 60 "build() entered
            // on thread" lines — setup runs on the main thread.
            log_startup(
                log,
                &format!("main thread id: {:?}", std::thread::current().id()),
            );
            // Round 56 (§7.1b): runtime-independent heartbeat — a
            // plain std::thread (NOT a tokio task) appending a line
            // to startup.log every 30 s, plus one at startup. If
            // the tokio runtime wedges, the heartbeat keeps going
            // (the r55 log went silent with no way to tell whether
            // the process was alive at all); if the heartbeat stops
            // too, the process itself is dead. The thread detaches
            // and ends with the process — there is no clean way to
            // stop it, and none is needed. No log path, no thread:
            // a heartbeat that cannot be written is pointless.
            if let Some(heartbeat_log) = startup_log_path_from_app(app.handle()) {
                std::thread::spawn(move || {
                    startup_log_line(&heartbeat_log, "heartbeat: startup");
                    loop {
                        std::thread::sleep(Duration::from_secs(30));
                        startup_log_line(&heartbeat_log, "heartbeat: alive");
                    }
                });
            }
            let manager = match build_manager(app.handle()) {
                Ok(m) => m,
                Err(e) => {
                    log_startup(log, &format!("build_manager failed: {e}"));
                    return Err(e.into());
                }
            };
            log_startup(log, "manager built");
            // Apply the stored settings (connection count, speed
            // cap, proxy, retries) before any download starts.
            let _ = manager.apply_settings();
            log_startup(log, "settings applied");
            // Round 69 (a): self-register the extension's native
            // messaging host so Chrome/Edge/Firefox can reach the
            // app even when it was never installed (a dev build
            // has no installer to do it). Best-effort — a failure
            // is logged, never fatal.
            #[cfg(target_os = "windows")]
            register_native_host_best_effort(app.handle(), log);
            // Forward engine progress events to the frontend.
            let handle = app.handle().clone();
            let mut rx = manager.subscribe();
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    // S03 progress window: keep an open window's
                    // title live ("<percent>% <filename>") from
                    // Rust — the frontend cannot set its own title
                    // (BUG-044: `plugin:window|set_title` is
                    // denied). A closed window simply has no label
                    // to update; the download itself is untouched.
                    let label = progress_window_label(&event.id);
                    if let Some(window) = handle.get_webview_window(&label) {
                        let _ = window.set_title(&progress_window_title(
                            event.downloaded_bytes,
                            event.size_bytes,
                            &event.filename,
                        ));
                    }
                    let _ = handle.emit("download-progress", event);
                }
            });
            // Forward queue completion actions (the UI shows the
            // countdown dialog for shutdown / hibernate).
            let action_handle = app.handle().clone();
            let mut arx = manager.subscribe_actions();
            tauri::async_runtime::spawn(async move {
                while let Ok(action) = arx.recv().await {
                    let _ = action_handle.emit("queue-action-pending", action);
                }
            });
            // Forward weekly update checks that found a newer
            // version (the UI shows the update dialog for them).
            // The weekly check itself rides on the 30 s scheduler
            // tick in the engine — there is no second scheduler.
            let update_handle = app.handle().clone();
            let mut urx = manager.subscribe_update_checks();
            tauri::async_runtime::spawn(async move {
                while let Ok(result) = urx.recv().await {
                    let _ = update_handle.emit("update-available", result);
                }
            });
            // Scheduler tick every 30 s (architecture.md background job).
            // The browser-extension drain rides on the same tick: a
            // link caught while the app is running is picked up
            // within 30 s, and links caught while it was closed are
            // drained once at startup (below).
            let tick_manager = Arc::clone(&manager);
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                loop {
                    interval.tick().await;
                    let _ = tick_manager.scheduler_tick().await;
                    let _ = tick_manager.drain_pending_links().await;
                }
            });
            // Links the extension caught while the app was closed.
            let drain_manager = Arc::clone(&manager);
            tauri::async_runtime::spawn(async move {
                let _ = drain_manager.drain_pending_links().await;
            });
            // Queues flagged "start on startup" begin draining once.
            let startup_manager = Arc::clone(&manager);
            tauri::async_runtime::spawn(async move {
                let _ = startup_manager.start_startup_queues().await;
            });
            // Clipboard monitoring (Round 52, reworked Round 75):
            // poll the system clipboard once a second and offer a
            // copied download link. The change signal is the OS
            // clipboard sequence number: when it equals the last
            // sequence the engine saw, the clipboard has not
            // changed — the read is skipped entirely (no arboard
            // open, no RDP channel cross). When it changed, the
            // shell reads the text and the engine decides whether
            // it is a download link, honouring the
            // `clipboard_monitoring` setting; the shell only
            // forwards offers. A failed read records nothing, so
            // the next tick retries it. The check is sync and
            // cheap (store lock + tiny work) — nothing is held
            // across an await.
            // Round 74 (12) — TWO repairs to the reader. (1) A
            // FRESH clipboard handle per read: arboard's Windows
            // handle carries no per-handle state, so a fresh one
            // costs nothing — and a read stuck in the RDP
            // clipboard channel can no longer poison the next
            // tick. The old design held ONE shared Clipboard
            // behind a Mutex: one stuck `get_text()` held the
            // lock forever, every later tick skipped on
            // `try_lock`, and monitoring silently died for the
            // rest of the session. A stuck read now only loses
            // its own tick. (2) It logs: the old reader swallowed
            // every failure with `.ok()`, so a dead reader was
            // indistinguishable from "the user copied nothing" —
            // the log now shows the start, the first read
            // failure + the recovery, and every offer emitted.
            // A `Clipboard::new()` that fails once no longer
            // disables monitoring for the session either — the
            // next tick simply tries again.
            let clipboard_handle = app.handle().clone();
            let clipboard_manager = Arc::clone(&manager);
            let clipboard_log = startup_log_path_from_app(app.handle());
            tauri::async_runtime::spawn(async move {
                log_startup(
                    clipboard_log.as_deref(),
                    "clipboard monitoring started (polling once a second)",
                );
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                // A slow clipboard must not burst a backlog of
                // reads when it recovers — one delayed tick, then
                // the next second.
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // Round 74 (12) — read failures log once (the
                // first failure, then the recovery), not every
                // second: a clipboard holding non-text fails
                // every read, and that must not drown the log.
                let mut read_failed_logged = false;
                loop {
                    interval.tick().await;
                    // Round 75 — the sequence gate: an unchanged
                    // clipboard costs no read at all.
                    let sequence = clipboard_sequence_number();
                    if let Some(seq) = sequence {
                        if clipboard_manager.clipboard_sequence_seen(seq) {
                            continue;
                        }
                    }
                    // Round 57 (§9.2a): the clipboard read runs
                    // on a blocking thread, NOT on a tokio worker.
                    // `arboard::Clipboard::get_text()` is a
                    // synchronous call that crosses the RDP
                    // clipboard channel on the VPS — a slow or
                    // stuck channel used to occupy one of the
                    // runtime's 2 workers every second, and two
                    // simultaneous blocking calls freeze the whole
                    // runtime (commands, timers, watchdogs). A
                    // blocking thread can stall without taking a
                    // worker with it; the 1 s tick just skips
                    // that round.
                    let read = tokio::task::spawn_blocking(|| {
                        let mut clip = match arboard::Clipboard::new() {
                            Ok(c) => c,
                            Err(e) => return Err(format!("clipboard open failed: {e}")),
                        };
                        clip.get_text().map_err(|e| e.to_string())
                    })
                    .await;
                    let text = match read {
                        Ok(Ok(t)) => {
                            if read_failed_logged {
                                log_startup(clipboard_log.as_deref(), "clipboard read recovered");
                                read_failed_logged = false;
                            }
                            Some(t)
                        }
                        Ok(Err(e)) => {
                            if !read_failed_logged {
                                log_startup(
                                    clipboard_log.as_deref(),
                                    &format!(
                                        "clipboard read failed ({e}) — monitoring keeps retrying"
                                    ),
                                );
                                read_failed_logged = true;
                            }
                            None
                        }
                        Err(_) => None, // spawn_blocking join failed; next tick retries
                    };
                    let Some(text) = text else {
                        // Read failed: nothing recorded, so the
                        // next tick retries the same sequence.
                        continue;
                    };
                    match clipboard_manager.check_clipboard_offer(&text, sequence) {
                        Ok(Some(offer)) => {
                            log_startup(
                                clipboard_log.as_deref(),
                                &format!("clipboard offer: {}", offer.url),
                            );
                            let _ = clipboard_handle.emit("clipboard-link-found", offer);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log_startup(
                                clipboard_log.as_deref(),
                                &format!("clipboard offer check failed: {e}"),
                            );
                        }
                    }
                }
            });
            log_startup(log, "spawns done");
            // Webview heartbeat state (§7.4, Round 56): created
            // here so the monitor thread and AppState share the
            // same map. The monitor is a plain std::thread
            // (runtime-independent, like the process heartbeat
            // above) that checks every 5 s whether each window's
            // JS is still pinging. A window whose pings stop for
            // 30 s logs "heartbeat lost" — the renderer may be
            // dead even though the window still exists (the r55
            // live test: both windows blank white, no console
            // data, nothing in the log). A recovered window logs
            // "resumed". Each line is once per outage, not every
            // check. No log path, no thread: it only logs.
            let webview_heartbeats: Arc<StdMutex<HashMap<String, WebviewHeartbeat>>> =
                Arc::new(StdMutex::new(HashMap::new()));
            if let Some(monitor_log) = startup_log_path_from_app(app.handle()) {
                let monitor_heartbeats = Arc::clone(&webview_heartbeats);
                let monitor_app = app.handle().clone();
                let monitor_manager = Arc::clone(&manager);
                // Round 60 (§11.4 item 1): keep a copy of the log
                // path — the closure below moves `monitor_log`.
                let monitor_log_start = monitor_log.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let now = epoch_ms();
                    // Round 57 (§9.1b): windows whose heartbeat
                    // just went lost this check — collected under
                    // the lock, reloaded after it is released.
                    // `maybe_auto_reload` takes the same lock for
                    // its cooldown, so calling it while holding
                    // the lock would deadlock. One auto-reload per
                    // outage (the `lost_logged` flag), at most one
                    // per 5 min per window (the cooldown inside
                    // `maybe_auto_reload`).
                    let mut to_reload: Vec<String> = Vec::new();
                    if let Ok(mut map) = monitor_heartbeats.lock() {
                        for (label, hb) in map.iter_mut() {
                            if !hb.started_logged {
                                startup_log_line(
                                    &monitor_log,
                                    &format!("webview {label} heartbeat started"),
                                );
                                hb.started_logged = true;
                            }
                            let silent_ms = now.saturating_sub(hb.last_seen_ms);
                            if silent_ms > 30_000 && !hb.lost_logged {
                                startup_log_line(
                                    &monitor_log,
                                    &format!(
                                        "webview {label} heartbeat lost (last seen {}s ago)",
                                        silent_ms / 1000
                                    ),
                                );
                                hb.lost_logged = true;
                                to_reload.push(label.clone());
                            } else if silent_ms <= 30_000 && hb.lost_logged {
                                startup_log_line(
                                    &monitor_log,
                                    &format!("webview {label} heartbeat resumed"),
                                );
                                hb.lost_logged = false;
                                // Round 58 (§9.2): the outage is
                                // over — the next one gets a fresh
                                // auto-reload budget.
                                hb.auto_reload_attempts = 0;
                            }
                        }
                    }
                    for label in to_reload {
                        let _ = maybe_auto_reload(
                            &monitor_app,
                            &monitor_manager,
                            &label,
                            &monitor_heartbeats,
                        );
                    }
                });
                // Round 60 (§11.4 item 1): prove the monitor thread
                // was spawned — before this, neither the monitor nor
                // the watchdog logged its start, so "did the watchdog
                // run" was unanswerable from the log.
                startup_log_line(&monitor_log_start, "heartbeat monitor started");
            }
            // Round 58 (§9.5a): main-thread watchdog — a plain
            // std::thread that asks the main thread for a trivial
            // window property every 5 s and logs when the main
            // thread stops answering within 10 s. `is_visible()`
            // round-trips the main thread via a user message, so
            // a main thread wedged in window creation (unbounded
            // `wait_with_pump`) shows up here as a timestamped
            // stall instead of silence. Diagnostic only — it
            // cannot un-wedge the main thread. Each check runs
            // the getter on a helper thread: a wedged main thread
            // blocks that helper forever (one leaked thread per
            // stalled check), never the watchdog itself.
            if let Some(watchdog_log) = startup_log_path_from_app(app.handle()) {
                let watchdog_app = app.handle().clone();
                // Round 60 (§11.4 item 1): keep a copy of the log
                // path — the closure below moves `watchdog_log`.
                let watchdog_log_start = watchdog_log.clone();
                std::thread::spawn(move || {
                    let mut stalled = false;
                    let mut stall_since_ms = 0u64;
                    // Round 60 (§11.4 item 1): tick counter for the
                    // periodic liveness proof (see below).
                    let mut tick_count: u64 = 0;
                    loop {
                        std::thread::sleep(Duration::from_secs(5));
                        tick_count += 1;
                        // Round 60 (§11.4 item 1): one `watchdog
                        // tick` line every 60 s (every 12th 5 s
                        // check, starting with the first), so the
                        // log shows the watchdog kept running even
                        // when the main thread never stalls.
                        if tick_count % 12 == 1 {
                            startup_log_line(&watchdog_log, "watchdog tick");
                        }
                        let Some(window) = watchdog_app.get_webview_window("main") else {
                            continue;
                        };
                        let (tx, rx) = std::sync::mpsc::channel();
                        // Round 60 (§11.4 item 3): the helper needs
                        // its own copy of the log path.
                        let probe_log = watchdog_log.clone();
                        std::thread::spawn(move || {
                            // Round 60 (§11.4 item 3): a fast-failing
                            // getter was silent — the one blind spot
                            // in the watchdog. Log it.
                            if let Err(e) = window.is_visible() {
                                startup_log_line(
                                    &probe_log,
                                    &format!("watchdog probe failed: {e}"),
                                );
                            }
                            let _ = tx.send(());
                        });
                        match rx.recv_timeout(Duration::from_secs(10)) {
                            Ok(()) => {
                                if stalled {
                                    let secs = epoch_ms().saturating_sub(stall_since_ms) / 1000;
                                    startup_log_line(
                                        &watchdog_log,
                                        &format!("main thread resumed after {secs}s"),
                                    );
                                    stalled = false;
                                }
                            }
                            Err(_) => {
                                if !stalled {
                                    stall_since_ms = epoch_ms();
                                    startup_log_line(&watchdog_log, "main thread stalled for 10s");
                                    stalled = true;
                                }
                            }
                        }
                    }
                });
                // Round 60 (§11.4 item 1): prove the watchdog thread
                // was spawned — same gap as the monitor (see above).
                startup_log_line(&watchdog_log_start, "main thread watchdog started");
            }
            // Round 57 (§9.1b/c): the main window gets the same
            // treatment as a progress window — a heartbeat entry
            // at startup (so a main window whose JS never runs is
            // watched from the start, not only after its first
            // ping) and a `ProcessFailed` registration. `manager`
            // is still owned here (it moves into AppState below).
            if let Ok(mut map) = webview_heartbeats.lock() {
                let hb = map
                    .entry("main".to_string())
                    .or_insert_with(|| WebviewHeartbeat::new(epoch_ms()));
                // Round 58 (§9.2): the main window already exists
                // (setup runs after it is created), so its
                // creation completed — the monitor may auto-reload
                // it if its renderer dies.
                hb.created_ok = true;
            }
            register_process_failed(app.handle(), "main", &webview_heartbeats, &manager);
            app.manage(AppState {
                manager,
                webview_heartbeats,
                download_completion: Arc::new(StdMutex::new(HashMap::new())),
            });
            // Round 56 (§7.4): log the main window's resolved URL
            // after creation — the same diagnostic as the progress
            // window's URL log, so a wrong URL is visible for the
            // main window too.
            match app.get_webview_window("main") {
                Some(window) => {
                    let url = window
                        .url()
                        .map(|u| u.to_string())
                        .unwrap_or_else(|e| format!("url error: {e}"));
                    log_startup(log, &format!("main window url: {url}"));
                }
                None => log_startup(log, "main window not found in setup"),
            }
            // Round 65 — system tray (best-effort): a tray failure
            // must never fail startup — the app runs fine without
            // it, the windows just cannot hide to the tray.
            match build_system_tray(app.handle()) {
                Ok(()) => log_startup(log, "system tray built"),
                Err(e) => log_startup(log, &format!("system tray unavailable: {e}")),
            }
            // Round 71 (C3) — the main window is built hidden
            // (tauri.conf.json `visible: false`, show-when-ready)
            // so it never shows a blank first paint. The frontend
            // shows it via `window_ready` when React mounts; this
            // fallback guarantees it appears even if that signal
            // never arrives — a main window that stayed hidden
            // would look like the app never started.
            if let Some(main) = app.get_webview_window("main") {
                spawn_show_fallback(&main);
            }
            log_startup(log, "setup ok");
            Ok(())
        })
        .on_window_event(|window, event| {
            // Round 65 — the MAIN window never really closes: its
            // X (and Alt+F4 / taskbar Close) hides it to the tray
            // instead, like IDM. The tray menu's Exit is the way
            // out (`app.exit`, not a window close). Progress
            // windows keep the default close behaviour — their X
            // closes just the window, the download keeps running.
            // Round 86 — MINIMIZE does NOT hide: it is a real
            // minimize to the taskbar (TitleBar.tsx); only X
            // hides to the tray.
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            start_download,
            pause_download,
            resume_download,
            redownload_download,
            refresh_download_address,
            note_own_clipboard_write,
            cancel_download,
            retry_now,
            open_folder,
            open_progress_window,
            open_complete_dialog,
            open_error_dialog,
            open_add_url_window,
            enqueue_clipboard_capture,
            get_pending_link,
            window_ready,
            log_frontend,
            set_download_completion_options,
            get_download_completion_options,
            exit_app,
            shutdown_computer,
            get_free_disk_space,
            get_progress_snapshot,
            webview_heartbeat,
            reload_webview_window,
            run_antivirus_scan,
            get_scan_result,
            test_antivirus_scanner,
            quarantine_download,
            open_file,
            open_file_with,
            delete_download,
            list_downloads,
            list_download_history,
            clear_download_history,
            redownload_from_history,
            open_history_folder,
            get_categories,
            create_category,
            delete_category,
            move_to_category,
            create_queue,
            delete_queue,
            rename_queue,
            update_queue,
            list_queues,
            get_queue,
            get_queue_files,
            add_to_queue,
            remove_from_queue,
            reorder_queue,
            start_queue,
            stop_queue,
            sync_queue_now,
            execute_queue_action,
            dismiss_queue_action,
            get_pending_action,
            fetch_page_links,
            add_batch,
            download_all_links,
            detect_page_videos,
            drain_pending_links,
            create_grabber_project,
            run_grabber,
            stop_grabber,
            get_grabber_project,
            list_grabber_projects,
            rename_grabber_project,
            delete_grabber_project,
            get_settings,
            update_settings,
            check_for_updates,
            set_speed_limit,
            get_speed_limit,
            set_download_limit,
            get_quota_status,
            reset_quota,
            set_download_speed_limit,
            get_proxy_config,
            save_proxy_config,
            test_proxy,
            list_site_logins,
            save_site_login,
            delete_site_login,
            get_sound_settings,
            save_sound_settings,
            test_sound_file,
            play_completion_sound,
            play_error_sound,
            set_taskbar_progress,
            update_category,
            list_pending_links,
            resolve_pending_link,
            probe_url,
            ytdlp_version,
            ytdlp_resolve,
            ytdlp_update,
            add_resolved_video_download,
            set_download_description,
            confirm_pending_link,
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(e) => {
            startup_log_line(&startup_log_path_early(), &format!("build failed: {e}"));
            panic!("error while building tauri application");
        }
    };
    // BUG-029: `app.run()` only returns when the app exits. The
    // line after it is the clean-shutdown marker (Round 56,
    // §7.1b) — it replaces the old "run returned ok" line.
    app.run(|_app_handle, _event| {});
    startup_log_line(&startup_log_path_early(), "clean shutdown");
}
