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

use std::sync::Arc;
use std::time::Duration;

use fetchline_engine::{
    BatchResult, CategoryRow, DetectedVideo, DownloadManager, DownloadRow, DrainSummary,
    GrabberProjectRow, PageLink, PendingAction, ProbeInfo, ProxyConfigInput, ProxyConfigRow,
    QueueRow, QuotaStatus, RefreshAddressResult, ScanResult, Settings, SiteLoginInput,
    SiteLoginRow, SoundSetting, SoundSettingInput, Store, SyncResult, UpdateCheckResult,
};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Size, State, WebviewUrl, WebviewWindowBuilder};

/// Shared state: one manager for the whole app lifetime.
pub struct AppState {
    pub manager: Arc<DownloadManager>,
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
    // 8 connections is the schema default; the stored settings are
    // applied in `setup` (apply_settings), which also wires the
    // saved proxy, speed limit and retry count.
    Ok(DownloadManager::new(Arc::new(store), 8))
}

#[tauri::command]
async fn add_download(
    state: State<'_, AppState>,
    url: String,
    filename: Option<String>,
    folder: Option<String>,
    category: Option<String>,
    queue: Option<String>,
) -> Result<(String, ProbeInfo), String> {
    state
        .manager
        .add_download(&url, filename, folder, category, queue)
        .await
        .map_err(|e| e.to_string())
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
// Rust, not from the frontend: this app ships no Tauri capability
// files, so the frontend `plugin:webview|create_webview_window`
// command is denied by the ACL (BUG-044) while app commands like
// these run ungated (local origin, no app ACL manifest — see
// `tauri::webview::on_message` in vendored tauri 2.12.5). The
// window's own title is likewise updated from Rust, in the
// progress-event forwarder below — the frontend `set_title`
// command is denied for the same reason. Closing the window never
// cancels the download: the engine keeps running and the window
// is only a view of it; double-clicking the row reopens it.
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

/// Open (or focus, when it already exists) the separate progress
/// window for a download. The window loads the same frontend
/// bundle with `#progress=<id>` in the URL, so `main.tsx` renders
/// the progress view instead of the main window. Errors when the
/// download does not exist.
#[tauri::command]
fn open_progress_window(
    app: AppHandle,
    state: State<'_, AppState>,
    download_id: String,
) -> Result<(), String> {
    let row = state
        .manager
        .get_download(&download_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Download not found: {download_id}"))?;
    let label = progress_window_label(&download_id);
    if let Some(window) = app.get_webview_window(&label) {
        // Already open — bring it forward instead of a second copy.
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let url = WebviewUrl::App(format!("index.html#progress={download_id}").into());
    WebviewWindowBuilder::new(&app, &label, url)
        .title(progress_window_title(
            row.downloaded_bytes as u64,
            row.size_bytes.map(|s| s as u64),
            &row.filename,
        ))
        .inner_size(600.0, 680.0)
        .min_inner_size(520.0, 320.0)
        .resizable(true)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
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

#[tauri::command]
async fn list_downloads(state: State<'_, AppState>) -> Result<Vec<DownloadRow>, String> {
    state.manager.list_downloads().map_err(|e| e.to_string())
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
    let result = tauri::Builder::default()
        .setup(|app| {
            let log = startup_log_path_from_app(app.handle());
            let log = log.as_deref();
            log_startup(log, "setup start");
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
            log_startup(log, "spawns done");
            app.manage(AppState { manager });
            log_startup(log, "setup ok");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            start_download,
            pause_download,
            resume_download,
            refresh_download_address,
            cancel_download,
            retry_now,
            open_folder,
            open_progress_window,
            set_progress_window_compact,
            get_progress_snapshot,
            run_antivirus_scan,
            get_scan_result,
            test_antivirus_scanner,
            quarantine_download,
            open_file,
            delete_download,
            list_downloads,
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
        .run(tauri::generate_context!());
    // BUG-029: `.run()` only returns when the app exits. Record the
    // outcome first — a run error used to die in `.expect` with
    // nothing on disk — then keep the exact same panic behaviour.
    match result {
        Ok(()) => startup_log_line(&startup_log_path_early(), "run returned ok"),
        Err(e) => {
            startup_log_line(
                &startup_log_path_early(),
                &format!("run failed: {e}"),
            );
            panic!("error while running tauri application");
        }
    }
}
