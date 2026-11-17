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
    GrabberProjectRow, HistoryRow, PageLink, PendingAction, ProbeInfo, ProxyConfigInput,
    ProxyConfigRow, QueueRow, QuotaStatus, RefreshAddressResult, ScanResult, Settings,
    SiteLoginInput, SiteLoginRow, SoundSetting, SoundSettingInput, Store, SyncResult,
    UpdateCheckResult,
};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Size, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

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
}

impl WebviewHeartbeat {
    fn new(now_ms: u64) -> Self {
        Self {
            last_seen_ms: now_ms,
            lost_logged: false,
            started_logged: false,
            last_auto_reload_ms: 0,
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
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
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
    state.manager.start_download(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn pause_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.pause_download(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn resume_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.resume_download(&id).await.map_err(|e| e.to_string())
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

/// Clipboard monitoring (Round 52): the user dismissed the
/// "Download this link?" offer for this URL — the engine will
/// not offer it again. The engine method is infallible and
/// idempotent (it just records the URL in session state).
#[tauri::command]
fn dismiss_clipboard_offer(state: State<'_, AppState>, url: String) -> Result<(), String> {
    state.manager.dismiss_clipboard_offer(&url);
    Ok(())
}

#[tauri::command]
async fn cancel_download(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.cancel_download(&id).await.map_err(|e| e.to_string())
}

/// Skip the backoff of a download waiting for a retry (S03,
/// "Retry now"). The engine refuses when nothing is waiting.
#[tauri::command]
async fn retry_now(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.retry_now(&id).await.map_err(|e| e.to_string())
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
            let percent = downloaded_bytes.saturating_mul(100).saturating_div(size).min(100);
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
    let log = startup_log_path_from_app(app);
    log_startup(
        log.as_deref(),
        &format!("progress window {label} closed for reload"),
    );
    build_progress_window(app, manager, download_id, heartbeats)?;
    Ok(())
}

/// Maybe auto-reload a window whose renderer is dead (§9.1b,
/// Round 57): the heartbeat monitor and the `ProcessFailed`
/// handler call this, never the banner. At most one auto-reload
/// per 5 min per window (`AUTO_RELOAD_COOLDOWN_MS`), so a wedged
/// runtime cannot cause a reload loop. Returns `Ok(true)` when a
/// reload actually ran. Every decision is logged — a skipped
/// reload is as diagnostic as a run one. Never restarts the
/// process (auto-restart is a pending user decision, not
/// implemented here).
fn maybe_auto_reload(
    app: &AppHandle,
    manager: &Arc<DownloadManager>,
    label: &str,
    heartbeats: &Arc<StdMutex<HashMap<String, WebviewHeartbeat>>>,
) -> Result<bool, String> {
    let now = epoch_ms();
    let log = startup_log_path_from_app(app);
    let log = log.as_deref();
    {
        let Ok(mut map) = heartbeats.lock() else {
            return Ok(false);
        };
        let hb = map
            .entry(label.to_string())
            .or_insert_with(|| WebviewHeartbeat::new(now));
        if now.saturating_sub(hb.last_auto_reload_ms) < AUTO_RELOAD_COOLDOWN_MS {
            log_startup(
                log,
                &format!("webview {label} auto-reload skipped (cooldown)"),
            );
            return Ok(false);
        }
        hb.last_auto_reload_ms = now;
        // The lock is released before the reload runs: the reload
        // itself registers a new heartbeat entry (progress windows).
    }
    log_startup(
        log,
        &format!("webview {label} auto-reloading (renderer dead)"),
    );
    match reload_webview_now(app, manager, label, heartbeats) {
        Ok(()) => {
            log_startup(log, &format!("webview {label} auto-reload done"));
            Ok(true)
        }
        Err(e) => {
            log_startup(log, &format!("webview {label} auto-reload FAILED: {e}"));
            Err(e)
        }
    }
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

/// Build one progress window for a download. The window loads the
/// same frontend bundle, so `main.tsx` renders the progress view
/// instead of the main window. Shared by `open_progress_window`
/// and by `reload_webview_window` (which closes a dead window and
/// builds a fresh one — §7.4, Round 56).
/// Round 56 (§7.1f): the log line now goes out BEFORE `build()`
/// (the r55 log had no creation line at all, so a live test could
/// not tell whether the call was even reached), and the window's
/// resolved URL is logged after creation — a URL that is wrong
/// here is a different bug from a webview that never paints.
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
    let url = WebviewUrl::App("index.html".into());
    let init_script = format!("window.__PROGRESS_ID__ = {download_id:?};");
    log_startup(
        log,
        &format!("creating progress window {label}: index.html (id via init script)"),
    );
    match WebviewWindowBuilder::new(app, &label, url)
        .initialization_script(init_script)
        .title(progress_window_title(
            row.downloaded_bytes as u64,
            row.size_bytes.map(|s| s as u64),
            &row.filename,
        ))
        .inner_size(600.0, 680.0)
        .min_inner_size(520.0, 320.0)
        .resizable(true)
        .center()
        .devtools(true)
        .build()
    {
        Ok(window) => {
            let url = window
                .url()
                .map(|u| u.to_string())
                .unwrap_or_else(|e| format!("url error: {e}"));
            log_startup(log, &format!("progress window {label} created: {url}"));
            // Register the heartbeat entry at creation (§9.1b): a
            // window whose JS never runs at all (renderer dead
            // from the start) would otherwise never ping, so the
            // monitor would have nothing to watch and never log
            // "lost". The first ping updates the stamp; the entry
            // here only starts the clock.
            if let Ok(mut map) = heartbeats.lock() {
                map.entry(label.clone())
                    .or_insert_with(|| WebviewHeartbeat::new(epoch_ms()));
            }
            register_process_failed(app, &label, heartbeats, manager);
            Ok(window)
        }
        Err(e) => {
            log_startup(
                log,
                &format!("progress window {label} FAILED to create: {e}"),
            );
            Err(e.to_string())
        }
    }
}

/// Open (or focus, when it already exists) the separate progress
/// window for a download. Errors when the download does not exist.
#[tauri::command]
fn open_progress_window(
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
    let log = startup_log_path_from_app(&app);
    let log = log.as_deref();
    let label = progress_window_label(&download_id);
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second copy.
        window.set_focus().map_err(|e| e.to_string())?;
        log_startup(log, &format!("progress window {label} already open — focused"));
        return Ok(());
    }
    build_progress_window(&app, &state.manager, &download_id, &state.webview_heartbeats)?;
    Ok(())
}

/// Resize a progress window between its full view (speed graph,
/// parts, scan details) and its compact view (file, bar, speed,
/// buttons). Called by the window's own "Hide details" toggle.
#[tauri::command]
fn set_progress_window_compact(
    app: AppHandle,
    download_id: String,
    compact: bool,
) -> Result<(), String> {
    let label = progress_window_label(&download_id);
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("No progress window open for download {download_id}"))?;
    let (width, height) = if compact { (600.0, 300.0) } else { (600.0, 680.0) };
    window
        .set_size(Size::Logical(LogicalSize::new(width, height)))
        .map_err(|e| e.to_string())
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
fn reload_webview_window(
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
    state.manager.get_scan_result(&id).map_err(|e| e.to_string())
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
    state.manager.quarantine_download(&id).map_err(|e| e.to_string())
}

/// Antivirus hook (S14): open a finished download in its default
/// application. Refused when the scanner flagged the file as a
/// threat, so a threat is never opened from the dialog.
#[tauri::command]
fn open_file(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.open_file(&id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_download(state: State<'_, AppState>, id: String, delete_file: bool) -> Result<(), String> {
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
async fn list_downloads(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<DownloadRow>, String> {
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
        trace_command_exit(
            log.as_deref(),
            "list_downloads",
            n,
            started,
            result.is_ok(),
        );
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
    state.manager.list_history(&query).map_err(|e| e.to_string())
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
    state.manager.open_history_folder(&id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_categories(state: State<'_, AppState>) -> Result<Vec<CategoryRow>, String> {
    // Architecture.md names this command `get_categories`; it is the
    // category list (there is no separate `list_categories`).
    state.manager.store().list_categories().map_err(|e| e.to_string())
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
    state.manager.delete_category(&id).map_err(|e| e.to_string())
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
    state.manager.run_sync_queue(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_queues(state: State<'_, AppState>) -> Result<Vec<QueueRow>, String> {
    state.manager.list_queues().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_queue(state: State<'_, AppState>, id: String) -> Result<Option<QueueRow>, String> {
    state.manager.get_queue(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_queue_files(state: State<'_, AppState>, queue_id: String) -> Result<Vec<DownloadRow>, String> {
    state.manager.list_queue_files(&queue_id).map_err(|e| e.to_string())
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
    state.manager.remove_from_queue(&download_id).map_err(|e| e.to_string())
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
    state.manager.start_queue(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_queue(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.stop_queue(&id).await.map_err(|e| e.to_string())
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
    state.manager.fetch_page_links(&page_url).await.map_err(|e| e.to_string())
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
async fn update_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    state.manager.update_settings(&settings).map_err(|e| e.to_string())
}

/// Change only the global speed cap (architecture.md
/// `set_speed_limit`). `None` (or 0) means unlimited.
#[tauri::command]
async fn set_speed_limit(
    state: State<'_, AppState>,
    kbps: Option<i64>,
) -> Result<Settings, String> {
    state.manager.set_speed_limit(kbps).map_err(|e| e.to_string())
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
    state.manager.set_download_limit(mb, hours).map_err(|e| e.to_string())
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
    state.manager.set_download_speed_limit(&id, kbps).map_err(|e| e.to_string())
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
    state.manager.save_proxy_config(&input).map_err(|e| e.to_string())
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
    state.manager.test_proxy(&input, &test_url).await.map_err(|e| e.to_string())
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
    state.manager.save_site_login(&input).map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_site_login(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.delete_site_login(&id).map_err(|e| e.to_string())
}

/// Sounds (S09 Sounds tab): one setting per event. The engine
/// emits the event and plays the WAV on Windows only.
#[tauri::command]
async fn get_sound_settings(state: State<'_, AppState>) -> Result<Vec<SoundSetting>, String> {
    state.manager.get_sound_settings().map_err(|e| e.to_string())
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
    state.manager.test_sound_file(&path).map_err(|e| e.to_string())
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
    state.manager.check_for_updates().await.map_err(|e| e.to_string())
}

/// Drain the browser-extension pending-links queue (S13): every
/// link the native host enqueued is put through the real
/// `add_download` path. Milestone 2: the drain STARTS each created
/// download at once when `settings.browser_start_drained` is on
/// (the default); with it off the downloads are left `queued`.
#[tauri::command]
async fn drain_pending_links(state: State<'_, AppState>) -> Result<DrainSummary, String> {
    state
        .manager
        .drain_pending_links()
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
    state.manager.run_grabber(&id).await.map_err(|e| e.to_string())
}

/// Ask a running grabber to stop after the current page / file.
#[tauri::command]
async fn stop_grabber(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.manager.stop_grabber(&id).await.map_err(|e| e.to_string())
}

/// One grabber project with its live progress and matched files.
#[tauri::command]
async fn get_grabber_project(state: State<'_, AppState>, id: String) -> Result<GrabberProjectRow, String> {
    state
        .manager
        .get_grabber_project(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Site grabber project not found: {id}"))
}

/// All grabber projects, newest first.
#[tauri::command]
async fn list_grabber_projects(state: State<'_, AppState>) -> Result<Vec<GrabberProjectRow>, String> {
    state.manager.list_grabber_projects().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
        Ok(v) => startup_log_line(
            &startup_log_path_early(),
            &format!("webview version: {v}"),
        ),
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
        .setup(|app| {
            let log = startup_log_path_from_app(app.handle());
            let log = log.as_deref();
            log_startup(log, "setup start");
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
            // Clipboard monitoring (Round 52): poll the system
            // clipboard once a second and offer a copied download
            // link. The engine decides whether the text is a new
            // downloadable link (it dedupes against dismissed
            // offers and existing downloads) and honours the
            // `clipboard_monitoring` setting; the shell only reads
            // the clipboard and forwards offers. The check is sync
            // and cheap (store lock + tiny work) — nothing is held
            // across an await. A clipboard that cannot be opened
            // disables monitoring for the session, never panics.
            let clipboard_handle = app.handle().clone();
            let clipboard_manager = Arc::clone(&manager);
            let clipboard_log = startup_log_path_from_app(app.handle());
            tauri::async_runtime::spawn(async move {
                let clipboard = match arboard::Clipboard::new() {
                    Ok(c) => Arc::new(StdMutex::new(c)),
                    Err(e) => {
                        log_startup(
                            clipboard_log.as_deref(),
                            &format!("clipboard unavailable: {e} — monitoring disabled"),
                        );
                        return;
                    }
                };
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                // A slow clipboard must not burst a backlog of
                // reads when it recovers — one delayed tick, then
                // the next second.
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    interval.tick().await;
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
                    // that round. `try_lock`: a previous read
                    // still stuck in the channel means this tick
                    // is skipped instead of queuing behind it.
                    let clipboard_clone = Arc::clone(&clipboard);
                    let text = tokio::task::spawn_blocking(move || {
                        let Ok(mut clip) = clipboard_clone.try_lock() else {
                            return None;
                        };
                        clip.get_text().ok()
                    })
                    .await
                    .unwrap_or(None);
                    let Some(text) = text else {
                        continue;
                    };
                    if let Ok(Some(offer)) = clipboard_manager.check_clipboard_offer(&text) {
                        let _ = clipboard_handle.emit("clipboard-link-found", offer);
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
            }
            // Round 57 (§9.1b/c): the main window gets the same
            // treatment as a progress window — a heartbeat entry
            // at startup (so a main window whose JS never runs is
            // watched from the start, not only after its first
            // ping) and a `ProcessFailed` registration. `manager`
            // is still owned here (it moves into AppState below).
            if let Ok(mut map) = webview_heartbeats.lock() {
                map.entry("main".to_string())
                    .or_insert_with(|| WebviewHeartbeat::new(epoch_ms()));
            }
            register_process_failed(app.handle(), "main", &webview_heartbeats, &manager);
            app.manage(AppState {
                manager,
                webview_heartbeats,
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
            log_startup(log, "setup ok");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            start_download,
            pause_download,
            resume_download,
            refresh_download_address,
            dismiss_clipboard_offer,
            cancel_download,
            retry_now,
            open_folder,
            open_progress_window,
            set_progress_window_compact,
            get_progress_snapshot,
            webview_heartbeat,
            reload_webview_window,
            run_antivirus_scan,
            get_scan_result,
            test_antivirus_scanner,
            quarantine_download,
            open_file,
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
            update_category
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
