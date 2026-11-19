//! SQLite persistence, using the schema in `replica/schema.sql`.
//!
//! One file, one local user, WAL mode. All writes go through a single
//! `Mutex<Connection>` so the engine, the Tauri commands and the checkpoint
//! timer can never interleave a transaction.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{EngineError, Result};
use crate::grabber::{GrabberFilters, GrabberProjectRow, MatchedFile};
use crate::options::{
    clean_site_host, validate_site_login_input, validate_sound_setting, SiteLoginInput,
    SiteLoginRow, SoundSetting, SoundSettingInput, SOUND_EVENTS,
};
use crate::proxy::{
    resolve_proxy_auth, validate_proxy_input, ProxyConfig, ProxyConfigInput, ProxyConfigRow,
};
use crate::secret::{protect_secret, unprotect_secret};
use crate::timeutil::parse_iso_to_unix;

/// The schema is the single source of truth — the same file the architect
/// wrote. It is embedded at compile time so the app can never drift from it.
const SCHEMA_SQL: &str = include_str!("../../../replica/schema.sql");

/// Maximum speed limit the engine accepts, in KB/s (10 GB/s —
/// beyond any real link). Larger values are rejected, not clamped:
/// converting them to bytes/s would overflow (BUG-008), and silently
/// clamping would hide a typo. 10_000_000 KB/s itself is accepted.
pub const MAX_SPEED_LIMIT_KBPS: i64 = 10_000_000;

/// Periodic synchronisation (S05/F04): the shortest gap between
/// sync runs, in minutes. Anything smaller would hammer the server
/// with HEAD probes every tick, so it is rejected, not clamped.
/// The longest gap is a week — a larger value is almost certainly
/// a typo (e.g. minutes typed where hours were meant).
pub const MIN_SYNC_INTERVAL_MINUTES: i64 = 5;
pub const MAX_SYNC_INTERVAL_MINUTES: i64 = 10_080;

/// A schedule time as sent by a caller: blank or whitespace-only
/// strings clear the field, anything else is kept (and validated by
/// the caller).
fn clean_time(v: Option<&str>) -> Option<&str> {
    match v.map(str::trim) {
        Some(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Clean an extension list: lowercase, no leading dots, blanks and
/// duplicates dropped.
pub fn clean_extensions(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in list {
        let e = e.trim().trim_start_matches('.').to_ascii_lowercase();
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Clean a host list: lowercase, no scheme, no path, blanks and
/// duplicates dropped. "https://Foo.org/some/path" becomes "foo.org".
pub fn clean_excluded_sites(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        let mut h = raw.trim().to_ascii_lowercase();
        if let Some(rest) = h.strip_prefix("https://") {
            h = rest.to_string();
        } else if let Some(rest) = h.strip_prefix("http://") {
            h = rest.to_string();
        }
        if let Some(slash) = h.find('/') {
            h.truncate(slash);
        }
        let h = h.trim_end_matches('.').to_string();
        if !h.is_empty() && !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// Does `host` match an exclusion entry — the entry itself or any of
/// its subdomains ("example.com" also excludes "sub.example.com").
pub fn site_is_excluded(host: &str, excluded: &[String]) -> bool {
    let host = host.trim().to_ascii_lowercase();
    excluded.iter().any(|e| {
        let e = e.trim().to_ascii_lowercase();
        !e.is_empty() && (host == e || host.ends_with(&format!(".{e}")))
    })
}

/// App settings (S09), singleton row id = 1. `speed_limit_kbps` is
/// `None` for unlimited; `default_action` is what a new download does
/// by default ("start_now" | "save_for_later"); `duplicate_action`
/// is what happens when the file already exists ("rename" |
/// "overwrite"). `start_on_startup` is a stored flag only — actually
/// registering the app in Windows autostart is out of scope.
/// `browser_start_drained` (Milestone 2 / S13 finish) is what the
/// drain does with a link the browser caught: true (default) starts
/// it downloading at once; false leaves it queued, the old behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub default_download_folder: String,
    pub max_connections: i64,
    pub speed_limit_kbps: Option<i64>,
    pub language: String,
    pub theme: String,
    pub browser_integration: bool,
    /// Clipboard monitoring (Round 52): when on, copying a download
    /// link offers to download it (default on). Off means the
    /// clipboard is never even looked at.
    pub clipboard_monitoring: bool,
    pub browser_start_drained: bool,
    /// Progress window (S03, 2026-11-15): when a download starts,
    /// a separate progress window opens for it (default on). Off
    /// leaves the table as the only progress view; double-clicking
    /// a row still opens its window.
    pub show_progress_window: bool,
    /// "Download complete" dialog (Round 66): when a download
    /// finishes, its progress window closes and a separate
    /// dialog shows the file, size, URL and save path (default
    /// on). Off means no dialog — the progress window stays open
    /// with its Open file / Open folder footer instead. The
    /// dialog's own "Don't show this dialog again" checkbox
    /// turns this off; Settings turns it back on.
    pub show_download_complete_dialog: bool,
    /// Notification sounds (Round 68): the master on/off switch
    /// for the completion sound (tada) and the error alert
    /// (default on). Off silences both the built-in sounds and
    /// any user-configured WAVs — the engine's `emit_sound`
    /// checks it too, so a configured file stays silent as well.
    pub sounds_enabled: bool,
    pub antivirus_command: Option<String>,
    /// Antivirus hook (S14): the on/off switch (default on — an
    /// empty command still means no scan) and the scan timeout in
    /// seconds (5–600, default 60). A scan running longer is
    /// killed and recorded as timed out.
    pub antivirus_enabled: bool,
    pub antivirus_timeout_secs: i64,
    pub file_types_auto: Vec<String>,
    pub excluded_sites: Vec<String>,
    pub check_updates_weekly: bool,
    /// Update check (S09 / should-have): the release server the
    /// check fetches its version manifest from. Fetchline's own
    /// server, configurable — the default is a placeholder that
    /// must be replaced with the real release server before
    /// shipping (see `update.rs`). Never IDM's servers or any
    /// third-party feed.
    pub update_url: String,
    /// When the last update check ran (UTC ISO-8601), success or
    /// failure — the weekly timer reads this to decide whether a
    /// check is due. `None` means never checked.
    pub last_update_check_at: Option<String>,
    pub start_on_startup: bool,
    pub default_action: String,
    pub duplicate_action: String,
    pub confirm_delete: bool,
    pub connection_timeout_secs: i64,
    pub max_retries: i64,
    /// Patient tail (Round 73): minutes of zero byte progress
    /// after which a patient tail gives up instead of retrying
    /// forever on a dead server (1–120, default 10). The run
    /// watchdogs are suppressed while a tail waits, so without
    /// this cap a server that delivered 92% and then died would
    /// spin forever. A tail that IS receiving bytes never trips
    /// it — the clock runs from the last byte, not from the
    /// tail's start.
    #[serde(default = "default_tail_zero_progress_cap_minutes")]
    pub tail_zero_progress_cap_minutes: i64,
    /// Dial-Up tab (S09 finish): legacy settings. The connection
    /// name may be empty; auto-dial and redial are honoured only
    /// where a dial-up connection exists (Windows RAS); hang-up
    /// after completion maps onto the on-completion action path.
    /// Redial attempts are dial-up connection attempts, never
    /// download retries.
    pub dialup_connection_name: Option<String>,
    pub dialup_auto_dial: bool,
    pub dialup_hangup_after_completion: bool,
    pub dialup_redial_attempts: i64,
    /// Download quota (S12): `quota_enabled` is the on/off switch;
    /// `quota_mb` is the limit in MB every `quota_hours` hours
    /// (`None` = no quota configured). The live window (start +
    /// bytes used) is NOT stored here — it lives in the
    /// `quota_state` table, so saving settings never resets it.
    pub quota_enabled: bool,
    pub quota_mb: Option<i64>,
    pub quota_hours: Option<i64>,
}

/// Round 73 — default for `Settings::tail_zero_progress_cap_minutes`.
/// Also the serde default, so a settings payload from an older UI
/// (which never sends the field) still deserialises instead of
/// failing the whole save.
pub const DEFAULT_TAIL_ZERO_PROGRESS_CAP_MINUTES: i64 = 10;

fn default_tail_zero_progress_cap_minutes() -> i64 {
    DEFAULT_TAIL_ZERO_PROGRESS_CAP_MINUTES
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_download_folder: String::new(),
            max_connections: 8,
            speed_limit_kbps: None,
            language: "en".to_string(),
            theme: "dark".to_string(),
            browser_integration: true,
            clipboard_monitoring: true,
            browser_start_drained: true,
            show_progress_window: true,
            show_download_complete_dialog: true,
            sounds_enabled: true,
            antivirus_command: None,
            antivirus_enabled: true,
            antivirus_timeout_secs: crate::antivirus::DEFAULT_SCAN_TIMEOUT_SECS,
            file_types_auto: Vec::new(),
            excluded_sites: Vec::new(),
            check_updates_weekly: true,
            update_url: crate::update::DEFAULT_UPDATE_URL.to_string(),
            last_update_check_at: None,
            start_on_startup: false,
            default_action: "start_now".to_string(),
            duplicate_action: "rename".to_string(),
            confirm_delete: true,
            connection_timeout_secs: 30,
            max_retries: 5,
            tail_zero_progress_cap_minutes: DEFAULT_TAIL_ZERO_PROGRESS_CAP_MINUTES,
            dialup_connection_name: None,
            dialup_auto_dial: false,
            dialup_hangup_after_completion: false,
            dialup_redial_attempts: 0,
            quota_enabled: false,
            quota_mb: None,
            quota_hours: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingLinkRow {
    pub id: String,
    pub url: String,
    pub referrer: Option<String>,
    pub source: String,
    pub status: String,
    pub download_id: Option<String>,
    pub error: Option<String>,
    /// The browser's cookies for this URL (S13 finish), as a JSON
    /// array of name/value pairs — or `None` when the hand-off
    /// carried none. Held only while the row is pending; the drain
    /// wipes it when it decides the link.
    pub cookies: Option<String>,
    pub created_at: String,
}

/// One finished download in the history (Round 53): recorded
/// when a download completes and kept even when the download is
/// deleted from the main table — `download_id` is deliberately
/// NOT a foreign key (see schema.sql). `folder` is the parent
/// directory of the finished file, so "Open folder" and
/// "Re-download" both work from a history row alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRow {
    pub id: String,
    pub download_id: String,
    pub filename: String,
    pub url: String,
    pub final_url: Option<String>,
    pub size_bytes: Option<i64>,
    pub folder: String,
    pub category_id: Option<String>,
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRow {
    pub id: String,
    pub url: String,
    pub final_url: Option<String>,
    pub filename: String,
    pub save_path: String,
    pub category_id: String,
    pub queue_id: Option<String>,
    pub queue_position: i64,
    pub size_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub status: String,
    pub speed_bps: i64,
    pub eta_seconds: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    /// Last attempt / last activity (UTC ISO-8601): the row's
    /// `updated_at`, bumped on every status change (start, pause,
    /// resume, error, complete) and on the 2 s checkpoint while
    /// downloading — the engine's record of the last try, shown
    /// as the table's "Last Try" column (Round 44).
    #[serde(default)]
    pub updated_at: String,
    /// Site-grabber project that produced this download, if any
    /// (Round 44: the categories tree filters by project).
    #[serde(default)]
    pub grabber_project_id: Option<String>,
    /// Per-download speed cap in KB/s (S12); `None` = no
    /// per-download cap (the global cap still applies).
    /// `#[serde(default)]` so rows written before S12 still read.
    #[serde(default)]
    pub speed_limit_kbps: Option<i64>,
    /// Paused because the download quota was reached (S12); only
    /// these downloads are auto-resumed on rollover / reset.
    #[serde(default)]
    pub quota_paused: bool,
    /// Antivirus scan result (S14): `clean` | `infected` |
    /// `scan_failed` | `not_configured` | `timed_out` |
    /// `scanning`, or `None` when the download was never scanned.
    /// `scan_detail` is the scanner's own report (or the reason
    /// the scan failed / was skipped); `scan_scanned_at` is when
    /// it finished (UTC ISO-8601, `None` while scanning).
    #[serde(default)]
    pub scan_status: Option<String>,
    #[serde(default)]
    pub scan_detail: Option<String>,
    #[serde(default)]
    pub scan_scanned_at: Option<String>,
    /// Whether the probe found the server range-capable (S03
    /// progress window): `Some(true)` = resumable, `Some(false)` =
    /// no ranges, `None` = unknown (a download added before this
    /// was stored). `#[serde(default)]` so rows written before
    /// S03 still read.
    #[serde(default)]
    pub accept_ranges: Option<bool>,
    /// Free-text note from the Add URL dialog's Description field
    /// (Round 67, IDM's Download File Info has one). `#[serde(default)]`
    /// so rows written before Round 67 still read.
    #[serde(default)]
    pub description: Option<String>,
    /// Round 72 (D) — live connection/part counts, written by
    /// the ~1 s checkpoint while a download runs: how many
    /// connections are transferring right now, and how many parts
    /// the file is split into. 0 when not downloading.
    /// `#[serde(default)]` so rows written before Round 72 still
    /// read.
    #[serde(default)]
    pub active_connections: i64,
    #[serde(default)]
    pub segment_count: i64,
    /// Round 71 (D4) — NOT a column: computed on read (see
    /// `mark_missing`): a Completed download whose file no
    /// longer exists on disk (deleted outside the app — the
    /// Round 70 retest's test files were deleted to free space
    /// while their rows still said Finished). The UI shows it
    /// instead of pretending the file is there to open.
    #[serde(default)]
    pub file_missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRow {
    pub id: String,
    pub download_id: String,
    pub segment_index: i64,
    pub start_byte: i64,
    pub end_byte: Option<i64>,
    pub downloaded_bytes: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryRow {
    pub id: String,
    pub name: String,
    pub default_folder: String,
    pub file_extensions: Vec<String>,
    pub is_builtin: bool,
}

/// One queue, as the UI sees it. `running` and `file_count` are not
/// stored — the store fills them in (running is always false here; the
/// manager overrides it from its live queue runs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRow {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub queue_type: String,
    pub is_main: bool,
    pub schedule_type: String,
    pub start_at: Option<String>,
    pub stop_at: Option<String>,
    pub start_on_startup: bool,
    pub on_complete_action: String,
    pub on_complete_program: Option<String>,
    /// Periodic synchronisation (S05/F04): minutes between sync
    /// runs (`None` unless `schedule_type` is `periodic`), and the
    /// last sync's time + per-outcome counts (0 when never synced).
    #[serde(default)]
    pub sync_interval_minutes: Option<i64>,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    #[serde(default)]
    pub last_sync_changed: i64,
    #[serde(default)]
    pub last_sync_unchanged: i64,
    #[serde(default)]
    pub last_sync_removed: i64,
    #[serde(default)]
    pub last_sync_failed: i64,
    #[serde(default)]
    pub last_sync_skipped: i64,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub file_count: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
    /// Optional diagnostics sink (set by the shell): a file path
    /// that receives `[epoch-seconds] message` lines, in the same
    /// format as the shell's `startup.log`. `None` (the default,
    /// and every test that never sets it) means no logging.
    /// The shell points it at `startup.log` next to the database,
    /// so engine diagnostics land in the same file the live tests
    /// already read. Best-effort: a failed write is swallowed,
    /// never an error for the caller.
    log_path: Mutex<Option<PathBuf>>,
}

impl Store {
    /// Open (creating) the database at `path` and apply the schema.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // BUG-041: two connections opening the same fresh database
        // at the same time raced inside execute_batch below (the
        // schema WRITES: CREATE TABLE + INSERT OR IGNORE), and the
        // loser failed instantly with SQLITE_BUSY ("database is
        // locked") because no busy timeout was set. A transient
        // lock must wait, not fail: 5 s is far longer than any
        // schema/migration write, far shorter than a user would
        // notice. This must be set before any statement runs.
        conn.pragma_update(None, "busy_timeout", 5000)?;
        // WAL is what this module's header already claims ("WAL
        // mode") — it lets readers proceed while a writer holds
        // the database, which is also what the native host needs
        // when it opens the same app.db while the app is running.
        // But it is BEST EFFORT, not required: changing the
        // journal mode needs an exclusive lock and SQLite does
        // NOT wait out contention for it (the busy handler is
        // not consulted — proven by probe: the pragma itself
        // returns SQLITE_BUSY under a race). WAL is an
        // optimisation, so a busy database keeps whatever mode
        // it already has and the open still succeeds; the
        // busy_timeout above protects every later statement.
        // Any other error from the pragma is real and propagates.
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy => {}
            Err(e) => return Err(e.into()),
        }
        conn.execute_batch(SCHEMA_SQL)?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            log_path: Mutex::new(None),
        })
    }

    /// Point the diagnostics sink at `path` (the shell's
    /// `startup.log` next to the database). Until this is called
    /// `log()` is a no-op.
    pub fn set_log_path(&self, path: PathBuf) {
        if let Ok(mut slot) = self.log_path.lock() {
            *slot = Some(path);
        }
    }

    /// Append one `[epoch-seconds] message` line to the diagnostics
    /// sink, when one is set. Best-effort and infallible by design:
    /// logging must never fail a download, a probe or a checkpoint.
    pub(crate) fn log(&self, msg: &str) {
        let path = match self.log_path.lock() {
            Ok(slot) => match slot.as_ref() {
                Some(p) => p.clone(),
                None => return,
            },
            Err(_) => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = f.write_all(format!("[{secs}] {msg}\n").as_bytes());
        }
    }

    /// Lock the single connection, logging when the wait exceeds
    /// 1 s. Every store method goes through here (not
    /// `self.conn.lock()` directly), so a slow acquisition names
    /// its call site — `#[track_caller]` makes `Location::caller`
    /// the store method that waited, not this helper. A slow wait
    /// means another thread is holding the lock across slow work;
    /// the log line is what names the waiter on the next live
    /// test. The log happens after acquiring (never while waiting),
    /// and the file write is best-effort, so this cannot wedge
    /// the lock it measures.
    #[track_caller]
    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        let start = std::time::Instant::now();
        // The raw lock: this is the one place that may touch
        // `self.conn` directly (the sed that rewrote the other
        // call sites must never rewrite this line — it would
        // recurse).
        let guard = self.conn.lock().unwrap();
        let waited = start.elapsed();
        if waited > std::time::Duration::from_secs(1) {
            self.log(&format!(
                "store lock waited {}ms at {}",
                waited.as_millis(),
                std::panic::Location::caller()
            ));
        }
        guard
    }

    /// Migrations for databases created before a schema change.
    /// `CREATE TABLE IF NOT EXISTS` in the schema never touches an
    /// existing table, so new columns need an explicit ALTER here.
    /// Each step is guarded by a PRAGMA check, so re-running is safe.
    fn migrate(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(downloads)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !cols.iter().any(|c| c == "queue_position") {
            // Added in the queues/scheduler milestone: explicit order of
            // files inside a queue (created_at has 1 s resolution and
            // cannot represent a reorder).
            conn.execute(
                "ALTER TABLE downloads ADD COLUMN queue_position INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "description") {
            // Added in Round 67 (2026-11-18): the Add URL dialog's
            // Description field (IDM's Download File Info has one) —
            // a free-text note on the download.
            conn.execute("ALTER TABLE downloads ADD COLUMN description TEXT", [])?;
        }
        drop(stmt);
        // Added in the browser-extension milestone (S13): the
        // pending-links queue. It is also in schema.sql, so fresh
        // databases get it from the schema; this ALTER-free CREATE
        // covers databases made before the table existed.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_links (
               id TEXT PRIMARY KEY, url TEXT NOT NULL, referrer TEXT,
               source TEXT NOT NULL DEFAULT 'browser'
                 CHECK (source IN ('browser', 'download_all', 'video')),
               status TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'accepted', 'rejected')),
               download_id TEXT REFERENCES downloads(id) ON DELETE SET NULL,
               error TEXT,
               cookies TEXT,
               created_at TEXT NOT NULL DEFAULT (datetime('now')))",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_pending_links_status_created \
             ON pending_links(status, created_at)",
            [],
        )?;
        // Added in Round 53 (2026-11-16): the download-history
        // table. It is also in schema.sql, so fresh databases get
        // it from the schema; this CREATE covers databases made
        // before the table existed. No foreign key on download_id
        // on purpose — deleting a download must never delete its
        // history row.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS download_history (
               id TEXT PRIMARY KEY,
               download_id TEXT NOT NULL UNIQUE,
               filename TEXT NOT NULL,
               url TEXT NOT NULL,
               final_url TEXT,
               size_bytes INTEGER,
               folder TEXT NOT NULL,
               category_id TEXT,
               completed_at TEXT NOT NULL DEFAULT (datetime('now')))",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_download_history_completed \
             ON download_history(completed_at DESC)",
            [],
        )?;
        // Added in the video-grabber milestone (S08): the
        // pending_links CHECK used to allow only 'browser' and
        // 'download_all'. A database made before S08 keeps its old
        // CHECK (CREATE TABLE IF NOT EXISTS never touches it), so
        // widen it by rebuilding the table in place — rows are
        // copied across, nothing is lost. Fresh databases get the
        // new CHECK from schema.sql directly.
        let pending_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'pending_links'",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(sql) = pending_sql {
            if !sql.contains("'video'") {
                conn.execute_batch(
                    "CREATE TABLE pending_links_new (
                       id TEXT PRIMARY KEY, url TEXT NOT NULL, referrer TEXT,
                       source TEXT NOT NULL DEFAULT 'browser'
                         CHECK (source IN ('browser', 'download_all', 'video')),
                       status TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'accepted', 'rejected')),
                       download_id TEXT REFERENCES downloads(id) ON DELETE SET NULL,
                       error TEXT,
                       created_at TEXT NOT NULL DEFAULT (datetime('now')));
                     INSERT INTO pending_links_new
                       (id, url, referrer, source, status, download_id, error, created_at)
                       SELECT id, url, referrer, source, status, download_id, error, created_at
                       FROM pending_links;
                     DROP TABLE pending_links;
                     ALTER TABLE pending_links_new RENAME TO pending_links;
                     CREATE INDEX idx_pending_links_status_created
                       ON pending_links(status, created_at);",
                )?;
            }
        }
        // Added in the S13 finish (2026-11-15): the pending row
        // carries the browser's cookies for its URL (a JSON array of
        // name/value pairs) until the drain decides the link. Fresh
        // databases get the column from the CREATE above; older
        // databases gain it here. This runs AFTER the S08 rebuild
        // above, so a rebuild cannot drop the column again.
        {
            let mut stmt = conn.prepare("PRAGMA table_info(pending_links)")?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == "cookies") {
                conn.execute("ALTER TABLE pending_links ADD COLUMN cookies TEXT", [])?;
            }
        }
        // Added in the site-grabber milestone (S06): the
        // grabber_projects table gained a 'stopped' status, live
        // progress columns and the matched-files JSON. A database
        // made before S06 keeps its old CHECK (CREATE TABLE IF NOT
        // EXISTS never touches it), so rebuild it in place — rows
        // are copied across, nothing is lost. Fresh databases get
        // the new table from schema.sql directly.
        let grabber_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'grabber_projects'",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(sql) = grabber_sql {
            if !sql.contains("'stopped'") || !sql.contains("pages_explored") {
                conn.execute_batch(
                    "CREATE TABLE grabber_projects_new (
                       id TEXT PRIMARY KEY, name TEXT NOT NULL, start_url TEXT NOT NULL,
                       filters_json TEXT NOT NULL DEFAULT '{}',
                       mode TEXT NOT NULL DEFAULT 'download'
                         CHECK (mode IN ('explore','download')),
                       schedule_json TEXT,
                       status TEXT NOT NULL DEFAULT 'idle'
                         CHECK (status IN ('idle','exploring','downloading','done','failed','stopped')),
                       pages_explored INTEGER NOT NULL DEFAULT 0,
                       files_matched INTEGER NOT NULL DEFAULT 0,
                       files_downloaded INTEGER NOT NULL DEFAULT 0,
                       files_failed INTEGER NOT NULL DEFAULT 0,
                       error_message TEXT,
                       matched_json TEXT NOT NULL DEFAULT '[]',
                       queue_id TEXT NOT NULL DEFAULT 'queue-main',
                       on_complete_action TEXT NOT NULL DEFAULT 'none'
                         CHECK (on_complete_action IN ('none','open_file','run_program','shutdown','hibernate')),
                       on_complete_program TEXT,
                       created_at TEXT NOT NULL DEFAULT (datetime('now')),
                       updated_at TEXT NOT NULL DEFAULT (datetime('now')));
                     INSERT INTO grabber_projects_new
                       (id, name, start_url, filters_json, mode, schedule_json, status, created_at, updated_at)
                       SELECT id, name, start_url, filters_json, mode, schedule_json, status, created_at, updated_at
                       FROM grabber_projects;
                     DROP TABLE grabber_projects;
                     ALTER TABLE grabber_projects_new RENAME TO grabber_projects;",
                )?;
            }
        }
        // Added in the site-grabber finish (S06, Milestone 4,
        // 2026-11-15): per-project queue choice + on-completion
        // action. A database made before Milestone 4 has neither
        // column; add them here (defaults: main queue, no action)
        // without losing any project. Fresh databases get them
        // from schema.sql directly.
        for (column, ddl) in [
            ("queue_id", "TEXT NOT NULL DEFAULT 'queue-main'"),
            (
                "on_complete_action",
                "TEXT NOT NULL DEFAULT 'none' CHECK (on_complete_action IN ('none','open_file','run_program','shutdown','hibernate'))",
            ),
            ("on_complete_program", "TEXT"),
        ] {
            let mut stmt = conn.prepare("PRAGMA table_info(grabber_projects)")?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE grabber_projects ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        // Added in the periodic-synchronisation milestone (S05/F04,
        // 2026-11-15): sync interval + last-sync time/counts on
        // queues. Fresh databases get them from schema.sql; older
        // databases gain them here, one ALTER per column.
        for (column, ddl) in [
            ("sync_interval_minutes", "INTEGER"),
            ("last_sync_at", "TEXT"),
            ("last_sync_changed", "INTEGER NOT NULL DEFAULT 0"),
            ("last_sync_unchanged", "INTEGER NOT NULL DEFAULT 0"),
            ("last_sync_removed", "INTEGER NOT NULL DEFAULT 0"),
            ("last_sync_failed", "INTEGER NOT NULL DEFAULT 0"),
            ("last_sync_skipped", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let mut stmt = conn.prepare("PRAGMA table_info(queues)")?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(&format!("ALTER TABLE queues ADD COLUMN {column} {ddl}"), [])?;
            }
        }
        // Added in the Options finish (S09, 2026-11-15): the
        // Dial-Up fields on settings, and the sound_settings table
        // (one row per event). Fresh databases get both from
        // schema.sql; older databases gain them here.
        for (table, column, ddl) in [
            ("settings", "start_on_startup", "INTEGER NOT NULL DEFAULT 0"),
            ("settings", "browser_start_drained", "INTEGER NOT NULL DEFAULT 1"),
            // Clipboard monitoring (Round 52): the on/off switch,
            // default on.
            (
                "settings",
                "clipboard_monitoring",
                "INTEGER NOT NULL DEFAULT 1",
            ),
            // Progress window (S03, 2026-11-15): the auto-open
            // switch, default on.
            (
                "settings",
                "show_progress_window",
                "INTEGER NOT NULL DEFAULT 1",
            ),
            // "Download complete" dialog (Round 66): the on/off
            // switch, default on.
            (
                "settings",
                "show_download_complete_dialog",
                "INTEGER NOT NULL DEFAULT 1",
            ),
            // Notification sounds (Round 68): the master on/off
            // switch, default on.
            ("settings", "sounds_enabled", "INTEGER NOT NULL DEFAULT 1"),
            ("settings", "default_action", "TEXT NOT NULL DEFAULT 'start_now'"),
            ("settings", "duplicate_action", "TEXT NOT NULL DEFAULT 'rename'"),
            ("settings", "confirm_delete", "INTEGER NOT NULL DEFAULT 1"),
            ("settings", "connection_timeout_secs", "INTEGER NOT NULL DEFAULT 30"),
            ("settings", "max_retries", "INTEGER NOT NULL DEFAULT 5"),
            // Patient tail (Round 73): zero-progress cap in
            // minutes, default 10.
            (
                "settings",
                "tail_zero_progress_cap_minutes",
                "INTEGER NOT NULL DEFAULT 10",
            ),
            ("settings", "dialup_connection_name", "TEXT"),
            ("settings", "dialup_auto_dial", "INTEGER NOT NULL DEFAULT 0"),
            (
                "settings",
                "dialup_hangup_after_completion",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "settings",
                "dialup_redial_attempts",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            // Update check (2026-11-15): the release-server URL
            // (Fetchline's own; default is the placeholder in
            // update.rs) and the last-check time the weekly timer
            // reads. Fresh databases get both from schema.sql.
            (
                "settings",
                "update_url",
                "TEXT NOT NULL DEFAULT 'https://updates.fetchline.app/releases/latest.json'",
            ),
            ("settings", "last_update_check_at", "TEXT"),
            ("proxy_configs", "enabled", "INTEGER NOT NULL DEFAULT 0"),
            ("proxy_configs", "bypass", "TEXT NOT NULL DEFAULT '[]'"),
            (
                "proxy_configs",
                "auth",
                "TEXT NOT NULL DEFAULT 'negotiate-auto'",
            ),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        // Added in the download-quota milestone (S12, 2026-11-15):
        // the quota switch + limit on settings, the per-download
        // cap + quota-paused flag on downloads, and the quota_state
        // singleton table holding the live window. Fresh databases
        // get all of them from schema.sql; older databases gain
        // them here. The quota_state row is seeded so `get` never
        // has to deal with a missing table row.
        for (table, column, ddl) in [
            ("settings", "quota_enabled", "INTEGER NOT NULL DEFAULT 0"),
            ("settings", "quota_mb", "INTEGER"),
            ("settings", "quota_hours", "INTEGER"),
            ("downloads", "speed_limit_kbps", "INTEGER"),
            (
                "downloads",
                "quota_paused",
                "INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        // Added in the antivirus-hook milestone (S14, 2026-11-15):
        // the scan switch + timeout on settings, and the scan
        // result columns on downloads. Fresh databases get them
        // from schema.sql; older databases gain them here.
        for (table, column, ddl) in [
            ("settings", "antivirus_enabled", "INTEGER NOT NULL DEFAULT 1"),
            (
                "settings",
                "antivirus_timeout_secs",
                "INTEGER NOT NULL DEFAULT 60",
            ),
            ("downloads", "scan_status", "TEXT"),
            ("downloads", "scan_detail", "TEXT"),
            ("downloads", "scan_scanned_at", "TEXT"),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        // Added in the progress-window milestone (S03, 2026-11-15):
        // whether the probe found the server range-capable, so the
        // progress window can show resume capability honestly.
        // NULL = unknown (a download added before this was stored).
        for (table, column, ddl) in [("downloads", "accept_ranges", "INTEGER")] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        // Added in Round 72 (2026-11-18): the live
        // connection/part counts the checkpoint writes for the
        // main window ("N connections · M parts"). Fresh
        // databases get them from schema.sql; older databases
        // gain them here.
        for (table, column, ddl) in [
            (
                "downloads",
                "active_connections",
                "INTEGER NOT NULL DEFAULT 0",
            ),
            ("downloads", "segment_count", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            if !cols.iter().any(|c| c == column) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
                    [],
                )?;
            }
        }
        conn.execute(
            "CREATE TABLE IF NOT EXISTS quota_state (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               window_start_unix INTEGER,
               bytes_used INTEGER NOT NULL DEFAULT 0 CHECK (bytes_used >= 0))",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO quota_state (id, window_start_unix, bytes_used) \
             VALUES (1, NULL, 0)",
            [],
        )?;
        Ok(())
    }

    /// In-memory database, used by the tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(Self {
            conn: Mutex::new(conn),
            log_path: Mutex::new(None),
        })
    }

    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    // -- pending links (S13 browser-extension handoff) --------------------

    /// One link the browser extension caught, waiting for the drain.
    /// `status` is pending | accepted | rejected; `download_id` is
    /// set on accept, `error` on reject. Rows are never deleted by
    /// the drain — the history of what the browser sent stays visible.
    pub fn enqueue_pending_link(
        &self,
        url: &str,
        referrer: Option<&str>,
        source: &str,
    ) -> Result<PendingLinkRow> {
        self.enqueue_pending_link_with_cookies(url, referrer, source, None)
    }

    /// Enqueue with the browser's cookies for the URL (S13 finish):
    /// `cookies` is the validated JSON array of name/value pairs,
    /// or `None` when the hand-off carried none. The column is
    /// wiped when the drain decides the row — cookies are never
    /// stored past the drain.
    pub fn enqueue_pending_link_with_cookies(
        &self,
        url: &str,
        referrer: Option<&str>,
        source: &str,
        cookies: Option<&str>,
    ) -> Result<PendingLinkRow> {
        let url = url.trim();
        // Only http(s) and ftp links can be downloaded (FTP has a
        // transport since the S09 finish); anything else is a
        // caller error, not a row. The check uses the URL as parsed,
        // so "HTTP://…" and trailing spaces are handled, not rejected.
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
            return Err(EngineError::InvalidUrl(format!(
                "Only http://, https:// and ftp:// links can be downloaded, not {}://",
                parsed.scheme()
            )));
        }
        let Some(host) = parsed.host_str().map(|h| h.to_ascii_lowercase()) else {
            return Err(EngineError::InvalidUrl("The link has no host".to_string()));
        };
        // Host boundary (defence in depth — the native host checks
        // this too, and the drain checks it a third time via
        // add_download): an excluded site never reaches the queue.
        let settings = self.get_settings().unwrap_or_default();
        if site_is_excluded(&host, &settings.excluded_sites) {
            return Err(EngineError::InvalidState(format!(
                "This site is excluded in Settings (excluded sites): {host}"
            )));
        }
        if !matches!(source, "browser" | "download_all" | "video") {
            return Err(EngineError::InvalidState(format!("Unknown pending-link source: {source}")));
        }
        let id = Self::new_id();
        let referrer = referrer.map(str::trim).filter(|r| !r.is_empty());
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO pending_links (id, url, referrer, source, status, cookies) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![id, url, referrer, source, cookies],
        )?;
        drop(conn);
        Ok(self
            .get_pending_link(&id)?
            .ok_or_else(|| EngineError::NotFound(id.clone()))?)
    }

    /// Pending links, oldest first (the drain works in arrival
    /// order). `status` filters, or `None` for every row.
    pub fn list_pending_links(&self, status: Option<&str>) -> Result<Vec<PendingLinkRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, url, referrer, source, status, download_id, error, cookies, created_at \
             FROM pending_links WHERE (?1 IS NULL OR status = ?1) \
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![status], row_to_pending_link)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    pub fn get_pending_link(&self, id: &str) -> Result<Option<PendingLinkRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, url, referrer, source, status, download_id, error, cookies, created_at \
             FROM pending_links WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_pending_link)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Mark a pending row accepted (with the created download id) or
    /// rejected (with the reason). Only a pending row can move —
    /// re-marking an already-decided row is a caller error, so a
    /// double drain can never accept the same link twice. Deciding
    /// the row also wipes its cookies column (S13 finish): cookies
    /// are held only while the row is pending, never past the drain.
    /// Round 69 (d) — expire interactive browser/video captures
    /// older than 24 h. A capture the user never answered used to
    /// sit `pending` forever, and the frontend's capture queue
    /// re-offered it on every launch — closing the Add URL dialog
    /// reopened it instantly with the stale link, which looked
    /// exactly like an unclosable dialog (Round 68 retest). An
    /// expired row is marked `rejected` with the reason, so it
    /// never reaches the dialog again. `download_all` rows are
    /// NOT expired here — the drain decides those itself.
    /// `created_at` is `datetime('now')` text, so the comparison
    /// is lexicographic and correct for that format.
    pub fn expire_stale_pending_links(&self) -> Result<usize> {
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE pending_links SET status = 'rejected', \
             error = 'Expired — the link was captured more than a day ago and was never confirmed', cookies = NULL \
             WHERE status = 'pending' AND source IN ('browser', 'video') \
             AND created_at < datetime('now', '-24 hours')",
            [],
        )?;
        Ok(changed)
    }

    pub fn mark_pending_link(
        &self,
        id: &str,
        status: &str,
        download_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        if !matches!(status, "accepted" | "rejected") {
            return Err(EngineError::InvalidState(format!("Unknown pending-link status: {status}")));
        }
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE pending_links SET status = ?1, download_id = ?2, error = ?3, cookies = NULL \
             WHERE id = ?4 AND status = 'pending'",
            params![status, download_id, error, id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // -- site grabber (S06) ---------------------------------------------

    /// Create a grabber project. `schedule_start_at` is the optional
    /// one-time start time (UTC ISO-8601); it is stored in
    /// `schedule_json` in the same shape as a queue schedule, and
    /// the existing 30 s scheduler tick starts the project when the
    /// time arrives — there is no second scheduler. `queue_id` is
    /// the queue grabbed files go to (the caller resolves the
    /// default — the main queue — before calling); the completion
    /// action reuses the queue completion actions.
    pub fn create_grabber_project(
        &self,
        name: &str,
        start_url: &str,
        filters: &GrabberFilters,
        mode: &str,
        schedule_start_at: Option<&str>,
        queue_id: &str,
        on_complete_action: &str,
        on_complete_program: Option<&str>,
    ) -> Result<GrabberProjectRow> {
        let id = Self::new_id();
        let filters_json = serde_json::to_string(filters)?;
        let schedule_json = schedule_start_at
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::json!({"start_at": s}).to_string());
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO grabber_projects (id, name, start_url, filters_json, mode, schedule_json, status, \
             queue_id, on_complete_action, on_complete_program) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'idle', ?7, ?8, ?9)",
            params![
                id,
                name.trim(),
                start_url.trim(),
                filters_json,
                mode,
                schedule_json,
                queue_id,
                on_complete_action,
                on_complete_program
            ],
        )?;
        drop(conn);
        self.get_grabber_project(&id)?
            .ok_or_else(|| EngineError::NotFound(id.clone()))
    }

    pub fn get_grabber_project(&self, id: &str) -> Result<Option<GrabberProjectRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, start_url, filters_json, mode, schedule_json, status, \
             pages_explored, files_matched, files_downloaded, files_failed, \
             error_message, matched_json, created_at, \
             queue_id, on_complete_action, on_complete_program \
             FROM grabber_projects WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_grabber_project)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// All grabber projects, newest first.
    pub fn list_grabber_projects(&self) -> Result<Vec<GrabberProjectRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, start_url, filters_json, mode, schedule_json, status, \
             pages_explored, files_matched, files_downloaded, files_failed, \
             error_message, matched_json, created_at, \
             queue_id, on_complete_action, on_complete_program \
             FROM grabber_projects ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], row_to_grabber_project)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// Write a project's status and live progress in one go. The
    /// matched-files list is stored as JSON so the progress view can
    /// list what was found, including per-file failures.
    #[allow(clippy::too_many_arguments)]
    pub fn update_grabber_project(
        &self,
        id: &str,
        status: &str,
        error_message: Option<&str>,
        pages_explored: i64,
        files_matched: i64,
        files_downloaded: i64,
        files_failed: i64,
        matched_files: &[MatchedFile],
    ) -> Result<()> {
        let matched_json = serde_json::to_string(matched_files)?;
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE grabber_projects SET status = ?1, error_message = ?2, \
             pages_explored = ?3, files_matched = ?4, files_downloaded = ?5, \
             files_failed = ?6, matched_json = ?7, updated_at = datetime('now') \
             WHERE id = ?8",
            params![
                status,
                error_message,
                pages_explored,
                files_matched,
                files_downloaded,
                files_failed,
                matched_json,
                id
            ],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Tag a download as belonging to a grabber project, so the main
    /// list can show where it came from. Called right after
    /// `add_download` creates the row — the add path itself is
    /// untouched, so S07/S13/S08 behaviour does not change.
    pub fn set_download_grabber_project(&self, download_id: &str, project_id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET grabber_project_id = ?1 WHERE id = ?2",
            params![project_id, download_id],
        )?;
        Ok(())
    }

    // -- categories -------------------------------------------------------

    pub fn list_categories(&self) -> Result<Vec<CategoryRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, default_folder, file_extensions, is_builtin \
             FROM categories ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            let ext_json: String = r.get(3)?;
            let ext: Vec<String> = serde_json::from_str(&ext_json).unwrap_or_default();
            Ok(CategoryRow {
                id: r.get(0)?,
                name: r.get(1)?,
                default_folder: r.get(2)?,
                file_extensions: ext,
                is_builtin: r.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// Pick a category for a file extension, falling back to Documents.
    pub fn category_for_extension(&self, ext: &str) -> Result<String> {
        let ext = ext.to_ascii_lowercase();
        for cat in self.list_categories()? {
            if cat.file_extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                return Ok(cat.id);
            }
        }
        Ok("cat-documents".to_string())
    }

    /// Auto-assign a category for an extension, honouring the
    /// `file_types_auto` allow-list (BUG-011): an empty list means
    /// every extension is auto-assigned; a non-empty list restricts
    /// auto-assign to the listed extensions, and anything else falls
    /// back to Documents — the same fallback as an unknown extension.
    pub fn category_for_extension_filtered(&self, ext: &str, auto_list: &[String]) -> Result<String> {
        let ext = ext.to_ascii_lowercase();
        if !auto_list.is_empty()
            && !auto_list.iter().any(|e| e.eq_ignore_ascii_case(&ext))
        {
            return Ok("cat-documents".to_string());
        }
        self.category_for_extension(&ext)
    }

    /// Create a custom category. Names must be unique (enforced by the
    /// schema) and non-empty; extensions are stored lowercase, without dots.
    pub fn create_category(
        &self,
        name: &str,
        folder: &str,
        extensions: &[String],
    ) -> Result<CategoryRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::InvalidState("Give the category a name".to_string()));
        }
        let folder = folder.trim();
        if folder.is_empty() {
            return Err(EngineError::InvalidState("Give the category a folder".to_string()));
        }
        let clean: Vec<String> = extensions
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if clean.is_empty() {
            return Err(EngineError::InvalidState(
                "List at least one file type, for example: mp4, mkv".to_string(),
            ));
        }
        let id = Self::new_id();
        let ext_json = serde_json::to_string(&clean)?;
        let conn = self.lock_conn();
        match conn.execute(
            "INSERT INTO categories (id, name, default_folder, file_extensions, is_builtin) \
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![id, name, folder, ext_json],
        ) {
            Ok(_) => Ok(CategoryRow {
                id,
                name: name.to_string(),
                default_folder: folder.to_string(),
                file_extensions: clean,
                is_builtin: false,
            }),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(EngineError::InvalidState(format!(
                    "A category called “{name}” already exists"
                )))
            }
            Err(e) => Err(EngineError::from(e)),
        }
    }

    /// Delete a category. Built-in categories cannot be deleted, and a
    /// category that still has downloads is refused — the schema's
    /// `ON DELETE RESTRICT` would block it anyway, but the user gets a
    /// clear message instead of a database error.
    pub fn delete_category(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        let (is_builtin, name): (i64, String) = conn
            .query_row(
                "SELECT is_builtin, name FROM categories WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| EngineError::NotFound(id.to_string()))?;
        if is_builtin != 0 {
            return Err(EngineError::InvalidState(format!(
                "“{name}” is a built-in category and cannot be deleted"
            )));
        }
        let in_use: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE category_id = ?1 AND status != 'deleted'",
            params![id],
            |r| r.get(0),
        )?;
        if in_use > 0 {
            return Err(EngineError::InvalidState(format!(
                "“{name}” still has {in_use} download{} — move them to another category first",
                if in_use == 1 { "" } else { "s" }
            )));
        }
        // Soft-deleted downloads are invisible in the UI and their rows
        // are garbage: remove them first. Without this, the schema's
        // ON DELETE RESTRICT on downloads.category_id refuses the
        // category delete with a raw database error even though the
        // check above just said the delete is allowed.
        conn.execute(
            "DELETE FROM downloads WHERE category_id = ?1 AND status = 'deleted'",
            params![id],
        )?;
        conn.execute("DELETE FROM categories WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Move one download into another category.
    pub fn move_to_category(&self, download_id: &str, category_id: &str) -> Result<()> {
        let conn = self.lock_conn();
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM categories WHERE id = ?1",
            params![category_id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(EngineError::NotFound(category_id.to_string()));
        }
        let changed = conn.execute(
            "UPDATE downloads SET category_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![category_id, download_id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(download_id.to_string()));
        }
        Ok(())
    }

    /// One category by id (for the Save To tab and folder lookup).
    pub fn get_category(&self, id: &str) -> Result<Option<CategoryRow>> {
        Ok(self.list_categories()?.into_iter().find(|c| c.id == id))
    }

    /// Change a category's save folder and/or file extensions
    /// (S09 Save To + File Types tabs). Either field may be left
    /// unchanged by passing `None`. Extensions are cleaned the same
    /// way as on creation; a category keeps at least one extension.
    pub fn update_category(
        &self,
        id: &str,
        folder: Option<&str>,
        extensions: Option<&[String]>,
    ) -> Result<CategoryRow> {
        let mut row = self
            .get_category(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if let Some(folder) = folder {
            let folder = folder.trim();
            if folder.is_empty() {
                return Err(EngineError::InvalidState(
                    "Give the category a folder".to_string(),
                ));
            }
            row.default_folder = folder.to_string();
        }
        if let Some(extensions) = extensions {
            let clean: Vec<String> = extensions
                .iter()
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
            if clean.is_empty() {
                return Err(EngineError::InvalidState(
                    "List at least one file type, for example: mp4, mkv".to_string(),
                ));
            }
            row.file_extensions = clean;
        }
        let ext_json = serde_json::to_string(&row.file_extensions)?;
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE categories SET default_folder = ?1, file_extensions = ?2, \
             updated_at = datetime('now') WHERE id = ?3",
            params![row.default_folder, ext_json, id],
        )?;
        Ok(row)
    }

    // -- settings (S09) ---------------------------------------------------

    /// The singleton settings row (id = 1), with the schema defaults
    /// when the row is missing a value.
    pub fn get_settings(&self) -> Result<Settings> {
        let conn = self.lock_conn();
        let row = conn.query_row(
            "SELECT default_download_folder, max_connections, speed_limit_kbps, \
             language, theme, browser_integration, browser_start_drained, antivirus_command, \
             file_types_auto, excluded_sites, check_updates_weekly, \
             start_on_startup, default_action, duplicate_action, \
             confirm_delete, connection_timeout_secs, max_retries, \
             dialup_connection_name, dialup_auto_dial, \
             dialup_hangup_after_completion, dialup_redial_attempts, \
             update_url, last_update_check_at, \
             quota_enabled, quota_mb, quota_hours, \
             antivirus_enabled, antivirus_timeout_secs, show_progress_window, \
             clipboard_monitoring, show_download_complete_dialog, sounds_enabled, \
             tail_zero_progress_cap_minutes \
             FROM settings WHERE id = 1",
            [],
            |r| {
                let file_types: String = r.get(8)?;
                let excluded: String = r.get(9)?;
                Ok(Settings {
                    default_download_folder: r.get(0)?,
                    max_connections: r.get(1)?,
                    speed_limit_kbps: r.get(2)?,
                    language: r.get(3)?,
                    theme: r.get(4)?,
                    browser_integration: r.get::<_, i64>(5)? != 0,
                    browser_start_drained: r.get::<_, i64>(6)? != 0,
                    antivirus_command: r.get(7)?,
                    file_types_auto: serde_json::from_str(&file_types).unwrap_or_default(),
                    excluded_sites: serde_json::from_str(&excluded).unwrap_or_default(),
                    check_updates_weekly: r.get::<_, i64>(10)? != 0,
                    start_on_startup: r.get::<_, i64>(11)? != 0,
                    default_action: r.get(12)?,
                    duplicate_action: r.get(13)?,
                    confirm_delete: r.get::<_, i64>(14)? != 0,
                    connection_timeout_secs: r.get(15)?,
                    max_retries: r.get(16)?,
                    dialup_connection_name: r
                        .get::<_, Option<String>>(17)?
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    dialup_auto_dial: r.get::<_, i64>(18)? != 0,
                    dialup_hangup_after_completion: r.get::<_, i64>(19)? != 0,
                    dialup_redial_attempts: r.get(20)?,
                    update_url: r
                        .get::<_, Option<String>>(21)?
                        .filter(|u| !u.trim().is_empty())
                        .unwrap_or_else(|| crate::update::DEFAULT_UPDATE_URL.to_string()),
                    last_update_check_at: r
                        .get::<_, Option<String>>(22)?
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    quota_enabled: r.get::<_, i64>(23)? != 0,
                    quota_mb: r.get(24)?,
                    quota_hours: r.get(25)?,
                    antivirus_enabled: r.get::<_, i64>(26)? != 0,
                    antivirus_timeout_secs: r.get::<_, Option<i64>>(27)?.unwrap_or(
                        crate::antivirus::DEFAULT_SCAN_TIMEOUT_SECS,
                    ),
                    show_progress_window: r.get::<_, i64>(28)? != 0,
                    clipboard_monitoring: r.get::<_, i64>(29)? != 0,
                    show_download_complete_dialog: r.get::<_, i64>(30)? != 0,
                    sounds_enabled: r.get::<_, i64>(31)? != 0,
                    tail_zero_progress_cap_minutes: r
                        .get::<_, Option<i64>>(32)?
                        .unwrap_or(DEFAULT_TAIL_ZERO_PROGRESS_CAP_MINUTES),
                })
            },
        );
        row.map_err(EngineError::from)
    }

    /// Replace the settings row, after validation. A speed limit of
    /// 0 means unlimited and is stored as NULL. Values the UI does
    /// not send are validated here too, so a bad engine caller cannot
    /// store a max-connections of 0 or a negative timeout.
    pub fn update_settings(&self, s: &Settings) -> Result<Settings> {
        if !(1..=32).contains(&s.max_connections) {
            return Err(EngineError::InvalidState(
                "Use between 1 and 32 connections per download".to_string(),
            ));
        }
        if let Some(kbps) = s.speed_limit_kbps {
            if kbps < 0 {
                return Err(EngineError::InvalidState(
                    "The speed limit cannot be negative — use 0 for unlimited".to_string(),
                ));
            }
            // BUG-008: reject absurd caps instead of storing them —
            // converting them to bytes/s overflows (debug panic,
            // silent wrap in release).
            if kbps > MAX_SPEED_LIMIT_KBPS {
                return Err(EngineError::InvalidState(format!(
                    "The speed limit is too large — the maximum is {MAX_SPEED_LIMIT_KBPS} KB/s"
                )));
            }
        }
        if !matches!(s.theme.as_str(), "light" | "dark") {
            return Err(EngineError::InvalidState(format!("Unknown theme: {}", s.theme)));
        }
        if !matches!(s.default_action.as_str(), "start_now" | "save_for_later") {
            return Err(EngineError::InvalidState(format!(
                "Unknown default action: {}",
                s.default_action
            )));
        }
        if !matches!(s.duplicate_action.as_str(), "rename" | "overwrite") {
            return Err(EngineError::InvalidState(format!(
                "Unknown duplicate handling: {}",
                s.duplicate_action
            )));
        }
        if !(1..=300).contains(&s.connection_timeout_secs) {
            return Err(EngineError::InvalidState(
                "The connection timeout must be between 1 and 300 seconds".to_string(),
            ));
        }
        if !(0..=10).contains(&s.max_retries) {
            return Err(EngineError::InvalidState(
                "Retries must be between 0 and 10".to_string(),
            ));
        }
        // Patient tail (Round 73): the zero-progress cap is
        // 1–120 minutes, rejected not clamped (the BUG-008 rule).
        if !(1..=120).contains(&s.tail_zero_progress_cap_minutes) {
            return Err(EngineError::InvalidState(
                "The patient-tail zero-progress cap must be between 1 and 120 minutes".to_string(),
            ));
        }
        if s.language.trim().is_empty() {
            return Err(EngineError::InvalidState("Pick a language".to_string()));
        }
        // Dial-Up tab (S09 finish): redial attempts are 0–10.
        // The connection name is free text (empty = none set).
        crate::options::validate_dialup(s.dialup_redial_attempts)?;
        // Update check: the server address must be a usable
        // http(s) URL. An empty address is rejected, not silently
        // reset to the default — a blank field in Settings means
        // the user cleared it by mistake, and the check would
        // otherwise go to a server nobody configured.
        {
            let url = s.update_url.trim();
            let parsed = reqwest::Url::parse(url).ok();
            if url.is_empty()
                || !matches!(parsed.map(|u| u.scheme().to_string()).as_deref(), Some("http") | Some("https"))
            {
                return Err(EngineError::InvalidState(
                    "The update server address must be an http:// or https:// URL".to_string(),
                ));
            }
        }
        // Download quota (S12): when enabled, both the limit and
        // the window must be set and inside their bounds; when
        // disabled, a configured value must still be in bounds, so
        // turning the quota back on cannot silently use a bad one.
        // Rejected, not clamped (the BUG-008 rule): 0 MB would
        // pause every download at once, and a 0-hour window would
        // roll over before anything downloads.
        if let Some(mb) = s.quota_mb {
            if !(crate::quota::MIN_QUOTA_MB..=crate::quota::MAX_QUOTA_MB).contains(&mb) {
                return Err(EngineError::InvalidState(format!(
                    "The download quota must be between {} and {} MB",
                    crate::quota::MIN_QUOTA_MB,
                    crate::quota::MAX_QUOTA_MB
                )));
            }
        }
        if let Some(hours) = s.quota_hours {
            if !(crate::quota::MIN_QUOTA_HOURS..=crate::quota::MAX_QUOTA_HOURS).contains(&hours) {
                return Err(EngineError::InvalidState(format!(
                    "The quota window must be between {} and {} hours",
                    crate::quota::MIN_QUOTA_HOURS,
                    crate::quota::MAX_QUOTA_HOURS
                )));
            }
        }
        if s.quota_enabled && (s.quota_mb.is_none() || s.quota_hours.is_none()) {
            return Err(EngineError::InvalidState(
                "Turn the quota on with both a size in MB and a window in hours".to_string(),
            ));
        }
        // Antivirus hook (S14): the scan timeout is 5–600 s,
        // rejected not clamped (the BUG-008 rule).
        crate::antivirus::validate_scan_timeout(s.antivirus_timeout_secs)?;
        let speed = match s.speed_limit_kbps {
            Some(0) | None => None,
            Some(kbps) => Some(kbps),
        };
        // BUG-011: the two lists are normalised on save, so the engine
        // and the UI always read the same clean values back.
        let file_types = serde_json::to_string(&clean_extensions(&s.file_types_auto))?;
        let excluded = serde_json::to_string(&clean_excluded_sites(&s.excluded_sites))?;
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE settings SET default_download_folder = ?1, max_connections = ?2, \
             speed_limit_kbps = ?3, language = ?4, theme = ?5, browser_integration = ?6, \
             browser_start_drained = ?7, antivirus_command = ?8, file_types_auto = ?9, excluded_sites = ?10, \
             check_updates_weekly = ?11, start_on_startup = ?12, default_action = ?13, \
             duplicate_action = ?14, confirm_delete = ?15, connection_timeout_secs = ?16, \
             max_retries = ?17, dialup_connection_name = ?18, dialup_auto_dial = ?19, \
             dialup_hangup_after_completion = ?20, dialup_redial_attempts = ?21, \
             update_url = ?22, last_update_check_at = ?23, \
             quota_enabled = ?24, quota_mb = ?25, quota_hours = ?26, \
             antivirus_enabled = ?27, antivirus_timeout_secs = ?28, \
             show_progress_window = ?29, clipboard_monitoring = ?30, \
             show_download_complete_dialog = ?31, sounds_enabled = ?32, \
             tail_zero_progress_cap_minutes = ?33, \
             updated_at = datetime('now') WHERE id = 1",
            params![
                s.default_download_folder.trim(),
                s.max_connections,
                speed,
                s.language.trim(),
                s.theme,
                if s.browser_integration { 1 } else { 0 },
                if s.browser_start_drained { 1 } else { 0 },
                s.antivirus_command.as_deref().filter(|v| !v.trim().is_empty()),
                file_types,
                excluded,
                if s.check_updates_weekly { 1 } else { 0 },
                if s.start_on_startup { 1 } else { 0 },
                s.default_action,
                s.duplicate_action,
                if s.confirm_delete { 1 } else { 0 },
                s.connection_timeout_secs,
                s.max_retries,
                s.dialup_connection_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty()),
                if s.dialup_auto_dial { 1 } else { 0 },
                if s.dialup_hangup_after_completion { 1 } else { 0 },
                s.dialup_redial_attempts,
                s.update_url.trim(),
                s.last_update_check_at.as_deref().filter(|v| !v.trim().is_empty()),
                if s.quota_enabled { 1 } else { 0 },
                s.quota_mb,
                s.quota_hours,
                if s.antivirus_enabled { 1 } else { 0 },
                s.antivirus_timeout_secs,
                if s.show_progress_window { 1 } else { 0 },
                if s.clipboard_monitoring { 1 } else { 0 },
                if s.show_download_complete_dialog { 1 } else { 0 },
                if s.sounds_enabled { 1 } else { 0 },
                s.tail_zero_progress_cap_minutes,
            ],
        )?;
        drop(conn);
        self.get_settings()
    }

    /// Record that an update check ran at `at_iso` (UTC ISO-8601),
    /// whether it succeeded or failed — the weekly timer reads
    /// this to decide whether the next check is due. The check
    /// itself must never fail because this write failed, so
    /// callers ignore its error.
    pub fn record_update_check(&self, at_iso: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE settings SET last_update_check_at = ?1, updated_at = datetime('now') WHERE id = 1",
            params![at_iso],
        )?;
        Ok(())
    }

    // -- download quota (S12) -------------------------------------------

    /// The live quota window: `(window_start_unix, bytes_used)`.
    /// A missing row (a database made before the seed, or an
    /// in-memory database) reads as "no window open, nothing used"
    /// — never an error.
    pub fn get_quota_state(&self) -> Result<(Option<i64>, i64)> {
        let conn = self.lock_conn();
        let row = conn.query_row(
            "SELECT window_start_unix, bytes_used FROM quota_state WHERE id = 1",
            [],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
        );
        match row {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, 0)),
            Err(e) => Err(EngineError::from(e)),
        }
    }

    /// Write the live quota window outright (used by rollover and
    /// reset). `window_start_unix = None` means "no window open" —
    /// the next counted byte anchors a fresh one.
    pub fn set_quota_state(&self, window_start_unix: Option<i64>, bytes_used: i64) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO quota_state (id, window_start_unix, bytes_used) VALUES (1, ?1, ?2) \
             ON CONFLICT(id) DO UPDATE SET window_start_unix = excluded.window_start_unix, \
             bytes_used = excluded.bytes_used",
            params![window_start_unix, bytes_used.max(0)],
        )?;
        Ok(())
    }

    /// Count `bytes` just downloaded against the persisted window
    /// at `now`, anchoring a new window when none is open or the
    /// old one expired. Returns the new total used in the window.
    /// This is the ONLY writer of `bytes_used` during a download,
    /// so the window survives a restart: the usage is on disk
    /// within a chunk of the bytes being written.
    pub fn record_quota_bytes(&self, bytes: u64, window_secs: i64, now: i64) -> Result<u64> {
        let (start, used) = self.get_quota_state()?;
        let (start, used) = match start {
            Some(s) if now < s + window_secs => (Some(s), used),
            _ => (Some(now), 0),
        };
        let used = used.saturating_add(bytes as i64);
        self.set_quota_state(start, used)?;
        Ok(used as u64)
    }

    /// How many downloads are paused waiting for the quota window.
    pub fn count_quota_paused(&self) -> Result<i64> {
        let conn = self.lock_conn();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE quota_paused = 1 AND status = 'paused'",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Ids of the downloads paused by the quota (still `paused`).
    pub fn list_quota_paused_ids(&self) -> Result<Vec<String>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM downloads WHERE quota_paused = 1 AND status = 'paused' \
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// Mark (or unmark) a download as paused-by-quota. Only the
    /// quota pause path sets this; a user's own pause never does,
    /// so a rollover can never auto-resume a download the user
    /// paused by hand.
    pub fn set_quota_paused(&self, id: &str, paused: bool) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET quota_paused = ?1 WHERE id = ?2",
            params![if paused { 1 } else { 0 }, id],
        )?;
        Ok(())
    }

    /// Set (or clear) one download's per-download speed cap.
    /// `None` (or 0) means no per-download cap. Validated here,
    /// like the global cap (BUG-008): an absurd cap is rejected,
    /// not clamped, so a typo is visible instead of silently
    /// becoming a different cap.
    pub fn set_download_speed_limit(&self, id: &str, kbps: Option<i64>) -> Result<()> {
        let kbps = match kbps {
            Some(0) | None => None,
            Some(k) => {
                if k < 0 {
                    return Err(EngineError::InvalidState(
                        "The per-download speed limit cannot be negative — use 0 for no cap".to_string(),
                    ));
                }
                if k > MAX_SPEED_LIMIT_KBPS {
                    return Err(EngineError::InvalidState(format!(
                        "The per-download speed limit is too large — the maximum is {MAX_SPEED_LIMIT_KBPS} KB/s"
                    )));
                }
                Some(k)
            }
        };
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE downloads SET speed_limit_kbps = ?1 WHERE id = ?2",
            params![kbps, id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // -- site logins (S09 Site logins tab) --------------------------------

    /// All saved site logins, by host. The password is never
    /// included — only whether one is stored.
    pub fn list_site_logins(&self) -> Result<Vec<SiteLoginRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, host, username, auth_type, \
             (password_enc IS NOT NULL AND password_enc != '') \
             FROM site_logins ORDER BY host",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SiteLoginRow {
                id: r.get(0)?,
                host: r.get(1)?,
                username: r.get(2)?,
                auth_type: r.get(3)?,
                has_password: r.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// The saved credential for one host, decrypted, for the engine
    /// only. Returns `None` when no login is saved for the host.
    /// This is the only place a site-login password is decrypted,
    /// and the result never crosses the Tauri bridge. Host matching
    /// is exact (after the same cleaning the save path uses) — a
    /// login for `example.com` is not sent to `sub.example.com`.
    pub fn get_site_login_credentials(&self, host: &str) -> Result<Option<(String, String)>> {
        let host = clean_site_host(host).unwrap_or_else(|| host.trim().to_ascii_lowercase());
        let conn = self.lock_conn();
        let row = conn.query_row(
            "SELECT username, password_enc FROM site_logins WHERE host = ?1",
            params![host],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        );
        let (username, password_enc) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(EngineError::from(e)),
        };
        Ok(Some((username, unprotect_secret(&password_enc)?)))
    }

    /// Save (insert or replace by host) a site login. `password:
    /// None` keeps the stored password, `Some(p)` sets it
    /// (protected before storage — see `secret.rs`). Only Basic
    /// auth is accepted: the engine only sends Basic auth, and
    /// storing another type would promise a login the engine
    /// cannot perform. Passwords are never logged; the returned
    /// row carries only `has_password`.
    pub fn save_site_login(&self, input: &SiteLoginInput) -> Result<SiteLoginRow> {
        let existing = self.list_site_logins()?;
        let by_host = existing.iter().find(|r| {
            r.host == clean_site_host(&input.host).unwrap_or_default()
        });
        let has_password = by_host.map(|r| r.has_password).unwrap_or(false);
        let host = validate_site_login_input(input, has_password)?;
        let password_enc = match &input.password {
            None => {
                // Keep the stored value, if any.
                match by_host {
                    Some(row) => {
                        let conn = self.lock_conn();
                        conn.query_row(
                            "SELECT password_enc FROM site_logins WHERE id = ?1",
                            params![row.id],
                            |r| r.get::<_, String>(0),
                        )?
                    }
                    None => {
                        return Err(EngineError::InvalidState(
                            "Give the login a password".to_string(),
                        ))
                    }
                }
            }
            Some(p) => protect_secret(p),
        };
        let id = input
            .id
            .clone()
            .filter(|i| !i.trim().is_empty())
            .or_else(|| by_host.map(|r| r.id.clone()))
            .unwrap_or_else(Self::new_id);
        let username = input.username.trim().to_string();
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO site_logins (id, host, username, password_enc, auth_type, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'basic', datetime('now')) \
             ON CONFLICT(host) DO UPDATE SET username = excluded.username, \
             password_enc = excluded.password_enc, auth_type = 'basic', \
             updated_at = datetime('now')",
            params![id, host, username, password_enc],
        )?;
        drop(conn);
        self.list_site_logins()?
            .into_iter()
            .find(|r| r.host == host)
            .ok_or_else(|| EngineError::InvalidState("Could not save the site login".to_string()))
    }

    /// Delete a saved site login. Unknown ids are a not-found error.
    pub fn delete_site_login(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        let changed = conn.execute("DELETE FROM site_logins WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        Ok(())
    }

    // -- sounds (S09 Sounds tab) ------------------------------------------

    /// All three events' sound settings, in UI order.
    pub fn get_sound_settings(&self) -> Result<Vec<SoundSetting>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT event, enabled, file_path FROM sound_settings",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SoundSetting {
                event: r.get(0)?,
                enabled: r.get::<_, i64>(1)? != 0,
                file: r.get(2)?,
            })
        })?;
        let mut out: Vec<SoundSetting> =
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)?;
        // The seeded rows are the whole vocabulary; order them the
        // way the UI lists them, and never surface an unknown event.
        out.retain(|s| SOUND_EVENTS.contains(&s.event.as_str()));
        out.sort_by_key(|s| SOUND_EVENTS.iter().position(|e| e == &s.event).unwrap_or(99));
        Ok(out)
    }

    /// The configured file for one event, when the event is enabled
    /// and a file is set. `None` means "no sound for this event" —
    /// disabled, unset, or unknown event — never an error.
    pub fn get_sound_file(&self, event: &str) -> Result<Option<String>> {
        Ok(self
            .get_sound_settings()?
            .into_iter()
            .find(|s| s.event == event)
            .filter(|s| s.enabled)
            .and_then(|s| s.file))
    }

    /// Save one event's sound setting. The file, when set, must be
    /// a `.wav` file; an empty file clears it.
    pub fn save_sound_setting(&self, input: &SoundSettingInput) -> Result<SoundSetting> {
        let file = validate_sound_setting(input)?;
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO sound_settings (event, enabled, file_path, updated_at) \
             VALUES (?1, ?2, ?3, datetime('now')) \
             ON CONFLICT(event) DO UPDATE SET enabled = excluded.enabled, \
             file_path = excluded.file_path, updated_at = datetime('now')",
            params![input.event, if input.enabled { 1 } else { 0 }, file],
        )?;
        drop(conn);
        self.get_sound_settings()?
            .into_iter()
            .find(|s| s.event == input.event)
            .ok_or_else(|| EngineError::InvalidState("Could not save the sound".to_string()))
    }

    // -- proxy (S09 Proxy/Socks tab) --------------------------------------

    /// The single proxy row, if one was ever saved. The password is
    /// never included — only whether one is stored.
    pub fn get_proxy_config(&self) -> Result<Option<ProxyConfigRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, type, host, port, username, use_for, bypass, enabled, \
             (password_enc IS NOT NULL AND password_enc != ''), auth \
             FROM proxy_configs ORDER BY updated_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |r| {
            let use_for: String = r.get(6)?;
            let bypass: String = r.get(7)?;
            let auth: String = r.get(10)?;
            Ok(ProxyConfigRow {
                id: r.get(0)?,
                name: r.get(1)?,
                proxy_type: r.get(2)?,
                host: r.get(3)?,
                port: r.get(4)?,
                username: r.get(5)?,
                use_for: serde_json::from_str(&use_for).unwrap_or_default(),
                bypass: serde_json::from_str(&bypass).unwrap_or_default(),
                enabled: r.get::<_, i64>(8)? != 0,
                has_password: r.get::<_, i64>(9)? != 0,
                auth: resolve_proxy_auth(&auth).to_string(),
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// The enabled proxy with its password decrypted, for the engine
    /// only. Returns `None` when no proxy is saved or it is disabled.
    /// This is the only place the stored password is decrypted, and
    /// the result never crosses the Tauri bridge.
    pub fn get_active_proxy(&self) -> Result<Option<ProxyConfig>> {
        let conn = self.lock_conn();
        let row = conn.query_row(
            "SELECT type, host, port, username, password_enc, use_for, bypass, auth \
             FROM proxy_configs WHERE enabled = 1 ORDER BY updated_at DESC LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        );
        let (proxy_type, host, port, username, password_enc, use_for, bypass, auth) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(EngineError::from(e)),
        };
        let password = match password_enc {
            Some(enc) if !enc.is_empty() => Some(unprotect_secret(&enc)?),
            _ => None,
        };
        Ok(Some(ProxyConfig {
            proxy_type,
            host,
            port,
            username: username.filter(|u| !u.trim().is_empty()),
            password,
            use_for: serde_json::from_str(&use_for).unwrap_or_default(),
            bypass: serde_json::from_str(&bypass).unwrap_or_default(),
            auth: resolve_proxy_auth(&auth).to_string(),
        }))
    }

    /// Save (insert or replace) the single proxy row. `password:
    /// None` keeps the stored password, `Some("")` clears it,
    /// `Some(p)` sets it (protected before storage — see secret.rs).
    /// `auth: None` keeps the stored authentication setting
    /// (`negotiate-auto` for a new proxy). Only one proxy row is
    /// kept: saving replaces the previous one.
    pub fn save_proxy_config(&self, input: &ProxyConfigInput) -> Result<ProxyConfigRow> {
        validate_proxy_input(input)?;
        let existing = self.get_proxy_config()?;
        let password_enc = match &input.password {
            None => {
                // Keep the stored value, if any.
                match &existing {
                    Some(_) => {
                        let conn = self.lock_conn();
                        conn.query_row(
                            "SELECT password_enc FROM proxy_configs ORDER BY updated_at DESC LIMIT 1",
                            [],
                            |r| r.get::<_, Option<String>>(0),
                        )?
                    }
                    None => None,
                }
            }
            Some(p) if p.is_empty() => None,
            Some(p) => Some(protect_secret(p)),
        };
        let use_for = serde_json::to_string(&input.use_for)?;
        let bypass = serde_json::to_string(&input.bypass)?;
        let username = input.username.clone().filter(|u| !u.trim().is_empty());
        let auth = match &input.auth {
            Some(a) => resolve_proxy_auth(a).to_string(),
            None => existing
                .as_ref()
                .map(|r| r.auth.clone())
                .unwrap_or_else(|| crate::proxy::PROXY_AUTH_NEGOTIATE_AUTO.to_string()),
        };
        let id = existing.map(|r| r.id).unwrap_or_else(Self::new_id);
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO proxy_configs (id, name, type, host, port, username, password_enc, \
             use_for, enabled, bypass, auth, updated_at) \
             VALUES (?1, 'Default proxy', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now')) \
             ON CONFLICT(id) DO UPDATE SET type = excluded.type, host = excluded.host, \
             port = excluded.port, username = excluded.username, \
             password_enc = excluded.password_enc, use_for = excluded.use_for, \
             enabled = excluded.enabled, bypass = excluded.bypass, auth = excluded.auth, \
             updated_at = datetime('now')",
            params![
                id,
                input.proxy_type,
                input.host.trim(),
                input.port,
                username,
                password_enc,
                use_for,
                if input.enabled { 1 } else { 0 },
                bypass,
                auth,
            ],
        )?;
        drop(conn);
        self.get_proxy_config()?
            .ok_or_else(|| EngineError::InvalidState("Could not save the proxy".to_string()))
    }

    // -- queues -----------------------------------------------------------

    pub fn list_queues(&self) -> Result<Vec<QueueRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT q.id, q.name, q.type, q.is_main, q.schedule_type, q.start_at, q.stop_at, \
             q.start_on_startup, q.on_complete_action, q.on_complete_program, \
             q.sync_interval_minutes, q.last_sync_at, q.last_sync_changed, \
             q.last_sync_unchanged, q.last_sync_removed, q.last_sync_failed, \
             q.last_sync_skipped, \
             (SELECT COUNT(*) FROM downloads d WHERE d.queue_id = q.id AND d.status != 'deleted') \
             FROM queues q ORDER BY q.is_main DESC, q.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(QueueRow {
                id: r.get(0)?,
                name: r.get(1)?,
                queue_type: r.get(2)?,
                is_main: r.get::<_, i64>(3)? != 0,
                schedule_type: r.get(4)?,
                start_at: r.get(5)?,
                stop_at: r.get(6)?,
                start_on_startup: r.get::<_, i64>(7)? != 0,
                on_complete_action: r.get(8)?,
                on_complete_program: r.get(9)?,
                sync_interval_minutes: r.get(10)?,
                last_sync_at: r.get(11)?,
                last_sync_changed: r.get(12)?,
                last_sync_unchanged: r.get(13)?,
                last_sync_removed: r.get(14)?,
                last_sync_failed: r.get(15)?,
                last_sync_skipped: r.get(16)?,
                running: false,
                file_count: r.get(17)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    pub fn get_queue(&self, id: &str) -> Result<Option<QueueRow>> {
        Ok(self.list_queues()?.into_iter().find(|q| q.id == id))
    }

    /// Create a custom download queue. Names must be unique and non-empty.
    pub fn create_queue(&self, name: &str) -> Result<QueueRow> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::InvalidState("Give the queue a name".to_string()));
        }
        let id = Self::new_id();
        let conn = self.lock_conn();
        match conn.execute(
            "INSERT INTO queues (id, name, type, is_main, schedule_type) \
             VALUES (?1, ?2, 'download', 0, 'one_time')",
            params![id, name],
        ) {
            Ok(_) => Ok(QueueRow {
                id,
                name: name.to_string(),
                queue_type: "download".to_string(),
                is_main: false,
                schedule_type: "one_time".to_string(),
                start_at: None,
                stop_at: None,
                start_on_startup: false,
                on_complete_action: "none".to_string(),
                on_complete_program: None,
                sync_interval_minutes: None,
                last_sync_at: None,
                last_sync_changed: 0,
                last_sync_unchanged: 0,
                last_sync_removed: 0,
                last_sync_failed: 0,
                last_sync_skipped: 0,
                running: false,
                file_count: 0,
            }),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(EngineError::InvalidState(format!(
                    "A queue called “{name}” already exists"
                )))
            }
            Err(e) => Err(EngineError::from(e)),
        }
    }

    /// Replace a queue's schedule and completion action. Empty strings
    /// clear the optional fields.
    ///
    /// A queue is either one-time or periodic, never both — that is
    /// what `schedule_type` stores, and the two shapes are validated
    /// against each other here: a periodic queue needs a check
    /// interval of at least `MIN_SYNC_INTERVAL_MINUTES` minutes
    /// (smaller values are rejected, not clamped, so a typo stays
    /// visible), and a one-time / unscheduled queue must not carry
    /// an interval at all. A periodic queue keeps its start/stop
    /// window: sync runs only inside it.
    #[allow(clippy::too_many_arguments)]
    pub fn update_queue(
        &self,
        id: &str,
        schedule_type: &str,
        start_at: Option<&str>,
        stop_at: Option<&str>,
        start_on_startup: bool,
        on_complete_action: &str,
        on_complete_program: Option<&str>,
        sync_interval_minutes: Option<i64>,
    ) -> Result<QueueRow> {
        if schedule_type != "one_time" && schedule_type != "none" && schedule_type != "periodic" {
            return Err(EngineError::InvalidState(format!(
                "Unknown schedule type: {schedule_type}"
            )));
        }
        // "none" clears the schedule (and any interval). The schema
        // only allows 'one_time'/'periodic', so an unscheduled queue
        // is stored as 'one_time' with no times — the scheduler skips
        // queues with no start time, and the UI reads that back as
        // "no schedule".
        let (schedule_type, start_at, stop_at, sync_interval_minutes) = if schedule_type == "none" {
            ("one_time", None, None, None)
        } else {
            (schedule_type, start_at, stop_at, sync_interval_minutes)
        };
        if schedule_type == "periodic" {
            match sync_interval_minutes {
                None => {
                    return Err(EngineError::InvalidState(
                        "Choose how often to check for changes — at least every 5 minutes".to_string(),
                    ))
                }
                Some(m) if m < MIN_SYNC_INTERVAL_MINUTES => {
                    return Err(EngineError::InvalidState(format!(
                        "Check for changes at least every {MIN_SYNC_INTERVAL_MINUTES} minutes — {m} minutes would probe the server too often"
                    )))
                }
                Some(m) if m > MAX_SYNC_INTERVAL_MINUTES => {
                    return Err(EngineError::InvalidState(format!(
                        "The check interval is too long — the maximum is {MAX_SYNC_INTERVAL_MINUTES} minutes (one week)"
                    )))
                }
                _ => {}
            }
        } else if sync_interval_minutes.is_some() {
            return Err(EngineError::InvalidState(
                "A check interval only applies to periodic synchronisation — choose “Repeat regularly” or clear the interval".to_string(),
            ));
        }
        // BUG-005 / BUG-006: validate the window before storing it.
        // Blank strings clear the field (the documented behaviour);
        // anything else must be a valid UTC ISO-8601 time, and a stop
        // time must be after the start time. Rejecting here means a
        // bad window can never be stored and silently never run.
        // The window rules are the same for one-time and periodic
        // queues: a periodic queue syncs only inside its window.
        let start_at = clean_time(start_at);
        let stop_at = clean_time(stop_at);
        if schedule_type == "one_time" || schedule_type == "periodic" {
            if let Some(start) = start_at {
                if parse_iso_to_unix(start).is_none() {
                    return Err(EngineError::InvalidState(
                        "The start time is not a valid date and time — use ISO format, e.g. 2026-11-14T20:30:00Z"
                            .to_string(),
                    ));
                }
            }
            if let Some(stop) = stop_at {
                if parse_iso_to_unix(stop).is_none() {
                    return Err(EngineError::InvalidState(
                        "The stop time is not a valid date and time — use ISO format, e.g. 2026-11-14T20:30:00Z"
                            .to_string(),
                    ));
                }
            }
            match (start_at, stop_at) {
                (None, Some(_)) => {
                    return Err(EngineError::InvalidState(
                        "Set a start time to use a stop time.".to_string(),
                    ))
                }
                (Some(start), Some(stop)) => {
                    // Both parsed above, so these unwraps cannot fail.
                    let start_unix = parse_iso_to_unix(start).unwrap();
                    let stop_unix = parse_iso_to_unix(stop).unwrap();
                    if stop_unix <= start_unix {
                        return Err(EngineError::InvalidState(
                            "The stop time must be after the start time.".to_string(),
                        ));
                    }
                }
                _ => {}
            }
        }
        // Completion actions validate through the one shared
        // helper (`crate::queue::validate_completion_action`) — the
        // site grabber stores the same actions on its projects, so
        // the messages must stay identical in both places.
        crate::queue::validate_completion_action(on_complete_action, on_complete_program)?;
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE queues SET schedule_type = ?1, start_at = ?2, stop_at = ?3, \
             start_on_startup = ?4, on_complete_action = ?5, on_complete_program = ?6, \
             sync_interval_minutes = ?7, updated_at = datetime('now') WHERE id = ?8",
            params![
                schedule_type,
                start_at,
                stop_at,
                if start_on_startup { 1 } else { 0 },
                on_complete_action,
                on_complete_program,
                sync_interval_minutes,
                id
            ],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        drop(conn);
        self.get_queue(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))
    }

    /// Put a download at the end of a queue. A finished download
    /// cannot be queued for a one-time drain: it would sit in the
    /// queue forever (the runner only unqueues files it finishes
    /// itself) and, with a completion action set, would arm that
    /// action after downloading nothing.
    ///
    /// A periodic synchronisation queue is the exception: its whole
    /// point is watching files that are already downloaded, so a
    /// finished download is exactly what belongs in it.
    pub fn add_to_queue(&self, download_id: &str, queue_id: &str) -> Result<()> {
        let status: Option<String> = {
            let conn = self.lock_conn();
            conn.query_row(
                "SELECT status FROM downloads WHERE id = ?1",
                params![download_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(EngineError::from)?
        };
        let queue_type: Option<String> = {
            let conn = self.lock_conn();
            conn.query_row(
                "SELECT schedule_type FROM queues WHERE id = ?1",
                params![queue_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(EngineError::from)?
        };
        match status.as_deref() {
            None | Some("deleted") => return Err(EngineError::NotFound(download_id.to_string())),
            Some("complete") if queue_type.as_deref() != Some("periodic") => {
                return Err(EngineError::InvalidState(
                    "This download is already finished, so it cannot be added to a queue".to_string(),
                ))
            }
            _ => {}
        }
        let conn = self.lock_conn();
        let queue_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM queues WHERE id = ?1",
            params![queue_id],
            |r| r.get(0),
        )?;
        if queue_exists == 0 {
            return Err(EngineError::NotFound(queue_id.to_string()));
        }
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(queue_position), -1) + 1 FROM downloads WHERE queue_id = ?1",
            params![queue_id],
            |r| r.get(0),
        )?;
        let changed = conn.execute(
            "UPDATE downloads SET queue_id = ?1, queue_position = ?2, updated_at = datetime('now') \
             WHERE id = ?3 AND status != 'deleted'",
            params![queue_id, next, download_id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(download_id.to_string()));
        }
        Ok(())
    }

    /// Take a download out of its queue (it stays in the main list).
    pub fn remove_from_queue(&self, download_id: &str) -> Result<()> {
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE downloads SET queue_id = NULL, queue_position = 0, updated_at = datetime('now') \
             WHERE id = ?1",
            params![download_id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(download_id.to_string()));
        }
        Ok(())
    }

    /// Files of one queue, in queue order.
    pub fn list_queue_files(&self, queue_id: &str) -> Result<Vec<DownloadRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, url, final_url, filename, save_path, category_id, queue_id, \
             queue_position, size_bytes, downloaded_bytes, status, speed_bps, eta_seconds, \
             etag, last_modified, error_message, created_at, completed_at, \
             speed_limit_kbps, quota_paused, scan_status, scan_detail, scan_scanned_at, accept_ranges, updated_at, grabber_project_id, description, active_connections, segment_count \
             FROM downloads WHERE queue_id = ?1 AND status != 'deleted' \
             ORDER BY queue_position ASC, created_at ASC",
        )?;
        let rows = stmt.query_map(params![queue_id], row_to_download)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// Move one file to a new position inside its queue (0-based).
    /// Positions are renumbered 0..n afterwards, so the order is always
    /// dense and unambiguous.
    pub fn reorder_queue(&self, queue_id: &str, download_id: &str, new_index: usize) -> Result<()> {
        let mut files = self.list_queue_files(queue_id)?;
        let pos = files
            .iter()
            .position(|d| d.id == download_id)
            .ok_or_else(|| EngineError::NotFound(download_id.to_string()))?;
        let row = files.remove(pos);
        let index = new_index.min(files.len());
        files.insert(index, row);
        let conn = self.lock_conn();
        for (i, d) in files.iter().enumerate() {
            conn.execute(
                "UPDATE downloads SET queue_position = ?1 WHERE id = ?2",
                params![i as i64, d.id],
            )?;
        }
        Ok(())
    }

    // -- downloads --------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn create_download(
        &self,
        url: &str,
        final_url: Option<&str>,
        filename: &str,
        save_path: &str,
        category_id: &str,
        size_bytes: Option<i64>,
        etag: Option<&str>,
        last_modified: Option<&str>,
        accept_ranges: Option<bool>,
    ) -> Result<String> {
        let id = Self::new_id();
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO downloads \
             (id, url, final_url, filename, save_path, category_id, size_bytes, etag, last_modified, accept_ranges, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued')",
            params![id, url, final_url, filename, save_path, category_id, size_bytes, etag, last_modified, accept_ranges.map(|b| b as i64)],
        )?;
        Ok(id)
    }

    /// True when a live (not deleted) download already reserves this
    /// final path. Adding the same link twice must not hand both rows
    /// the same file: the second download would race the first and
    /// overwrite it on completion.
    pub fn is_save_path_used(&self, save_path: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE save_path = ?1 AND status != 'deleted'",
            params![save_path],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// True when a live (not deleted) download already has this URL
    /// (either as the URL as added, or the final URL after redirects).
    /// Download All flags such links and skips them by default.
    pub fn url_exists(&self, url: &str) -> Result<bool> {
        let conn = self.lock_conn();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloads \
             WHERE (url = ?1 OR final_url = ?1) AND status != 'deleted'",
            params![url],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn get_download(&self, id: &str) -> Result<Option<DownloadRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, url, final_url, filename, save_path, category_id, queue_id, \
             queue_position, size_bytes, downloaded_bytes, status, speed_bps, eta_seconds, \
             etag, last_modified, error_message, created_at, completed_at, \
             speed_limit_kbps, quota_paused, scan_status, scan_detail, scan_scanned_at, accept_ranges, updated_at, grabber_project_id, description, active_connections, segment_count \
             FROM downloads WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_download)?;
        match rows.next() {
            Some(r) => {
                let mut row = r?;
                mark_missing(&mut row);
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, url, final_url, filename, save_path, category_id, queue_id, \
             queue_position, size_bytes, downloaded_bytes, status, speed_bps, eta_seconds, \
             etag, last_modified, error_message, created_at, completed_at, \
             speed_limit_kbps, quota_paused, scan_status, scan_detail, scan_scanned_at, accept_ranges, updated_at, grabber_project_id, description, active_connections, segment_count \
             FROM downloads WHERE status != 'deleted' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_download)?;
        let mut rows = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(EngineError::from)?;
        for row in &mut rows {
            mark_missing(row);
        }
        Ok(rows)
    }

    pub fn set_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET status = ?1, error_message = ?2, updated_at = datetime('now') \
             WHERE id = ?3",
            params![status, error, id],
        )?;
        Ok(())
    }

    /// Round 67 — set the Add URL dialog's Description note on a
    /// download (`None` / empty clears it). Called right after
    /// `add_download`, so the description never has to ride the
    /// add path's signature (and its many call sites).
    pub fn set_download_description(&self, id: &str, description: Option<&str>) -> Result<()> {
        let conn = self.lock_conn();
        let description = description.map(str::trim).filter(|d| !d.is_empty());
        conn.execute(
            "UPDATE downloads SET description = ?1 WHERE id = ?2",
            params![description, id],
        )?;
        Ok(())
    }

    /// Round 55 (defect 1/4): startup reconciliation. A row left
    /// in `downloading` by a previous run — a crash, a killed
    /// process, a wedged run — is a zombie: no run exists for it
    /// in this process, so it could never finish, and Resume
    /// would refuse it as "already running". Reset such rows to
    /// `paused` with a message, keeping their bytes so the user
    /// can resume. Called once at startup (the Tauri shell's
    /// `build_manager`); returns the number of rows reset.
    pub fn reconcile_stuck_downloads(&self) -> Result<usize> {
        let conn = self.lock_conn();
        let n = conn.execute(
            "UPDATE downloads SET status = 'paused', \
             error_message = 'Interrupted — resume to continue', \
             updated_at = datetime('now') WHERE status = 'downloading'",
            [],
        )?;
        Ok(n)
    }

    pub fn mark_complete(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET status = 'complete', completed_at = datetime('now'), \
             updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
        // Download history (Round 53): a finished download is
        // recorded here, in the same lock — the row is read with
        // the already-locked connection (get_download would lock
        // the same mutex and deadlock). This is the ONLY place
        // history is recorded, so failed / cancelled / paused rows
        // never land in it. INSERT OR REPLACE + UNIQUE(download_id)
        // makes a second completion of the same download a no-op.
        let row = conn
            .query_row(
                "SELECT id, url, final_url, filename, save_path, category_id, queue_id, \
                 queue_position, size_bytes, downloaded_bytes, status, speed_bps, eta_seconds, \
                 etag, last_modified, error_message, created_at, completed_at, \
                 speed_limit_kbps, quota_paused, scan_status, scan_detail, scan_scanned_at, accept_ranges, updated_at, grabber_project_id, description, active_connections, segment_count \
                 FROM downloads WHERE id = ?1",
                params![id],
                row_to_download,
            )
            .optional()?;
        if let Some(row) = row {
            Self::record_history_locked(&conn, &row)?;
        }
        Ok(())
    }

    // -- download history (Round 53) --------------------------------------

    /// Record a finished download in the history. Idempotent:
    /// `INSERT OR REPLACE` on the UNIQUE(download_id) keeps one
    /// row per download no matter how often it is called. The
    /// folder is the parent directory of the finished file; the
    /// completion time is the row's own `completed_at` when it has
    /// one, otherwise now.
    pub fn record_history(&self, row: &DownloadRow) -> Result<()> {
        let conn = self.lock_conn();
        Self::record_history_locked(&conn, row)
    }

    fn record_history_locked(conn: &Connection, row: &DownloadRow) -> Result<()> {
        let folder = Path::new(&row.save_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        conn.execute(
            "INSERT OR REPLACE INTO download_history \
             (id, download_id, filename, url, final_url, size_bytes, folder, category_id, completed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE(?9, datetime('now')))",
            params![
                Self::new_id(),
                row.id,
                row.filename,
                row.url,
                row.final_url,
                row.size_bytes,
                folder,
                row.category_id,
                row.completed_at,
            ],
        )?;
        Ok(())
    }

    /// History rows, newest first. An empty query matches
    /// everything; otherwise the filename or the URL must contain
    /// the query — `%` and `_` in the query are escaped so they
    /// match literally, never as LIKE wildcards.
    pub fn list_history(&self, query: &str) -> Result<Vec<HistoryRow>> {
        let conn = self.lock_conn();
        let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = conn.prepare(
            "SELECT id, download_id, filename, url, final_url, size_bytes, folder, category_id, completed_at \
             FROM download_history \
             WHERE (?1 = '' OR filename LIKE ?2 ESCAPE '\\' OR url LIKE ?2 ESCAPE '\\') \
             ORDER BY completed_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![query, pattern], row_to_history)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// One history row by its history id, or `None`.
    pub fn get_history_entry(&self, id: &str) -> Result<Option<HistoryRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, download_id, filename, url, final_url, size_bytes, folder, category_id, completed_at \
             FROM download_history WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_history)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Empty the whole history. The main downloads table is
    /// untouched — clearing history never deletes a download.
    pub fn clear_history(&self) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM download_history", [])?;
        Ok(())
    }

    // -- antivirus scan result (S14) --------------------------------------

    /// Record one scan's result on the download row. The download
    /// itself is untouched (still `complete`) — the scan is a
    /// report about the finished file, never a second status.
    pub fn set_scan_result(&self, id: &str, result: &crate::antivirus::ScanResult) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET scan_status = ?1, scan_detail = ?2, scan_scanned_at = ?3, \
             updated_at = datetime('now') WHERE id = ?4",
            params![result.status, result.detail, result.scanned_at, id],
        )?;
        Ok(())
    }

    /// The stored scan result for a download, or `None` when it
    /// was never scanned (or the download does not exist).
    pub fn get_scan_result(&self, id: &str) -> Result<Option<crate::antivirus::ScanResult>> {
        let row = self.get_download(id)?;
        Ok(row.and_then(|r| {
            r.scan_status.map(|status| crate::antivirus::ScanResult {
                status,
                detail: r.scan_detail,
                scanned_at: r.scan_scanned_at,
            })
        }))
    }

    /// Move a download's recorded file path (used by quarantine:
    /// the file itself is moved by the caller, this only updates
    /// the row so the table keeps pointing at the real file).
    pub fn update_save_path(&self, id: &str, save_path: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET save_path = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![save_path, id],
        )?;
        Ok(())
    }

    /// Persist the current byte counts (called every ~2 s and on pause).
    /// Round 72 (D): also persists the live connection/part counts
    /// for the main window (`active_connections`, `segment_count`).
    pub fn checkpoint(
        &self,
        id: &str,
        downloaded: i64,
        speed_bps: i64,
        eta: Option<i64>,
        active_connections: i64,
        segment_count: i64,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET downloaded_bytes = ?1, speed_bps = ?2, eta_seconds = ?3, \
             active_connections = ?4, segment_count = ?5, \
             updated_at = datetime('now') WHERE id = ?6",
            params![
                downloaded,
                speed_bps,
                eta,
                active_connections,
                segment_count,
                id
            ],
        )?;
        Ok(())
    }

    // -- segments ---------------------------------------------------------

    pub fn create_segments(&self, download_id: &str, ranges: &[(i64, Option<i64>)]) -> Result<Vec<SegmentRow>> {
        let conn = self.lock_conn();
        let mut out = Vec::with_capacity(ranges.len());
        for (index, (start, end)) in ranges.iter().enumerate() {
            let id = Self::new_id();
            conn.execute(
                "INSERT INTO segments (id, download_id, segment_index, start_byte, end_byte, status) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
                params![id, download_id, index as i64, start, end],
            )?;
            out.push(SegmentRow {
                id,
                download_id: download_id.to_string(),
                segment_index: index as i64,
                start_byte: *start,
                end_byte: *end,
                downloaded_bytes: 0,
                status: "pending".to_string(),
            });
        }
        Ok(out)
    }

    pub fn get_segments(&self, download_id: &str) -> Result<Vec<SegmentRow>> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, download_id, segment_index, start_byte, end_byte, downloaded_bytes, status \
             FROM segments WHERE download_id = ?1 ORDER BY segment_index",
        )?;
        let rows = stmt.query_map(params![download_id], |r| {
            Ok(SegmentRow {
                id: r.get(0)?,
                download_id: r.get(1)?,
                segment_index: r.get(2)?,
                start_byte: r.get(3)?,
                end_byte: r.get(4)?,
                downloaded_bytes: r.get(5)?,
                status: r.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(EngineError::from)
    }

    /// Update one segment's progress (and, when it finishes, its status).
    pub fn update_segment(&self, id: &str, downloaded: i64, status: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE segments SET downloaded_bytes = ?1, status = ?2, updated_at = datetime('now') \
             WHERE id = ?3",
            params![downloaded, status, id],
        )?;
        Ok(())
    }

    /// Split a segment in two for the dynamic-split rule.
    ///
    /// The existing segment is shortened first (`end_byte = mid - 1`) and
    /// only then is the new segment inserted — the schema's no-overlap
    /// trigger aborts the insert otherwise.
    pub fn split_segment(
        &self,
        segment: &SegmentRow,
        mid: i64,
    ) -> Result<SegmentRow> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE segments SET end_byte = ?1 WHERE id = ?2",
            params![mid - 1, segment.id],
        )?;
        let id = Self::new_id();
        // New index = one past the current maximum, so repeated splits
        // of the same segment can never collide on the unique index.
        let next_index: i64 = conn.query_row(
            "SELECT COALESCE(MAX(segment_index), -1) + 1 FROM segments WHERE download_id = ?1",
            params![segment.download_id],
            |r| r.get(0),
        )?;
        conn.execute(
            "INSERT INTO segments (id, download_id, segment_index, start_byte, end_byte, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            params![id, segment.download_id, next_index, mid, segment.end_byte],
        )?;
        Ok(SegmentRow {
            id,
            download_id: segment.download_id.clone(),
            segment_index: next_index,
            start_byte: mid,
            end_byte: segment.end_byte,
            downloaded_bytes: 0,
            status: "pending".to_string(),
        })
    }

    /// Delete all segments of a download (used when a resume finds the
    /// remote file changed and the download must restart from zero).
    pub fn delete_segments(&self, download_id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM segments WHERE download_id = ?1", params![download_id])?;
        Ok(())
    }

    /// Delete ONE segment row (Round 73 — patient-tail coalesce):
    /// the row is an unstarted pending segment that was merged
    /// into its neighbour's range, so it must not survive to a
    /// later pause/resume (which would reload it alongside the
    /// merged range and overlap it).
    pub fn delete_segment(&self, id: &str) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute("DELETE FROM segments WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Change one segment's byte range in place (Round 73 —
    /// patient-tail coalesce): the surviving segment absorbs the
    /// ranges of the pending segments merged into it. Callers
    /// delete those rows first, so the widened range never
    /// overlaps a live row (the schema's no-overlap trigger
    /// would abort the update otherwise).
    pub fn update_segment_range(
        &self,
        id: &str,
        start_byte: i64,
        end_byte: Option<i64>,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE segments SET start_byte = ?1, end_byte = ?2, updated_at = datetime('now') \
             WHERE id = ?3",
            params![start_byte, end_byte, id],
        )?;
        Ok(())
    }

    /// Reset a download to zero bytes (file changed on the server).
    /// Round 72 (D): the live connection/part counts reset too —
    /// a reset row is not downloading, so both read 0 until its
    /// next run's checkpoint writes them.
    pub fn reset_download(&self, id: &str, size: Option<i64>, etag: Option<&str>, last_modified: Option<&str>) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET downloaded_bytes = 0, speed_bps = 0, eta_seconds = NULL, \
             active_connections = 0, segment_count = 0, \
             error_message = NULL, size_bytes = ?1, etag = ?2, last_modified = ?3, \
             status = 'queued', updated_at = datetime('now') WHERE id = ?4",
            params![size, etag, last_modified, id],
        )?;
        Ok(())
    }

    /// Remember what the server last said about a file, without
    /// touching its bytes or status. Used by a sync run when the
    /// probe shows the file is unchanged but the stored validator
    /// was missing (e.g. a download made before sync existed, or a
    /// server that only started sending an ETag) — the next sync
    /// can then compare against something real.
    pub fn update_known_remote(
        &self,
        id: &str,
        size: Option<i64>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE downloads SET size_bytes = COALESCE(?1, size_bytes), \
             etag = COALESCE(?2, etag), last_modified = COALESCE(?3, last_modified), \
             updated_at = datetime('now') WHERE id = ?4",
            params![size, etag, last_modified, id],
        )?;
        Ok(())
    }

    /// Point a download at a new address (Round 51, refresh
    /// download address): the URL and what the new probe learned
    /// about the file, without touching the downloaded bytes, the
    /// segments or the status. The caller decides whether the
    /// bytes stay valid (same file) or must be dropped first (a
    /// changed file, via `reset_download`). The old error is
    /// cleared — the new address is a fresh start, not a
    /// continuation of the failure.
    pub fn update_download_address(
        &self,
        id: &str,
        url: &str,
        final_url: Option<&str>,
        size: Option<i64>,
        etag: Option<&str>,
        last_modified: Option<&str>,
        accept_ranges: bool,
    ) -> Result<()> {
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE downloads SET url = ?1, final_url = ?2, size_bytes = ?3, \
             etag = ?4, last_modified = ?5, accept_ranges = ?6, error_message = NULL, \
             updated_at = datetime('now') WHERE id = ?7",
            params![url, final_url, size, etag, last_modified, accept_ranges as i64, id],
        )?;
        if changed == 0 {
            return Err(EngineError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Record a finished sync run on its queue: when it ran and how
    /// many files landed in each outcome. The UI reads these back
    /// through `list_queues` / `get_queue` as the queue's
    /// "last sync" + "changed files" status.
    pub fn record_sync(
        &self,
        queue_id: &str,
        last_sync_at: &str,
        changed: i64,
        unchanged: i64,
        removed: i64,
        failed: i64,
        skipped: i64,
    ) -> Result<()> {
        let conn = self.lock_conn();
        let changed_rows = conn.execute(
            "UPDATE queues SET last_sync_at = ?1, last_sync_changed = ?2, \
             last_sync_unchanged = ?3, last_sync_removed = ?4, \
             last_sync_failed = ?5, last_sync_skipped = ?6, \
             updated_at = datetime('now') WHERE id = ?7",
            params![last_sync_at, changed, unchanged, removed, failed, skipped, queue_id],
        )?;
        if changed_rows == 0 {
            return Err(EngineError::NotFound(queue_id.to_string()));
        }
        Ok(())
    }
}

fn row_to_grabber_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<GrabberProjectRow> {
    let filters_json: String = r.get(3)?;
    let filters: GrabberFilters =
        serde_json::from_str(&filters_json).unwrap_or_default();
    let schedule_json: Option<String> = r.get(5)?;
    let schedule_start_at = schedule_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("start_at").and_then(|s| s.as_str()).map(str::to_string));
    let matched_json: String = r.get(12)?;
    let matched_files: Vec<MatchedFile> =
        serde_json::from_str(&matched_json).unwrap_or_default();
    // Pre-Milestone-4 rows have no queue/action columns (the
    // migration adds them with defaults, but read defensively):
    // a missing queue reads back as the main queue, a missing
    // action as "none".
    let queue_id: Option<String> = r.get(14)?;
    let on_complete_action: Option<String> = r.get(15)?;
    Ok(GrabberProjectRow {
        id: r.get(0)?,
        name: r.get(1)?,
        start_url: r.get(2)?,
        filters,
        mode: r.get(4)?,
        schedule_start_at,
        queue_id: queue_id
            .filter(|q| !q.trim().is_empty())
            .unwrap_or_else(|| crate::grabber::MAIN_QUEUE_ID.to_string()),
        on_complete_action: on_complete_action.unwrap_or_else(|| "none".to_string()),
        on_complete_program: r.get(16)?,
        status: r.get(6)?,
        pages_explored: r.get(7)?,
        files_matched: r.get(8)?,
        files_downloaded: r.get(9)?,
        files_failed: r.get(10)?,
        error_message: r.get(11)?,
        matched_files,
        created_at: r.get(13)?,
    })
}

fn row_to_pending_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<PendingLinkRow> {
    Ok(PendingLinkRow {
        id: r.get(0)?,
        url: r.get(1)?,
        referrer: r.get(2)?,
        source: r.get(3)?,
        status: r.get(4)?,
        download_id: r.get(5)?,
        error: r.get(6)?,
        cookies: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn row_to_history(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRow> {
    Ok(HistoryRow {
        id: r.get(0)?,
        download_id: r.get(1)?,
        filename: r.get(2)?,
        url: r.get(3)?,
        final_url: r.get(4)?,
        size_bytes: r.get(5)?,
        folder: r.get(6)?,
        category_id: r.get(7)?,
        completed_at: r.get(8)?,
    })
}

fn row_to_download(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRow> {
    Ok(DownloadRow {
        id: r.get(0)?,
        url: r.get(1)?,
        final_url: r.get(2)?,
        filename: r.get(3)?,
        save_path: r.get(4)?,
        category_id: r.get(5)?,
        queue_id: r.get(6)?,
        queue_position: r.get(7)?,
        size_bytes: r.get(8)?,
        downloaded_bytes: r.get(9)?,
        status: r.get(10)?,
        speed_bps: r.get(11)?,
        eta_seconds: r.get(12)?,
        etag: r.get(13)?,
        last_modified: r.get(14)?,
        error_message: r.get(15)?,
        created_at: r.get(16)?,
        completed_at: r.get(17)?,
        speed_limit_kbps: r.get(18)?,
        quota_paused: r.get::<_, i64>(19)? != 0,
        scan_status: r.get(20)?,
        scan_detail: r.get(21)?,
        scan_scanned_at: r.get(22)?,
        accept_ranges: r.get::<_, Option<i64>>(23)?.map(|v| v != 0),
        updated_at: r.get(24)?,
        grabber_project_id: r.get(25)?,
        description: r.get(26)?,
        active_connections: r.get(27)?,
        segment_count: r.get(28)?,
        // Round 71 (D4) — not a column; `mark_missing` sets it
        // after the read (it needs a filesystem check, and the
        // row mapper must stay a pure DB read).
        file_missing: false,
    })
}

/// Round 71 (D4) — mark Completed downloads whose file no
/// longer exists on disk. Called on every `list_downloads` /
/// `get_download` read (a `stat` per complete row per poll is
/// nothing, and a file deleted mid-session must show up at the
/// next poll, not only at startup). Only `complete` rows are
/// checked: every other status is still downloading, paused or
/// failed, so a missing file there is expected, not stale.
fn mark_missing(row: &mut DownloadRow) {
    row.file_missing = row.status == "complete" && !std::path::Path::new(&row.save_path).exists();
}
