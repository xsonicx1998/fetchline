//! Segmented download manager.
//!
//! Behaviour (per `replica/architecture.md`):
//! 1. Probe, then split the file into segments downloaded in parallel
//!    with HTTP `Range` requests.
//! 2. **Dynamic split:** when a connection frees up, the largest
//!    unfinished segment is halved and the second half becomes a new
//!    segment (documented simplification: splits happen in the supervisor
//!    loop, at most once per 250 ms tick, only when a segment still has
//!    more than 256 KiB left — the original splits continuously; the
//!    observable behaviour, faster completion on multi-connection
//!    servers, is the same).
//! 3. Bytes are written to a `.part` file at exact offsets. The file is
//!    pre-allocated when the size is known and renamed atomically on
//!    completion, so a crash can never leave a "complete" partial file.
//! 4. Segment progress is checkpointed to SQLite every ~1 s and on
//!    pause, so resume continues from the last persisted byte, and
//!    the main window's 1 s poll always sees live progress — a
//!    download that finishes in ~1 s still reports at least once
//!    (Round 55: the 2 s interval used to hide whole fast
//!    downloads). On resume the server is re-probed; if the size
//!    or `ETag` changed, the download restarts from zero instead
//!    of stitching a wrong file.
//! 5. Each segment retries with exponential backoff (1 s, 2 s, 4 s, 8 s,
//!    16 s); after 5 attempts the download goes to `error` with the
//!    server's message.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::RANGE;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio::task::JoinSet;

use crate::clipboard::{extract_download_link, ClipboardMonitor, ClipboardOffer};
use crate::error::{EngineError, Result};
use crate::antivirus::{run_scan, test_scanner, ScanResult, SCAN_INFECTED};
use crate::grabber::{
    is_page_url, robots_disallows, same_host, type_matches, validate_grabber_input,
    GrabberFilters, GrabberProjectRow, MatchedFile, MAIN_QUEUE_ID, MAX_FILE_ATTEMPTS,
    POLITE_DELAY,
};
use crate::limiter::RateLimiter;
use crate::links::{classify_kind, extract_links, filename_from_link_url, LinkKind, PageLink};
use crate::options::{
    effective_completion_action, SiteLoginInput, SiteLoginRow, SoundEventRecord,
    SoundSetting, SoundSettingInput,
};
use crate::native_host::{cookies_from_json, cookies_to_header};
use crate::probe::{probe_with_cookies, probe_with_proxy, ProbeInfo};
use crate::proxy::{
    basic_proxy_auth_header, build_client, build_segment_client, offered_proxy_auth_schemes,
    proxy_auth_order, resolve_proxy_auth, ProxyAuthScheme, ProxyConfig, ProxyConfigInput,
    ProxyConfigRow, PROXY_AUTH_BASIC,
};
use crate::queue::PendingAction;
use crate::quota::{window_end_unix, window_expired, QuotaGate, QuotaStatus, QUOTA_MB_BYTES};
use crate::store::{CategoryRow, DownloadRow, HistoryRow, QueueRow, SegmentRow, Settings, Store};
use crate::timeutil::{now_unix, parse_iso_to_unix, unix_to_iso};
use crate::update::{
    fetch_manifest, result_from_manifest, weekly_check_due, UpdateCheckResult,
};

const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Round 55 (defect 1): a body stream that delivers no chunk for
/// this long is stalled, not slow — the segment fails and retries
/// instead of hanging forever. 30 s is far above any healthy
/// trickle (chunks arrive continuously even at a few KB/s) and far
/// below the minutes a wedged download used to sit at 0%.
const SEGMENT_STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Round 55 (defect 1): backstop for a run hung anywhere the
/// per-chunk watchdog cannot see (FTP / Kerberos / NTLM reads, a
/// task stuck outside its stream loop) — a run that makes no
/// progress at all for this long fails with a clear message
/// instead of leaving status `downloading` forever. 180 s covers
/// the slowest legitimate case (a 1 KB/s cap on a 64 KiB chunk
/// takes ~64 s between writes, plus connect time) with margin,
/// and the check is suppressed while a segment waits out its
/// retry backoff, so backoffs never trip it.
const RUN_STALL_TIMEOUT: Duration = Duration::from_secs(180);
const MIN_SPLIT_REMAINING: u64 = 256 * 1024;
const INITIAL_SEGMENTS: usize = 4;
const MIN_SIZE_TO_SEGMENT: u64 = 1024 * 1024;

/// Epoch milliseconds — the independent run watchdog's clock
/// (§7.2, Round 56). Absolute (not run-relative) so a stale
/// watchdog task from an earlier run of the same download can
/// never mistake its own start time for the current run's
/// progress: it always reads the current run's timestamp from
/// the `running` map entry.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One segment's live progress, as reported to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentProgress {
    pub segment_index: i64,
    pub start_byte: i64,
    pub end_byte: Option<i64>,
    pub downloaded_bytes: i64,
    pub status: String,
}

/// A progress snapshot, emitted on `download-progress` while a download runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub id: String,
    pub filename: String,
    pub downloaded_bytes: u64,
    pub size_bytes: Option<u64>,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub status: String,
    pub segments: Vec<SegmentProgress>,
    /// Set while a segment is waiting out its retry backoff (S03):
    /// the dialog shows "waiting for retry" with a countdown from
    /// this, instead of a silent stall. `None` the rest of the time.
    /// `#[serde(default)]` so events written before S03 still read.
    #[serde(default)]
    pub retry: Option<RetryInfo>,
    /// The antivirus scan result (S14), once known: `None` while
    /// the download is still running (no scan has happened yet),
    /// `Some` on the completion event and on the later event the
    /// detached scan emits when it finishes. `#[serde(default)]`
    /// so events written before S14 still read.
    #[serde(default)]
    pub scan: Option<ScanResult>,
}

/// One segment's retry wait, as the progress dialog shows it (S03).
/// `attempt` counts the failures so far (1 = the first failure);
/// `max_attempts` is the total attempts the settings allow, so the
/// next try is attempt `attempt + 1`. `retry_in_secs` counts down
/// from the engine's backoff deadline — it is recomputed on every
/// progress event, so the dialog's countdown is driven by the
/// engine, not by a UI timer that could drift from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryInfo {
    pub attempt: u32,
    pub max_attempts: u32,
    pub retry_in_secs: u64,
    pub error: String,
}

/// One line of a batch that could not be added, with its 1-based line
/// number so the UI can point at it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidLine {
    pub line: usize,
    pub url: String,
    pub error: String,
}

/// Result of adding a batch (S10) or a Download All selection (S07):
/// the ids that were created, and the lines that were skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub created: Vec<String>,
    pub invalid: Vec<InvalidLine>,
}

/// One file's outcome in a sync run. `status` is one of:
/// `unchanged` (probe matches what is stored — nothing downloaded),
/// `changed` (remote file differs — re-downloaded, byte-identical),
/// `removed` (the server answers 404 — the file is gone, reported
/// rather than silently dropped), `failed` (the probe or the
/// re-download failed — retried at the next sync, never in a loop),
/// `skipped` (excluded site, or a file that is not finished yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFileResult {
    pub download_id: String,
    pub filename: String,
    pub status: String,
    pub message: Option<String>,
}

/// The outcome of one sync run over a periodic queue: per-file
/// results plus counts, so the UI can show "last sync" and "changed
/// files" without re-reading every file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub queue_id: String,
    pub changed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub files: Vec<SyncFileResult>,
}

/// Result of draining the browser-extension pending-links queue
/// (S13): one `(pending_link_id, download_id)` pair per accepted
/// link, and one `(pending_link_id, reason)` pair per rejected link.
/// A drain never deletes rows — each row is marked accepted or
/// rejected in place, so the history stays visible. `start_failed`
/// (Milestone 2) is one `(pending_link_id, reason)` pair per
/// accepted link whose download could not be started — the download
/// itself exists and stays queued, so this is not a rejection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainSummary {
    pub accepted: Vec<(String, String)>,
    pub rejected: Vec<(String, String)>,
    #[serde(default)]
    pub start_failed: Vec<(String, String)>,
}

/// Outcome of `refresh_download_address` (Round 51): either the
/// row now points at the new address and the download was started
/// again, or the new address serves a different file and nothing
/// was changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RefreshAddressResult {
    /// The new address was verified as the same file (or the user
    /// confirmed a different file): the row points at it and the
    /// download is running again. `kept_bytes` is the progress
    /// carried over — 0 when a confirmed different file restarted
    /// from zero.
    Switched {
        kept_bytes: i64,
        size_bytes: Option<i64>,
    },
    /// The new address serves a different file: nothing was
    /// changed — the old address, the downloaded bytes and the
    /// segments are all still there. The dialog shows the new
    /// file's facts and only switches on an explicit confirm
    /// (which restarts from zero).
    Mismatch {
        size_bytes: Option<i64>,
        etag: Option<String>,
        filename: Option<String>,
    },
}

/// Mutable per-segment state shared between its task and the supervisor.
struct SegState {
    row: SegmentRow,
    /// Bytes already written, counted from `row.start_byte`.
    done: u64,
    /// Inclusive end byte the task must stop at; shrinks on a dynamic split.
    end: Option<u64>,
}

struct Running {
    pause: Arc<AtomicBool>,
    /// The run task's handle, shared with the thread watchdog
    /// (§9.2f, Round 57): an `Arc` so both the `running` map entry
    /// and the watchdog's registry entry can abort the same task.
    handle: Arc<tokio::task::JoinHandle<()>>,
    /// The run's retry wait (S03): a segment that failed writes its
    /// backoff deadline here, the supervisor reads it into every
    /// progress event, and "Retry now" sets the flag to cut the wait
    /// short. One slot per download — the last writer wins when
    /// several segments wait at once, which is the honest display:
    /// the download is waiting for a retry either way.
    retry: Arc<RetrySlot>,
    /// This download's own speed cap (S12): a second token bucket
    /// alongside the global one, so the effective rate is the min
    /// of the two. Unlimited when the download has no cap — the
    /// bucket then never waits, exactly like the global one at 0.
    per_download_limiter: Arc<RateLimiter>,
    /// Epoch millis of the last byte progress, written by the
    /// supervisor loop in `run_inner` whenever the downloaded
    /// total grows and read by the independent run watchdog
    /// (§7.2, Round 56). It lives in the map entry — not in a
    /// variable captured by the run task — so the watchdog always
    /// reads the CURRENT run's timestamp: a stale watchdog from
    /// an earlier run of the same download can never abort a
    /// newer run that is making progress.
    last_progress_ms: Arc<AtomicU64>,
}

/// What the truly-independent thread watchdog (§9.2f, Round 57)
/// watches for one download: the same shared state the tokio run
/// watchdog reads, but owned by a plain `std::thread` instead of
/// a tokio task — so it keeps running when the tokio runtime
/// itself is wedged. One entry per running download, in
/// `DownloadManager::thread_watchdogs`; the entry is removed when
/// the run ends (see `run`), when the tokio watchdog fires, or
/// when this thread fires, so a stale entry can never abort a
/// newer run of the same download (the thread checks
/// `Arc::ptr_eq` against the CURRENT entry before acting).
struct ThreadWatchEntry {
    last_progress_ms: Arc<AtomicU64>,
    retry: Arc<RetrySlot>,
    pause: Arc<AtomicBool>,
    handle: Arc<tokio::task::JoinHandle<()>>,
}

/// One retry wait: the backoff deadline of a segment that failed,
/// plus the flag "Retry now" sets. The deadline (not a remaining
/// count) is what is stored, so the countdown the dialog shows is
/// recomputed from the engine's clock on every progress event.
struct RetryWait {
    attempt: u32,
    max_attempts: u32,
    error: String,
    retry_at: Instant,
}

struct RetrySlot {
    wait: StdMutex<Option<RetryWait>>,
    retry_now: AtomicBool,
}

impl RetrySlot {
    fn new() -> Self {
        Self {
            wait: StdMutex::new(None),
            retry_now: AtomicBool::new(false),
        }
    }

    /// Record that a segment failed and will try again at the end of
    /// its backoff. The backoff itself is the EXISTING rule
    /// (`1 << attempt`, capped at 16 s) — this only publishes it.
    fn set_wait(&self, attempt: u32, max_attempts: u32, error: &str, backoff: Duration) {
        *self.wait.lock().unwrap() = Some(RetryWait {
            attempt,
            max_attempts,
            error: error.to_string(),
            retry_at: Instant::now() + backoff,
        });
    }

    /// The wait is over (the segment retried, paused, or finished):
    /// nothing is waiting any more.
    fn clear_wait(&self) {
        *self.wait.lock().unwrap() = None;
    }

    /// The countdown as the dialog shows it, or `None` when no
    /// segment is waiting. The seconds count down from the stored
    /// deadline (rounded up, so 1.9 s still shows 2 s).
    fn info(&self) -> Option<RetryInfo> {
        let wait = self.wait.lock().unwrap();
        let w = wait.as_ref()?;
        let remaining = w.retry_at.saturating_duration_since(Instant::now());
        Some(RetryInfo {
            attempt: w.attempt,
            max_attempts: w.max_attempts,
            retry_in_secs: remaining.as_millis().div_ceil(1000) as u64,
            error: w.error.clone(),
        })
    }
}

pub struct DownloadManager {
    /// The HTTP client. Rebuilt (behind the lock) whenever the proxy
    /// or the connection timeout changes, so a settings save takes
    /// effect without restarting the app — or any download.
    /// Round 55: this is the short-request client (probe, page
    /// fetch, update check) built by `build_client` — it carries
    /// the 60 s total timeout. Segment body streams use
    /// `segment_client` below instead: a slow download
    /// legitimately outlives any total budget.
    client: RwLock<Arc<Client>>,
    /// The segment client (Round 55): connect timeout only, no
    /// total timeout. Stalls are caught per chunk
    /// (`SEGMENT_STALL_TIMEOUT`) and by the run watchdog
    /// (`RUN_STALL_TIMEOUT`) instead of a total budget.
    segment_client: RwLock<Arc<Client>>,
    store: Arc<Store>,
    /// Connections per download (settings.max_connections, 1–16).
    /// Atomic so a settings save changes the segment count of the
    /// NEXT download immediately; running downloads keep theirs.
    max_connections: AtomicUsize,
    /// Segment retries (settings.max_retries, 0–10).
    max_retries: AtomicU32,
    /// Global speed cap, shared by every running download (S12).
    limiter: Arc<RateLimiter>,
    events: broadcast::Sender<ProgressEvent>,
    /// The latest progress event per download (S03 progress
    /// window, 2026-11-15). The frontend's `download-progress`
    /// listener never fires in the built app (BUG-044: no Tauri
    /// capability grants `plugin:event|listen`), so the window
    /// polls this snapshot instead of listening — the same event
    /// the Rust forwarder broadcasts, kept here as well. Entries
    /// are kept for the session (the completion event is what the
    /// window shows after a download finishes); one small event
    /// per download that ever ran, so the map stays bounded by
    /// the download list. A stale entry only ever shows old
    /// numbers.
    last_progress: Arc<StdMutex<HashMap<String, ProgressEvent>>>,
    running: AsyncMutex<HashMap<String, Running>>,
    /// Truly-independent thread watchdogs (§9.2f, Round 57): one
    /// `ThreadWatchEntry` per running download, watched by a plain
    /// `std::thread` — NOT a tokio task — so it keeps running when
    /// the tokio runtime itself is wedged (the Round 56 tokio
    /// watchdog is a task on the same runtime it watches, and
    /// freezes with it). Keyed by download id; the thread exits
    /// when its entry is removed or replaced.
    thread_watchdogs: StdMutex<HashMap<String, Arc<ThreadWatchEntry>>>,
    /// Stop flags of queues currently being drained, by queue id.
    /// The runner task detaches on spawn and removes its own entry
    /// when it ends, so no handle needs keeping.
    queue_runs: AsyncMutex<HashMap<String, Arc<AtomicBool>>>,
    /// Stop flags of periodic sync queues currently syncing, by
    /// queue id. Same pattern as `queue_runs`: the sync task
    /// detaches on spawn (from the 30 s tick) and removes its own
    /// entry when it ends. A manual "Sync now" runs inline instead,
    /// so the caller can await its result.
    sync_runs: AsyncMutex<HashMap<String, Arc<AtomicBool>>>,
    /// One-time queues whose scheduled run already happened this
    /// session — the scheduler must not start them again on later ticks.
    scheduled_done: AsyncMutex<HashSet<String>>,
    /// Stop flags of grabber projects currently running, by project
    /// id. Same pattern as `queue_runs`: the run task detaches on
    /// spawn and removes its own entry when it ends.
    grabber_runs: AsyncMutex<HashMap<String, Arc<AtomicBool>>>,
    /// Completion actions waiting for the UI countdown to confirm them.
    pending_actions: AsyncMutex<HashMap<String, PendingAction>>,
    action_events: broadcast::Sender<PendingAction>,
    /// Sound events the engine emitted (S09 Sounds tab), with the
    /// file each resolved to. The log is the testable surface on
    /// this VM: playback itself only happens on Windows, in
    /// `emit_sound` — here the event is recorded, never faked.
    sound_events: Arc<StdMutex<Vec<SoundEventRecord>>>,
    /// Browser cookies for downloads in flight (S13 finish):
    /// download id -> validated `Cookie` header value. Set by the
    /// drain from the pending row, read once per run and carried
    /// by that run's segment requests, for that download's host
    /// only. Memory only, never on disk: a restart drops them (a
    /// paused download resumed after a restart fails with a clear
    /// 401 instead of silently reusing a stale session), and the
    /// entry is removed when the download completes, errors or is
    /// cancelled — a pause keeps it, so pause/resume in one
    /// session keeps working.
    cookies: StdMutex<HashMap<String, String>>,
    /// Clipboard monitoring (Round 52): the hash of the last text
    /// seen and the URLs already offered or dismissed, so the same
    /// link is never offered twice. Pure logic — no clipboard
    /// access; the shell hands the text in.
    clipboard: StdMutex<ClipboardMonitor>,
    /// Update check (S09 / should-have): true while a weekly
    /// check spawned from the scheduler tick is in flight, so a
    /// later tick never starts a second one; and the channel the
    /// weekly check uses to tell the UI a new version is
    /// available (the UI shows the update dialog for it).
    update_check_running: AtomicBool,
    update_events: broadcast::Sender<UpdateCheckResult>,
    /// Antivirus scans in flight (S14), by download id. A second
    /// scan request for the same download while one runs is
    /// refused ("already running") instead of running the scanner
    /// twice on one file.
    scanning: StdMutex<HashSet<String>>,
    /// Independent run watchdog timeout in seconds (§7.2,
    /// Round 56): how long a run may go without byte progress
    /// before the watchdog task aborts it. Default
    /// `RUN_STALL_TIMEOUT` (180 s) — the same budget the in-loop
    /// watchdog uses. A diagnostics/test knob only: production
    /// never changes it, and the setter clamps to >= 1 s so a
    /// zero can never make the watchdog fire instantly. Tests
    /// shorten it to prove the watchdog fires without waiting
    /// three minutes.
    run_watchdog_timeout: AtomicU64,
}

impl DownloadManager {
    pub fn new(store: Arc<Store>, max_connections: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        let (action_events, _) = broadcast::channel(16);
        let (update_events, _) = broadcast::channel(16);
        let client = build_client(None, Settings::default().connection_timeout_secs as u64)
            .unwrap_or_else(|_| {
                Client::builder()
                    .user_agent("Fetchline/0.1")
                    .build()
                    .expect("reqwest client builds")
            });
        // Round 55: the segment client has no total timeout (see
        // `build_segment_client`); a fallback plain client is the
        // same for both when the configured build fails.
        let segment_client = build_segment_client(None, Settings::default().connection_timeout_secs as u64)
            .unwrap_or_else(|_| {
                Client::builder()
                    .user_agent("Fetchline/0.1")
                    .build()
                    .expect("reqwest client builds")
            });
        Arc::new(Self {
            client: RwLock::new(Arc::new(client)),
            segment_client: RwLock::new(Arc::new(segment_client)),
            store,
            max_connections: AtomicUsize::new(max_connections.max(1)),
            max_retries: AtomicU32::new(Settings::default().max_retries as u32),
            limiter: Arc::new(RateLimiter::new(0)),
            events,
            last_progress: Arc::new(StdMutex::new(HashMap::new())),
            running: AsyncMutex::new(HashMap::new()),
            thread_watchdogs: StdMutex::new(HashMap::new()),
            queue_runs: AsyncMutex::new(HashMap::new()),
            sync_runs: AsyncMutex::new(HashMap::new()),
            grabber_runs: AsyncMutex::new(HashMap::new()),
            scheduled_done: AsyncMutex::new(HashSet::new()),
            pending_actions: AsyncMutex::new(HashMap::new()),
            action_events,
            sound_events: Arc::new(StdMutex::new(Vec::new())),
            cookies: StdMutex::new(HashMap::new()),
            clipboard: StdMutex::new(ClipboardMonitor::new()),
            update_check_running: AtomicBool::new(false),
            update_events,
            scanning: StdMutex::new(HashSet::new()),
            run_watchdog_timeout: AtomicU64::new(RUN_STALL_TIMEOUT.as_secs()),
        })
    }

    /// The client current downloads and probes use.
    fn client(&self) -> Arc<Client> {
        Arc::clone(&self.client.read().unwrap())
    }

    /// The client segment body streams use (Round 55): connect
    /// timeout only, no total timeout — a slow download must never
    /// be cut off by a total budget.
    fn segment_client(&self) -> Arc<Client> {
        Arc::clone(&self.segment_client.read().unwrap())
    }

    /// Connections per download, as last applied from settings.
    pub fn max_connections(&self) -> usize {
        self.max_connections.load(Ordering::Relaxed)
    }

    /// Current global speed cap in bytes per second (0 = unlimited).
    pub fn speed_limit_bps(&self) -> u64 {
        self.limiter.rate_bps()
    }

    /// Independent run watchdog timeout in seconds (§7.2, Round
    /// 56): how long a run may go without byte progress before
    /// the watchdog task aborts it. Default 180 s
    /// (`RUN_STALL_TIMEOUT`).
    pub fn run_watchdog_timeout_secs(&self) -> u64 {
        self.run_watchdog_timeout.load(Ordering::Relaxed)
    }

    /// Shorten (or restore) the run watchdog timeout. A
    /// diagnostics/test knob only — production never calls it.
    /// Clamped to >= 1 s so a zero can never make the watchdog
    /// fire instantly on a healthy run.
    pub fn set_run_watchdog_timeout_secs(&self, secs: u64) {
        self.run_watchdog_timeout
            .store(secs.max(1), Ordering::SeqCst);
    }

    /// Read settings + the active proxy from the database and apply
    /// them live: connection count, retry count, speed cap, timeout
    /// and proxy. Call after `update_settings` / `save_proxy_config`
    /// and once at launch (the Tauri shell does this). Running
    /// downloads keep their segment count; the cap and the proxy
    /// apply to the next request they make.
    pub fn apply_settings(&self) -> Result<()> {
        let settings = self.store.get_settings()?;
        self.max_connections
            .store(settings.max_connections.max(1) as usize, Ordering::Relaxed);
        self.max_retries
            .store(settings.max_retries.max(0) as u32, Ordering::Relaxed);
        self.limiter.set_rate_bps(
            // BUG-008 defence in depth: the store rejects absurd caps,
            // but the multiply must never overflow even if a stored
            // value somehow got past validation.
            settings.speed_limit_kbps.map(|k| (k.max(0) as u64).saturating_mul(1024)).unwrap_or(0),
        );
        let proxy = self.store.get_active_proxy()?;
        let client = build_client(proxy.as_ref(), settings.connection_timeout_secs as u64)?;
        *self.client.write().unwrap() = Arc::new(client);
        let segment_client =
            build_segment_client(proxy.as_ref(), settings.connection_timeout_secs as u64)?;
        *self.segment_client.write().unwrap() = Arc::new(segment_client);
        Ok(())
    }

    // -- settings (S09) + speed limit (S12) ------------------------------

    pub fn get_settings(&self) -> Result<Settings> {
        self.store.get_settings()
    }

    /// Save settings and apply them live (see `apply_settings`).
    pub fn update_settings(&self, settings: &Settings) -> Result<Settings> {
        let saved = self.store.update_settings(settings)?;
        self.apply_settings()?;
        Ok(saved)
    }

    /// Change only the global speed cap (architecture.md
    /// `set_speed_limit`). `None` (or 0) means unlimited.
    pub fn set_speed_limit(&self, kbps: Option<i64>) -> Result<Settings> {
        let mut settings = self.store.get_settings()?;
        settings.speed_limit_kbps = match kbps {
            Some(0) | None => None,
            Some(k) => Some(k),
        };
        self.update_settings(&settings)
    }

    /// The stored speed cap in KB/s (`None` = unlimited).
    pub fn get_speed_limit(&self) -> Result<Option<i64>> {
        Ok(self.store.get_settings()?.speed_limit_kbps)
    }

    // -- download quota + per-download cap (S12) -------------------------

    /// The quota limit in force, if any: `(limit_bytes, window_secs)`.
    /// `None` when the quota is off or unconfigured. Read from
    /// settings on every call, so a save takes effect at once for
    /// the next download (a running download keeps the gate it
    /// started with — see `QuotaGate`).
    fn quota_limit(&self) -> Result<Option<(u64, i64)>> {
        let s = self.store.get_settings()?;
        match (s.quota_enabled, s.quota_mb, s.quota_hours) {
            (true, Some(mb), Some(hours)) if mb > 0 && hours > 0 => Ok(Some((
                (mb as u64).saturating_mul(QUOTA_MB_BYTES),
                hours.saturating_mul(3600),
            ))),
            _ => Ok(None),
        }
    }

    /// Is the quota reached right now? An expired window is not
    /// reached — it has rolled over, even if no tick has run yet
    /// to clear it (the next record anchors a fresh window).
    pub fn quota_reached_at(&self, now: i64) -> Result<bool> {
        let Some((limit, window_secs)) = self.quota_limit()? else {
            return Ok(false);
        };
        let (start, used) = self.store.get_quota_state()?;
        if window_expired(start, window_secs / 3600, now) {
            return Ok(false);
        }
        Ok(used as u64 >= limit)
    }

    /// The quota's live state for the UI (S12): limit, usage,
    /// window end, whether it is reached, how many downloads are
    /// paused waiting for it. Read-only: it never rolls the window
    /// or resumes anything — the tick and `reset_quota` do that.
    pub fn get_quota_status_at(&self, now: i64) -> Result<QuotaStatus> {
        let settings = self.store.get_settings()?;
        let (start, used) = self.store.get_quota_state()?;
        let limit = match (settings.quota_enabled, settings.quota_mb, settings.quota_hours) {
            (true, Some(mb), Some(_)) => Some((mb as u64).saturating_mul(QUOTA_MB_BYTES)),
            _ => None,
        };
        let window_hours = if settings.quota_enabled {
            settings.quota_hours
        } else {
            None
        };
        let reached = match limit {
            Some(l) if !window_expired(start, window_hours.unwrap_or(0), now) => {
                used as u64 >= l
            }
            _ => false,
        };
        Ok(QuotaStatus {
            enabled: settings.quota_enabled,
            limit_mb: if settings.quota_enabled {
                settings.quota_mb
            } else {
                None
            },
            window_hours,
            used_bytes: used,
            window_start_unix: start,
            window_end_unix: window_hours.and_then(|h| window_end_unix(start, h)),
            reached,
            quota_paused_downloads: self.store.count_quota_paused()?,
        })
    }

    /// The quota's live state against the real clock.
    pub fn get_quota_status(&self) -> Result<QuotaStatus> {
        self.get_quota_status_at(now_unix())
    }

    /// Set the download quota (architecture.md `set_download_limit`):
    /// X MB every Y hours, enabled. Validated by `update_settings`
    /// (both values required and in bounds); the live window is
    /// untouched — enabling a quota never resets usage already
    /// counted, and the window anchors at the next counted byte.
    pub fn set_download_limit(&self, mb: i64, hours: i64) -> Result<Settings> {
        let mut settings = self.store.get_settings()?;
        settings.quota_enabled = true;
        settings.quota_mb = Some(mb);
        settings.quota_hours = Some(hours);
        self.update_settings(&settings)
    }

    /// Turn the quota off (the limit stays stored, so turning it
    /// back on restores it). A disabled quota never pauses anything
    /// and its window is left as-is.
    pub fn set_quota_enabled(&self, enabled: bool) -> Result<Settings> {
        let mut settings = self.store.get_settings()?;
        settings.quota_enabled = enabled;
        self.update_settings(&settings)
    }

    /// Set (or clear) one download's per-download speed cap in
    /// KB/s. `None` (or 0) means no per-download cap — the global
    /// cap still applies. The cap is live-changeable: a running
    /// download picks the new rate up on its next chunk, like the
    /// global cap. Validated by the store (absurd caps rejected,
    /// the BUG-008 rule).
    pub fn set_download_speed_limit(&self, id: &str, kbps: Option<i64>) -> Result<()> {
        self.store.set_download_speed_limit(id, kbps)?;
        // Live-apply to a running download: the bucket it already
        // holds is re-rated, so no restart is needed. A download
        // that is not running picks the stored cap up at its next
        // start, from the row.
        let bps = match kbps {
            Some(0) | None => 0,
            Some(k) => (k.max(0) as u64).saturating_mul(1024),
        };
        // `running` is an async mutex: this is a sync method, so
        // the live update cannot block on it — use `try_lock` and
        // skip when a run is mid-register (that run read the row
        // first, so it already has the new cap or is about to).
        if let Ok(running) = self.running.try_lock() {
            if let Some(r) = running.get(id) {
                r.per_download_limiter.set_rate_bps(bps);
            }
        }
        Ok(())
    }

    /// Roll the window over when it expired, WITHOUT resuming
    /// anything: used by the start gate, so a download started
    /// after the window end is not refused for a window that is
    /// already over. The resume of quota-paused downloads belongs
    /// to the tick / reset, not to a single start.
    fn maybe_rollover_quota_at(&self, now: i64) -> Result<()> {
        let Some((_, window_secs)) = self.quota_limit()? else {
            return Ok(());
        };
        let (start, used) = self.store.get_quota_state()?;
        if window_expired(start, window_secs / 3600, now) && (start.is_some() || used > 0) {
            self.store.set_quota_state(None, 0)?;
        }
        Ok(())
    }

    /// Check the quota window at `now`: when it expired, reset the
    /// usage and resume the downloads it paused. Called on the 30 s
    /// scheduler tick (there is no second scheduler) and by the
    /// tests with a fake `now`. Returns true when a rollover
    /// happened. A resume that fails (e.g. the server is gone) is
    /// recorded nowhere special: the download stays paused with
    /// its `quota_paused` flag cleared by the start attempt, and
    /// its error (if any) is on the row — the user can start it
    /// by hand.
    pub async fn check_quota_rollover_at(self: &Arc<Self>, now: i64) -> Result<bool> {
        let Some((_, window_secs)) = self.quota_limit()? else {
            return Ok(false);
        };
        let (start, used) = self.store.get_quota_state()?;
        if !window_expired(start, window_secs / 3600, now) {
            return Ok(false);
        }
        if start.is_none() && used == 0 && self.store.count_quota_paused()? == 0 {
            return Ok(false);
        }
        self.store.set_quota_state(None, 0)?;
        self.resume_quota_paused_downloads().await;
        Ok(true)
    }

    /// Resume every download the quota paused, in creation order.
    /// Each start goes through the normal `start_download` (quota
    /// gate, segments, limiter), so a resume after a reset or a
    /// rollover is a real start, not a flag flip. Failures are
    /// ignored here: the download keeps its row and its error for
    /// the user to see.
    async fn resume_quota_paused_downloads(self: &Arc<Self>) {
        let Ok(ids) = self.store.list_quota_paused_ids() else {
            return;
        };
        for id in ids {
            let _ = self.start_download(&id).await;
        }
    }

    /// Manual action (S12): reset the quota now — start a fresh
    /// window at once (usage 0, window anchored at `now`) and
    /// resume the downloads the quota paused. This is the "resume
    /// now" the UI offers: there is no separate resume that keeps
    /// the old window, because resuming into an exhausted window
    /// would pause the download again on its next chunk.
    pub async fn reset_quota_at(self: &Arc<Self>, now: i64) -> Result<QuotaStatus> {
        self.store.set_quota_state(Some(now), 0)?;
        self.resume_quota_paused_downloads().await;
        self.get_quota_status_at(now)
    }

    /// Reset the quota against the real clock.
    pub async fn reset_quota(self: &Arc<Self>) -> Result<QuotaStatus> {
        self.reset_quota_at(now_unix()).await
    }

    // -- update check (S09 / should-have, "Quick update check") ----

    /// Subscribe to weekly-check results: the engine sends the
    /// result only when a newer version was found, so the UI can
    /// show the update dialog. Up-to-date and failed weekly
    /// checks send nothing (the failure is recorded in settings
    /// and retried next week).
    pub fn subscribe_update_checks(&self) -> broadcast::Receiver<UpdateCheckResult> {
        self.update_events.subscribe()
    }

    /// Manual check (Help > Check for updates): fetches the
    /// manifest from the configured update server and reports
    /// whether a newer version exists. Always runs, even when
    /// the weekly setting is off. Never installs anything and
    /// never starts a download — the result is for the dialog
    /// to show, and the user decides.
    pub async fn check_for_updates(&self) -> Result<UpdateCheckResult> {
        let url = self.store.get_settings()?.update_url;
        self.check_for_updates_from(&url).await
    }

    /// Manual check against one URL (the configured one, in
    /// practice; tests point it at a local fixture server). The
    /// last-check time is recorded after the attempt — success
    /// or failure — so a failed check retries next week, not on
    /// every tick. A failure to record the time never fails the
    /// check itself.
    pub async fn check_for_updates_from(&self, url: &str) -> Result<UpdateCheckResult> {
        let outcome = fetch_manifest(&self.client(), url).await;
        let now = now_unix();
        let _ = self
            .store
            .record_update_check(&unix_to_iso(now));
        match outcome {
            Ok(manifest) => Ok(result_from_manifest(&manifest, url.trim(), now)),
            Err(e) => Err(e),
        }
    }

    /// Is an automatic (weekly) check due at `now`? Reads the
    /// weekly setting and the last recorded check from settings.
    pub fn weekly_update_check_due_at(&self, now: i64) -> Result<bool> {
        let settings = self.store.get_settings()?;
        Ok(weekly_check_due(
            settings.check_updates_weekly,
            settings.last_update_check_at.as_deref(),
            now,
        ))
    }

    /// Run the weekly check when due, and do nothing when it is
    /// not: returns `None` without touching the network. When a
    /// newer version is found the result is also sent on the
    /// update channel, so the UI can prompt the user. A failed
    /// check is an error (the last-check time is still recorded
    /// by `check_for_updates_from`, so the next attempt is next
    /// week). Called from a spawned task by the scheduler tick —
    /// never inline in the tick, so a slow server cannot stall
    /// queue scheduling.
    pub async fn run_weekly_update_check_at(
        self: &Arc<Self>,
        now: i64,
    ) -> Result<Option<UpdateCheckResult>> {
        if !self.weekly_update_check_due_at(now)? {
            return Ok(None);
        }
        let result = self.check_for_updates().await?;
        if result.update_available {
            let _ = self.update_events.send(result.clone());
        }
        Ok(Some(result))
    }

    /// Spawn the weekly check from the scheduler tick when one is
    /// due. Non-blocking: the fetch runs in a detached task, and
    /// the in-flight flag stops a later tick starting a second
    /// one while it runs. Errors are swallowed — a failed check
    /// is recorded in settings and retried next week, and the
    /// tick must never fail because the update server is down.
    fn maybe_spawn_weekly_update_check(self: &Arc<Self>, now: i64) {
        if !self.weekly_update_check_due_at(now).unwrap_or(false) {
            return;
        }
        if self
            .update_check_running
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let _ = this.run_weekly_update_check_at(now).await;
            this.update_check_running.store(false, Ordering::SeqCst);
        });
    }

    // -- site logins (S09 Site logins tab) --------------------------------

    /// The saved Basic-auth credential for a URL's host, if any.
    /// Exact-host match only: a login for `example.com` is never
    /// sent to `sub.example.com` or anywhere else.
    fn site_auth_for_url(&self, url: &str) -> Option<(String, String)> {
        let host = url_host(url)?;
        self.store.get_site_login_credentials(&host).ok().flatten()
    }

    /// The enabled proxy with its password, if one is configured.
    /// FTP and NTLM paths need the config itself (not just the
    /// reqwest client built from it), so they read it here, fresh
    /// from the store, at the moment of use.
    fn active_proxy(&self) -> Option<ProxyConfig> {
        self.store.get_active_proxy().ok().flatten()
    }

    /// Probe a URL with the saved site login for its host, if one
    /// exists — the one probe path the add/resume paths use, so a
    /// login-gated file probes (and then downloads) instead of
    /// failing with 401. The active proxy rides along too, so
    /// `ftp://` URLs probe through the FTP transport (direct or via
    /// an FTP proxy) and an NTLM proxy can answer a 407 with the
    /// handshake instead of a bare failure.
    async fn probe_url(&self, url: &str) -> Result<ProbeInfo> {
        // Round 56 (§7.1d): probe duration is logged (throttled by
        // the call itself — a probe is one network round trip, not
        // a hot loop) so a slow or hanging probe is visible in
        // startup.log instead of a silent Add-dialog spin. The
        // error is logged too: the caller still gets the Err, but
        // the log line is what survives a wedged UI.
        let start = Instant::now();
        let auth = self.site_auth_for_url(url);
        let auth_ref = auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
        let proxy = self.active_proxy();
        let result = probe_with_proxy(&self.client(), url, auth_ref, proxy.as_ref()).await;
        match &result {
            Ok(info) => self.store.log(&format!(
                "probe {} ok in {}ms (size {:?}, range {})",
                url,
                start.elapsed().as_millis(),
                info.size,
                info.accept_ranges
            )),
            Err(e) => self.store.log(&format!(
                "probe {} failed in {}ms: {e}",
                url,
                start.elapsed().as_millis()
            )),
        }
        result
    }

    /// Probe a URL with the browser's cookies for it, if any —
    /// else with the saved site login (S09). Cookies take
    /// precedence: the browser's live session is fresher than a
    /// saved Basic credential. FTP cannot carry cookies, so an FTP
    /// link probes the plain way. This is the probe the drain and
    /// the resume path use for a cookie-carrying download — a
    /// paused cookie download resumes with its cookies instead of
    /// failing the re-probe with 401.
    async fn probe_url_with_cookies(&self, url: &str, cookies: Option<&str>) -> Result<ProbeInfo> {
        let is_ftp = reqwest::Url::parse(url).map(|u| u.scheme() == "ftp").unwrap_or(false);
        match cookies {
            Some(header) if !header.is_empty() && !is_ftp => {
                let proxy = self.active_proxy();
                probe_with_cookies(&self.client(), url, Some(header), proxy.as_ref()).await
            }
            _ => self.probe_url(url).await,
        }
    }

    pub fn list_site_logins(&self) -> Result<Vec<SiteLoginRow>> {
        self.store.list_site_logins()
    }

    pub fn save_site_login(&self, input: &SiteLoginInput) -> Result<SiteLoginRow> {
        self.store.save_site_login(input)
    }

    pub fn delete_site_login(&self, id: &str) -> Result<()> {
        self.store.delete_site_login(id)
    }

    // -- sounds (S09 Sounds tab) ------------------------------------------

    /// Sound events the engine has emitted so far, oldest first.
    /// The test surface for sounds on this VM (playback never
    /// happens here).
    pub fn sound_events(&self) -> Vec<SoundEventRecord> {
        self.sound_events.lock().unwrap().clone()
    }

    /// The configured file for one event, when it is enabled and a
    /// file is set. `None` means "no sound for this event".
    pub fn resolve_sound(&self, event: &str) -> Result<Option<String>> {
        self.store.get_sound_file(event)
    }

    /// Emit one sound event: resolve the configured file and record
    /// the event with it. On Windows the file is also played, in a
    /// detached process, so a slow player can never stall the
    /// download loop; on any other OS nothing is played — the
    /// record is the whole behaviour here, and it says so in the
    /// build log. A disabled event, an unset file, or a file that
    /// no longer exists on disk records nothing and plays nothing.
    fn emit_sound(&self, event: &str) {
        let file = match self.store.get_sound_file(event) {
            Ok(Some(f)) if std::path::Path::new(&f).is_file() => f,
            _ => return,
        };
        self.sound_events.lock().unwrap().push(SoundEventRecord {
            event: event.to_string(),
            file: Some(file.clone()),
            at_unix: now_unix(),
        });
        #[cfg(target_os = "windows")]
        {
            let _ = play_wav_file(&file);
        }
    }

    pub fn get_sound_settings(&self) -> Result<Vec<SoundSetting>> {
        self.store.get_sound_settings()
    }

    /// Save every event's sound setting in one call (the Settings
    /// dialog saves all three tabs at once).
    pub fn save_sound_settings(&self, inputs: &[SoundSettingInput]) -> Result<Vec<SoundSetting>> {
        let mut out = Vec::new();
        for input in inputs {
            out.push(self.store.save_sound_setting(input)?);
        }
        Ok(out)
    }

    /// Play one configured event's sound now (the Sounds tab's Test
    /// button plays the draft file instead — see
    /// `test_sound_file`). On this Linux VM this is an error:
    /// playback is Windows-only, like shutdown/hibernate.
    pub fn play_sound(&self, event: &str) -> Result<String> {
        let file = self
            .store
            .get_sound_file(event)?
            .ok_or_else(|| EngineError::InvalidState(format!("No sound is set for {event} — pick a WAV file and turn the event on first")))?;
        play_wav_file(&file)
    }

    /// Play an arbitrary WAV file (the Test button's draft file).
    /// Validates first: the file must exist and end in `.wav`.
    /// On this Linux VM this is an error — playback is
    /// Windows-only — so the button's honest result here is the
    /// validation, not audio.
    pub fn test_sound_file(&self, path: &str) -> Result<String> {
        let path = path.trim();
        if path.is_empty() {
            return Err(EngineError::InvalidState(
                "Pick a WAV file to test".to_string(),
            ));
        }
        if !path.to_ascii_lowercase().ends_with(".wav") {
            return Err(EngineError::InvalidState(
                "Sounds must be WAV files — pick a file ending in .wav".to_string(),
            ));
        }
        if !std::path::Path::new(path).is_file() {
            return Err(EngineError::InvalidState(format!(
                "That sound file does not exist: {path}"
            )));
        }
        play_wav_file(path)
    }

    // -- proxy (S09 Proxy/Socks tab) --------------------------------------

    pub fn get_proxy_config(&self) -> Result<Option<ProxyConfigRow>> {
        self.store.get_proxy_config()
    }

    /// Save the proxy and rebuild the HTTP client so it is used at
    /// once (see `apply_settings`).
    pub fn save_proxy_config(&self, input: &ProxyConfigInput) -> Result<ProxyConfigRow> {
        let saved = self.store.save_proxy_config(input)?;
        self.apply_settings()?;
        Ok(saved)
    }

    /// Probe a URL through a proxy that has NOT been saved yet (the
    /// Test button). Builds a throwaway client from the draft config
    /// and returns the HTTP status on a 2xx answer; the saved proxy
    /// is untouched.
    ///
    /// A test that cannot honestly report on the proxy is a failure,
    /// not a success (BUG-009/010): a 407 is an authentication error,
    /// any other non-2xx status is an error, an FTP proxy cannot be
    /// tested with an HTTP(S) URL, and a proxy scoped to one scheme
    /// cannot be tested with a URL of the other scheme.
    ///
    /// FTP and NTLM (S09 finish): an `ftp://` test URL is probed
    /// through the draft FTP proxy with the real FTP transport
    /// (200 on a successful SIZE); an HTTP proxy that answers 407
    /// walks the proxy-auth fallback order (Kerberos, then NTLM,
    /// then Basic — see `proxy::proxy_auth_order`) for plain
    /// `http://` URLs. Kerberos for `https://` is refused with a
    /// clear error (a TLS tunnel after the handshake is not built),
    /// like NTLM.
    pub async fn test_proxy(&self, input: &ProxyConfigInput, test_url: &str) -> Result<u16> {
        crate::proxy::validate_proxy_input(input)?;
        if !input.enabled {
            return Err(EngineError::InvalidState(
                "The proxy is disabled — enable it before testing".to_string(),
            ));
        }
        let parsed =
            reqwest::Url::parse(test_url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        let scheme = parsed.scheme();
        if !matches!(scheme, "http" | "https" | "ftp") {
            return Err(EngineError::InvalidUrl(format!(
                "The test URL must be an http://, https:// or ftp:// URL, not {scheme}://"
            )));
        }
        if input.proxy_type == "ftp" {
            if scheme != "ftp" {
                return Err(EngineError::InvalidState(
                    "This is an FTP proxy, which does not apply to HTTP(S) traffic — it cannot be tested with this URL".to_string(),
                ));
            }
            // FTP test: the draft proxy is the FTP proxy, so the
            // FTP transport proves it (login + SIZE). `use_for`
            // scopes are http/https only and do not apply to FTP.
            let cfg = ProxyConfig {
                proxy_type: input.proxy_type.clone(),
                host: input.host.trim().to_string(),
                port: input.port,
                username: input.username.clone().filter(|u| !u.trim().is_empty()),
                password: input.password.clone().filter(|p| !p.is_empty()),
                use_for: input.use_for.clone(),
                bypass: input.bypass.clone(),
                auth: crate::proxy::resolve_proxy_auth(
                    input.auth.as_deref().unwrap_or(crate::proxy::PROXY_AUTH_NEGOTIATE_AUTO),
                )
                .to_string(),
            };
            crate::ftp::ftp_probe(test_url, Some(&cfg)).await?;
            return Ok(200);
        }
        if scheme == "ftp" {
            return Err(EngineError::InvalidState(
                "An ftp:// test URL needs an FTP proxy — this proxy is not one, and FTP through an HTTP proxy is not supported".to_string(),
            ));
        }
        if !input.use_for.is_empty() && !input.use_for.iter().any(|s| s == scheme) {
            let scopes = input.use_for.join(", ");
            return Err(EngineError::InvalidState(format!(
                "This proxy is only used for {scopes}, so it cannot be tested with an {scheme}:// URL — use a {scopes} test URL instead"
            )));
        }
        let cfg = ProxyConfig {
            proxy_type: input.proxy_type.clone(),
            host: input.host.trim().to_string(),
            port: input.port,
            username: input.username.clone().filter(|u| !u.trim().is_empty()),
            password: input.password.clone().filter(|p| !p.is_empty()),
            use_for: input.use_for.clone(),
            bypass: input.bypass.clone(),
            auth: crate::proxy::resolve_proxy_auth(
                input.auth.as_deref().unwrap_or(crate::proxy::PROXY_AUTH_NEGOTIATE_AUTO),
            )
            .to_string(),
        };
        let client = build_client(Some(&cfg), 30)?;
        let resp = match client.get(test_url).send().await {
            Ok(r) => r,
            Err(e) => {
                // As in the probe: a failed CONNECT hides the
                // proxy's 407, so ask the proxy what it wants and
                // explain an NTLM / Kerberos-only proxy clearly.
                if scheme == "https" && cfg.username.is_some() {
                    if let Some(kind) = crate::ntlm::probe_connect_auth(&cfg, test_url).await {
                        match kind {
                            crate::ntlm::ConnectAuthKind::Ntlm => {
                                return Err(EngineError::InvalidState(
                                    "NTLM proxy authentication is only supported for http:// URLs — \
                                     https:// through an NTLM proxy needs a TLS tunnel after the \
                                     handshake, which is not built"
                                        .to_string(),
                                ));
                            }
                            crate::ntlm::ConnectAuthKind::NegotiateOnly => {
                                return Err(EngineError::InvalidState(
                                    "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
                                     https:// through a Kerberos proxy needs a TLS tunnel after the \
                                     handshake, which is not built"
                                        .to_string(),
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                return Err(e.into());
            }
        };
        let status = resp.status();
        if status.as_u16() == 407 {
            // The proxy refused the request: walk the proxy-auth
            // fallback order (Kerberos, then NTLM, then Basic — see
            // `proxy::proxy_auth_order`). `auth: basic` already sent
            // its credentials preemptively, so a 407 there means
            // they were refused. A failed scheme falls through to
            // the next one in the order.
            if resolve_proxy_auth(&cfg.auth) == PROXY_AUTH_BASIC {
                return Err(EngineError::InvalidState(
                    "Proxy authentication failed (407) — check the proxy username and password".to_string(),
                ));
            }
            let offered = offered_proxy_auth_schemes(
                resp.headers()
                    .get_all("proxy-authenticate")
                    .iter()
                    .filter_map(|v| v.to_str().ok()),
            );
            let order = proxy_auth_order(&cfg.auth, &offered, cfg.has_basic_creds());
            if order.is_empty() {
                return Err(crate::proxy::proxy_auth_not_offered_error(&cfg.auth, &offered));
            }
            let mut last_err: Option<EngineError> = None;
            for auth_scheme in order {
                match auth_scheme {
                    ProxyAuthScheme::Kerberos => {
                        if scheme != "http" {
                            return Err(EngineError::InvalidState(
                                "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
                                 https:// through a Kerberos proxy needs a TLS tunnel after the \
                                 handshake, which is not built"
                                    .to_string(),
                            ));
                        }
                        match crate::kerberos::kerberos_head(&cfg, test_url).await {
                            Ok((kerb_status, _)) => {
                                if kerb_status == 407 {
                                    last_err = Some(EngineError::InvalidState(
                                        "The proxy refused the Kerberos (Negotiate) authentication (407) — \
                                         check that the proxy trusts this account and that the Kerberos ticket is valid"
                                            .to_string(),
                                    ));
                                    continue;
                                }
                                if !(200..300).contains(&kerb_status) {
                                    return Err(EngineError::InvalidState(format!(
                                        "The proxy answered with HTTP status {kerb_status}"
                                    )));
                                }
                                return Ok(kerb_status);
                            }
                            Err(e) => {
                                last_err = Some(e);
                                continue;
                            }
                        }
                    }
                    ProxyAuthScheme::Ntlm => {
                        if scheme != "http" {
                            return Err(EngineError::InvalidState(
                                "NTLM proxy authentication is only supported for http:// URLs — \
                                 https:// through an NTLM proxy needs a TLS tunnel after the \
                                 handshake, which is not built"
                                    .to_string(),
                            ));
                        }
                        match crate::ntlm::ntlm_head(&cfg, test_url).await {
                            Ok((ntlm_status, _)) => {
                                if ntlm_status == 407 {
                                    last_err = Some(EngineError::InvalidState(
                                        "Proxy authentication failed (407) — check the proxy username and password"
                                            .to_string(),
                                    ));
                                    continue;
                                }
                                if !(200..300).contains(&ntlm_status) {
                                    return Err(EngineError::InvalidState(format!(
                                        "The proxy answered with HTTP status {ntlm_status}"
                                    )));
                                }
                                return Ok(ntlm_status);
                            }
                            Err(e) => {
                                last_err = Some(e);
                                continue;
                            }
                        }
                    }
                    ProxyAuthScheme::Basic => {
                        let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) else {
                            return Err(EngineError::InvalidState(
                                "The proxy needs Basic authentication — set the proxy username and password"
                                    .to_string(),
                            ));
                        };
                        let resp = client
                            .get(test_url)
                            .header("proxy-authorization", basic_proxy_auth_header(user, pass))
                            .send()
                            .await?;
                        let basic_status = resp.status();
                        if basic_status.as_u16() == 407 {
                            last_err = Some(EngineError::InvalidState(
                                "Proxy authentication failed (407) — check the proxy username and password"
                                    .to_string(),
                            ));
                            continue;
                        }
                        if !basic_status.is_success() {
                            return Err(EngineError::InvalidState(format!(
                                "The proxy answered with HTTP status {basic_status}"
                            )));
                        }
                        return Ok(basic_status.as_u16());
                    }
                }
            }
            return Err(last_err.unwrap_or_else(|| {
                EngineError::InvalidState(
                    "Proxy authentication failed (407) — check the proxy username and password".to_string(),
                )
            }));
        }
        if !status.is_success() {
            return Err(EngineError::InvalidState(format!(
                "The proxy answered with HTTP status {status}"
            )));
        }
        Ok(status.as_u16())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProgressEvent> {
        self.events.subscribe()
    }

    pub fn subscribe_actions(&self) -> broadcast::Receiver<PendingAction> {
        self.action_events.subscribe()
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    // -- Tauri-command surface -------------------------------------------

    /// Probe the URL, create the download row and its segments.
    /// Does not start downloading — call `start_download` for that.
    /// When `queue_id` is given, the download is filed at the end of
    /// that queue ("Save for later"); otherwise it stays unqueued.
    ///
    /// Save folder, in order: the folder the caller gave, the
    /// category's folder (S09 Save To), the default folder in
    /// settings (S09 General), the OS Downloads folder.
    pub async fn add_download(
        self: &Arc<Self>,
        url: &str,
        filename: Option<String>,
        folder: Option<String>,
        category_id: Option<String>,
        queue_id: Option<String>,
    ) -> Result<(String, ProbeInfo)> {
        self.add_download_with_cookies(url, filename, folder, category_id, queue_id, None)
            .await
    }

    /// Add a download carrying the browser's cookies for its URL
    /// (S13 finish): `cookies` is the validated `Cookie` header
    /// value, or `None`. The cookies are sent on the probe (for
    /// http(s) — FTP cannot carry them, so an FTP link probes the
    /// plain way) and kept in memory for this download's segment
    /// requests; a restart drops them. Nothing is stored on disk:
    /// the pending row that carried them is wiped by the drain.
    pub async fn add_download_with_cookies(
        self: &Arc<Self>,
        url: &str,
        filename: Option<String>,
        folder: Option<String>,
        category_id: Option<String>,
        queue_id: Option<String>,
        cookies: Option<String>,
    ) -> Result<(String, ProbeInfo)> {
        let settings = self.store.get_settings().unwrap_or_default();
        // BUG-011: sites in the excluded-sites list are refused before
        // any network request is made — no probe, no download.
        if let Some(host) = url_host(url) {
            if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                return Err(EngineError::InvalidState(format!(
                    "This site is excluded in Settings (excluded sites): {host}"
                )));
            }
        }
        // Browser cookies (S13 finish) take precedence over a
        // stored site login: the browser's live session is fresher
        // than a saved Basic credential. Without cookies the saved
        // credential is sent as Basic auth on the probe (S09) — a
        // login-gated file probes instead of failing with 401. FTP
        // cannot carry cookies, so an FTP link probes the plain way.
        let info = self
            .probe_url_with_cookies(url, cookies.as_deref())
            .await?;
        let name = sanitize_filename(
            filename
                .filter(|f| !f.trim().is_empty())
                .or_else(|| info.filename.clone())
                .or_else(|| filename_from_url(&info.final_url))
                .unwrap_or_else(|| "download".to_string()),
        );
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let category = match category_id {
            Some(c) if !c.trim().is_empty() => c,
            // BUG-011: auto-assign honours the file_types_auto list.
            _ => self.store.category_for_extension_filtered(&ext, &settings.file_types_auto)?,
        };
        let folder = folder
            .filter(|f| !f.trim().is_empty())
            .or_else(|| {
                self.store
                    .get_category(&category)
                    .ok()
                    .flatten()
                    .map(|c| c.default_folder)
                    .filter(|f| !f.trim().is_empty())
            })
            .or_else(|| {
                let f = settings.default_download_folder.trim().to_string();
                if f.is_empty() { None } else { Some(f) }
            })
            .unwrap_or_else(|| default_downloads_dir().to_string_lossy().to_string());
        std::fs::create_dir_all(&folder)?;
        let folder_path = PathBuf::from(&folder);
        // Duplicate handling (S09 Downloads): "rename" keeps both
        // files (`name (2).ext`, …) and also counts a path already
        // reserved in the database — a download that has not started
        // yet has no file on disk, so the disk check alone hands two
        // adds of the same link the same path and they race each
        // other. "overwrite" uses the plain path, as the user asked.
        let save_path = if settings.duplicate_action == "overwrite" {
            folder_path.join(&name)
        } else {
            unique_path(&folder_path, &name, &|p| {
                p.exists()
                    || PathBuf::from(format!("{}.part", p.display())).exists()
                    || self
                        .store
                        .is_save_path_used(&p.to_string_lossy())
                        .unwrap_or(false)
            })
        };
        let size = info.size.map(|s| s as i64);
        let id = self.store.create_download(
            url,
            Some(&info.final_url),
            &name,
            &save_path.to_string_lossy(),
            &category,
            size,
            info.etag.as_deref(),
            info.last_modified.as_deref(),
            Some(info.accept_ranges),
        )?;
        // Browser cookies (S13 finish): kept in memory for this
        // download's segment requests, never on disk — see the
        // `cookies` field. A pause keeps them (same session); a
        // terminal state removes them (see `run` / `cancel_download`).
        if let Some(header) = cookies {
            self.cookies.lock().unwrap().insert(id.clone(), header);
        }
        let ranges = initial_ranges(&info, self.max_connections());
        self.store.create_segments(&id, &ranges)?;
        if let Some(queue) = queue_id {
            self.store.add_to_queue(&id, &queue)?;
        }
        Ok((id, info))
    }

    // -- browser extension (S13): pending-links drain -------------------

    /// Drain the browser-extension queue: every pending row is put
    /// through the REAL `add_download` path, so the excluded-sites
    /// refusal, the `file_types_auto` category auto-assign and the
    /// duplicate-path guard all apply exactly as they do to a
    /// manually added link. Each row is marked accepted (with the
    /// created download id) or rejected (with the reason) — one bad
    /// link never stops the rest of the queue.
    ///
    /// Defence in depth: the drain re-checks the desktop
    /// `browser_integration` flag and the excluded-sites list even
    /// though the host already checked both before enqueueing — a
    /// row enqueued before the setting changed must not slip
    /// through. Milestone 2 (S13 finish): a drained link is
    /// STARTED at once when `settings.browser_start_drained` is on
    /// (the default) — the old queued-only behaviour is still
    /// available by turning that setting off. Starting happens
    /// through the normal `start_download`, so the speed limiter,
    /// proxy and segment machinery all apply; if the start itself
    /// fails, the download stays queued and the failure is recorded
    /// in `summary.start_failed`, not as a rejection.
    ///
    /// S13 finish (cookies): a row that carries the browser's
    /// cookies is drained through `add_download_with_cookies`, so
    /// the probe and the download's segment requests send them for
    /// that host only. Deciding the row (accepted or rejected)
    /// wipes the cookies column — the drain is the last place they
    /// exist on disk.
    pub async fn drain_pending_links(self: &Arc<Self>) -> Result<DrainSummary> {
        let settings = self.store.get_settings().unwrap_or_default();
        let pending = self.store.list_pending_links(Some("pending"))?;
        let mut summary = DrainSummary {
            accepted: Vec::new(),
            rejected: Vec::new(),
            start_failed: Vec::new(),
        };
        if !settings.browser_integration {
            for row in pending {
                let err = "Browser integration is turned off in Fetchline settings".to_string();
                let _ = self.store.mark_pending_link(&row.id, "rejected", None, Some(&err));
                summary.rejected.push((row.id, err));
            }
            return Ok(summary);
        }
        for row in pending {
            // Re-check exclusion at drain time (a row enqueued
            // before the site was excluded must not slip through);
            // add_download checks it again before any probe, so
            // this is the third check, not the only one.
            if let Some(host) = url_host(&row.url) {
                if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                    let err = format!("This site is excluded in Settings (excluded sites): {host}");
                    let _ = self.store.mark_pending_link(&row.id, "rejected", None, Some(&err));
                    summary.rejected.push((row.id, err));
                    continue;
                }
            }
            // Browser cookies (S13 finish): rebuild the validated
            // header from the row's stored pairs. A corrupt value
            // (which enqueueing can never write) reads as no
            // cookies rather than failing the drain. Marking the row
            // accepted/rejected below wipes the column.
            let cookies = row
                .cookies
                .as_deref()
                .map(|json| cookies_to_header(&cookies_from_json(json)).ok())
                .flatten();
            match self
                .add_download_with_cookies(&row.url, None, None, None, None, cookies)
                .await
            {
                Ok((id, _)) => {
                    let _ = self.store.mark_pending_link(&row.id, "accepted", Some(&id), None);
                    if settings.browser_start_drained {
                        if let Err(e) = self.start_download(&id).await {
                            summary
                                .start_failed
                                .push((row.id.clone(), e.to_string()));
                        }
                    }
                    summary.accepted.push((row.id, id));
                }
                Err(e) => {
                    let err = e.to_string();
                    let _ = self.store.mark_pending_link(&row.id, "rejected", None, Some(&err));
                    summary.rejected.push((row.id, err));
                }
            }
        }
        Ok(summary)
    }

    /// Expand a page into pending links (S13 Download All without a
    /// content script): fetch the page and probe its links with the
    /// existing `fetch_page_links` path, then enqueue every link
    /// that probed clean. Dead links and excluded links are skipped
    /// (they carry an error in the probe result) — one bad link
    /// never fails the page. Returns the number enqueued.
    ///
    /// BUG-012: this is the host's boundary for a `download_all`
    /// with no links, so it enforces the desktop
    /// `browser_integration` flag itself — without this check the
    /// host queued links while catching was off, and the drain
    /// accepted them once the flag was turned back on.
    pub async fn enqueue_page_links(self: &Arc<Self>, page_url: &str) -> Result<usize> {
        let settings = self.store.get_settings().unwrap_or_default();
        if !settings.browser_integration {
            return Err(EngineError::InvalidState(
                "Browser integration is turned off in Fetchline settings".to_string(),
            ));
        }
        let links = self.fetch_page_links(page_url).await?;
        let mut enqueued = 0usize;
        for link in links {
            if link.error.is_some() {
                continue;
            }
            if self
                .store
                .enqueue_pending_link(&link.url, Some(page_url), "download_all")
                .is_ok()
            {
                enqueued += 1;
            }
        }
        Ok(enqueued)
    }

    // -- categories (pass-through to the store) ---------------------------

    // -- Download All + batch ---------------------------------------------

    /// Fetch a page and return its downloadable links, each probed for
    /// size and flagged when it already exists in the download list.
    /// Links that fail the probe are returned with their error and are
    /// excluded from the selection by the UI — the fetch itself does
    /// not fail because one link is dead.
    pub async fn fetch_page_links(self: &Arc<Self>, page_url: &str) -> Result<Vec<PageLink>> {
        let settings = self.store.get_settings().unwrap_or_default();
        // BUG-011: an excluded page is refused before it is fetched.
        if let Some(host) = url_host(page_url) {
            if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                return Err(EngineError::InvalidState(format!(
                    "This site is excluded in Settings (excluded sites): {host}"
                )));
            }
        }
        // The page itself may be login-gated: send the saved site
        // login for its host, if one exists (BUG-025 — the scan
        // previously fetched the page anonymously, so a gated page
        // could never be scanned even with a saved credential).
        let page_auth = self.site_auth_for_url(page_url);
        let mut page_req = self.client().get(page_url);
        if let Some((user, pass)) = page_auth.as_ref() {
            page_req = page_req.basic_auth(user, Some(pass));
        }
        let resp = page_req
            .send()
            .await
            .map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(EngineError::Status(
                status.as_u16(),
                format!("{status} while fetching the page"),
            ));
        }
        let bytes = resp.bytes().await?;
        const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(EngineError::InvalidState(
                "This page is too large to scan for links".to_string(),
            ));
        }
        let html = String::from_utf8_lossy(&bytes).into_owned();
        let final_url = page_url.to_string();
        let urls = extract_links(&html, &final_url);
        // BUG-011: links to excluded sites are never probed — they are
        // returned with an error, so the UI lists them as unselectable
        // exactly like dead links. The rest are probed in parallel.
        let is_excluded = |u: &str| {
            url_host(u).is_some_and(|h| crate::store::site_is_excluded(&h, &settings.excluded_sites))
        };
        let to_probe: Vec<String> = urls.iter().filter(|u| !is_excluded(u)).cloned().collect();
        // Probe the links in parallel: a page can hold dozens of them
        // and sequential HEADs would take seconds each on a slow host.
        // The active proxy rides along, so ftp:// links on a page
        // probe through the FTP transport too. Each link is probed
        // with the saved site login for ITS host, if one exists
        // (BUG-025 — the probes previously went out anonymously, so
        // a login-gated link showed a 401 in Download All / the
        // video panel even with a saved credential); exact-host
        // match only, so one site's password never leaks to another.
        let client = self.client();
        let proxy = self.active_proxy();
        let probed: HashMap<String, std::result::Result<ProbeInfo, EngineError>> =
            futures_util::future::join_all(to_probe.into_iter().map(|url| {
                let client = Arc::clone(&client);
                let proxy = proxy.clone();
                let auth = self.site_auth_for_url(&url);
                async move {
                    let auth_ref = auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
                    let r = probe_with_proxy(&client, &url, auth_ref, proxy.as_ref()).await;
                    (url, r)
                }
            }))
            .await
            .into_iter()
            .collect();
        let mut out = Vec::with_capacity(urls.len());
        for url in urls {
            let filename = filename_from_link_url(&url);
            let kind = classify_kind(&filename);
            let already_exists = self.store.url_exists(&url).unwrap_or(false);
            if is_excluded(&url) {
                out.push(PageLink {
                    url,
                    filename,
                    kind,
                    size: None,
                    error: Some(
                        "This site is excluded in Settings (excluded sites)".to_string(),
                    ),
                    already_exists,
                });
                continue;
            }
            match probed.get(&url) {
                Some(Ok(info)) => out.push(PageLink {
                    url,
                    filename,
                    kind,
                    size: info.size,
                    error: None,
                    already_exists,
                }),
                Some(Err(e)) => out.push(PageLink {
                    url,
                    filename,
                    kind,
                    size: None,
                    error: Some(e.to_string()),
                    already_exists,
                }),
                None => out.push(PageLink {
                    url,
                    filename,
                    kind,
                    size: None,
                    error: Some("The link could not be probed".to_string()),
                    already_exists,
                }),
            }
        }
        Ok(out)
    }

    /// Video grabber (S08 / F05): scan a page for downloadable
    /// video/audio, for the desktop video panel. The page is fetched
    /// and probed with the existing `fetch_page_links` path, so an
    /// excluded page is refused, excluded links are flagged, and
    /// dead links carry their error instead of being offered. Only
    /// media links (video/audio kind, known media extension) are
    /// returned. Quality is derived honestly from the URL/file-name
    /// label (`video::quality_for`) — a static scan cannot see the
    /// element's real dimensions, so anything without a label is
    /// "Unknown quality".
    ///
    /// HLS, decided in Milestone 3 (see `video.rs` module docs):
    /// playlists are offered as-is, never merged. The scan
    /// *understands* them instead: a master playlist is expanded
    /// into its real variants (resolution read from the playlist,
    /// each variant probed for size, capped at
    /// `MAX_HLS_VARIANTS`), a media playlist reports its segment
    /// count and duration and whether it is live, and an encrypted
    /// playlist is listed as not downloadable — the playlist alone
    /// will not play and Fetchline does not fetch decryption keys.
    /// A DASH `.mpd` is listed as a manifest, saved as-is. A
    /// playlist that cannot be fetched or parsed is kept as the
    /// single playlist item it was, with its generic note — the
    /// scan never fails because one playlist is odd. DRM cannot be
    /// detected from static HTML, so a protected stream is only
    /// discovered when its probe fails — the panel shows the probe
    /// error instead of a download button.
    pub async fn detect_page_videos(
        self: &Arc<Self>,
        page_url: &str,
    ) -> Result<Vec<crate::video::DetectedVideo>> {
        let links = self.fetch_page_links(page_url).await?;
        let mut out = Vec::new();
        for link in links {
            if !matches!(link.kind, LinkKind::Video | LinkKind::Audio) {
                continue;
            }
            let Some(format) = crate::video::format_of_url(&link.url) else {
                continue;
            };
            let live = format == "M3U8";
            let base = crate::video::DetectedVideo {
                quality: crate::video::quality_for(&link.url, 0, 0),
                url: link.url.clone(),
                filename: link.filename.clone(),
                format: format.clone(),
                size: link.size,
                error: link.error.clone(),
                live,
                downloadable: link.error.is_none(),
                note: if format == "M3U8" || format == "MPD" {
                    Some(crate::video::playlist_note(&format, live))
                } else {
                    None
                },
            };
            // Only a live (probed-OK) playlist is worth fetching:
            // a dead or excluded link already carries its error.
            if format == "M3U8" && link.error.is_none() {
                if let Some(expanded) = self.expand_hls_playlist(&base).await {
                    out.extend(expanded);
                    continue;
                }
            }
            out.push(base);
        }
        Ok(out)
    }

    /// Fetch a playlist's text, capped at `MAX_PLAYLIST_BYTES`.
    /// `None` on any failure (non-success status, oversize, bad
    /// UTF-8, network error) — the caller keeps the plain playlist
    /// item instead of failing the whole scan.
    async fn fetch_playlist_text(&self, url: &str) -> Option<String> {
        // A gated playlist needs the saved site login for its host
        // too (BUG-025), or the variant expansion silently falls
        // back to the unauthenticated master item.
        let auth = self.site_auth_for_url(url);
        let mut req = self.client().get(url);
        if let Some((user, pass)) = auth.as_ref() {
            req = req.basic_auth(user, Some(pass));
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?;
        if bytes.len() > crate::video::MAX_PLAYLIST_BYTES {
            return None;
        }
        String::from_utf8(bytes.to_vec()).ok()
    }

    /// Expand one probed-OK `.m3u8` item into what the panel should
    /// show. Returns `None` when the playlist cannot be fetched or
    /// parsed (caller keeps the single item); otherwise a media
    /// playlist returns one enriched item and a master playlist
    /// returns one item per variant (capped, each probed).
    async fn expand_hls_playlist(
        &self,
        item: &crate::video::DetectedVideo,
    ) -> Option<Vec<crate::video::DetectedVideo>> {
        use crate::video::{parse_m3u8, playlist_note, quality_from_resolution, MAX_HLS_VARIANTS};
        let text = self.fetch_playlist_text(&item.url).await?;
        let playlist = parse_m3u8(&text)?;
        if !playlist.is_master {
            if playlist.encrypted {
                let mut item = item.clone();
                item.downloadable = false;
                item.error = Some(
                    "This stream is encrypted — the playlist alone will not play, \
                     and Fetchline does not fetch decryption keys."
                        .to_string(),
                );
                item.note = Some(playlist_note("M3U8", playlist.live));
                return Some(vec![item]);
            }
            let mut item = item.clone();
            item.live = playlist.live;
            let mut note = playlist_note("M3U8", playlist.live);
            if playlist.segments > 0 {
                note.push_str(&format!(
                    " ({} segment{}, {} s)",
                    playlist.segments,
                    if playlist.segments == 1 { "" } else { "s" },
                    playlist.duration_secs as u64,
                ));
            }
            item.note = Some(note);
            return Some(vec![item]);
        }
        // Master playlist: one entry per variant, in playlist
        // order, capped. Each variant is probed like a page link so
        // a dead variant playlist is listed as not downloadable
        // instead of offered, and its size is real.
        let client = self.client();
        let proxy = self.active_proxy();
        let mut out = Vec::new();
        for variant in playlist.variants.iter().take(MAX_HLS_VARIANTS) {
            let Ok(variant_url) = reqwest::Url::parse(&item.url)
                .and_then(|base| base.join(&variant.uri))
            else {
                continue;
            };
            let variant_url = variant_url.to_string();
            if out.iter().any(|v: &crate::video::DetectedVideo| v.url == variant_url) {
                continue;
            }
            let auth = self.site_auth_for_url(&variant_url);
            let auth_ref = auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
            let (size, error) = match probe_with_proxy(&client, &variant_url, auth_ref, proxy.as_ref()).await {
                Ok(info) => (info.size, None),
                Err(e) => (None, Some(e.to_string())),
            };
            // Liveness/encryption come from the variant's own media
            // playlist when it can be fetched; when it cannot, the
            // entry keeps the generic note and live=false — the
            // probe above already says whether it is downloadable.
            let (live, encrypted) = match self.fetch_playlist_text(&variant_url).await {
                Some(vtext) => match parse_m3u8(&vtext) {
                    Some(vp) => (vp.live, vp.encrypted),
                    None => (false, false),
                },
                None => (false, false),
            };
            let mut entry = crate::video::DetectedVideo {
                quality: quality_from_resolution(variant.resolution),
                url: variant_url.clone(),
                filename: crate::links::filename_from_link_url(&variant_url),
                format: "M3U8".to_string(),
                size,
                error: None,
                live,
                downloadable: error.is_none() && !encrypted,
                note: Some(playlist_note("M3U8", live)),
            };
            if encrypted {
                entry.downloadable = false;
                entry.error = Some(
                    "This stream is encrypted — the playlist alone will not play, \
                     and Fetchline does not fetch decryption keys."
                        .to_string(),
                );
            } else if let Some(e) = error {
                entry.downloadable = false;
                entry.error = Some(e);
            }
            out.push(entry);
        }
        if out.is_empty() {
            // A master with no usable variants (all URIs unparsable
            // or duplicated): keep the single master item.
            return None;
        }
        Some(out)
    }

    // -- site grabber (S06 / F06) ---------------------------------------

    /// Create a grabber project (wizard step 1–3). Validation lives
    /// in `grabber::validate_grabber_input` — depth over 5, a page
    /// cap over 1000, no file types, or a bad URL are all rejected
    /// with a clear error. An excluded start site is refused before
    /// any network request, like `add_download`.
    ///
    /// This is the short form: grabbed files go to the main queue
    /// and no completion action is armed. The wizard uses
    /// `create_grabber_project_full` so the user can pick a queue
    /// and an action.
    #[allow(clippy::too_many_arguments)]
    pub fn create_grabber_project(
        &self,
        name: &str,
        start_url: &str,
        types: &[String],
        depth: i64,
        max_pages: i64,
        mode: &str,
        schedule_start_at: Option<&str>,
    ) -> Result<GrabberProjectRow> {
        self.create_grabber_project_full(
            name,
            start_url,
            types,
            depth,
            max_pages,
            mode,
            schedule_start_at,
            None,
            "none",
            None,
        )
    }

    /// Create a grabber project with a chosen queue and a
    /// completion action (Milestone 4). The queue must exist — an
    /// unknown queue is rejected here, at save time, with a clear
    /// error, so a project can never be saved pointing at a queue
    /// that is not there; the run re-checks (a queue can be
    /// deleted after the project was created — see
    /// `run_grabber_task`). The action validates through the same
    /// shared helper as a queue's own completion action, so the
    /// two can never drift apart. An empty queue id means the main
    /// queue.
    #[allow(clippy::too_many_arguments)]
    pub fn create_grabber_project_full(
        &self,
        name: &str,
        start_url: &str,
        types: &[String],
        depth: i64,
        max_pages: i64,
        mode: &str,
        schedule_start_at: Option<&str>,
        queue_id: Option<&str>,
        on_complete_action: &str,
        on_complete_program: Option<&str>,
    ) -> Result<GrabberProjectRow> {
        let filters = validate_grabber_input(name, start_url, types, depth, max_pages, mode, schedule_start_at)?;
        crate::queue::validate_completion_action(on_complete_action, on_complete_program)?;
        let queue_id = queue_id
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .unwrap_or(MAIN_QUEUE_ID)
            .to_string();
        if self.store.get_queue(&queue_id)?.is_none() {
            return Err(EngineError::InvalidState(format!(
                "Queue not found: {queue_id} — pick an existing queue"
            )));
        }
        let settings = self.store.get_settings().unwrap_or_default();
        if let Some(host) = url_host(start_url) {
            if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                return Err(EngineError::InvalidState(format!(
                    "This site is excluded in Settings (excluded sites): {host}"
                )));
            }
        }
        self.store.create_grabber_project(
            name,
            start_url,
            &filters,
            mode,
            schedule_start_at.map(str::trim).filter(|s| !s.is_empty()),
            &queue_id,
            on_complete_action,
            on_complete_program
                .map(str::trim)
                .filter(|p| !p.is_empty()),
        )
    }

    pub fn get_grabber_project(&self, id: &str) -> Result<Option<GrabberProjectRow>> {
        self.store.get_grabber_project(id)
    }

    pub fn list_grabber_projects(&self) -> Result<Vec<GrabberProjectRow>> {
        self.store.list_grabber_projects()
    }

    /// Start a grabber run in the background (the wizard's Run
    /// button). Returns at once; the UI polls
    /// `get_grabber_project` for progress. A scheduled start time
    /// does not block a manual Run — the schedule is only for the
    /// scheduler tick, which starts idle projects when their time
    /// arrives. A project that is already running cannot be started
    /// again; a finished, failed or stopped one can be re-run, which
    /// resets its counts.
    ///
    /// The status is written to `exploring` (counts reset)
    /// synchronously HERE, before the task is spawned, so any read
    /// after this call returns already sees a running state. The
    /// background task used to be the first writer, so a poller
    /// reading right after the call could still see the previous
    /// terminal state (`done`) and conclude the new run was over
    /// (BUG-019). The run is also registered in `grabber_runs`
    /// before spawning, so `stop_grabber` and a second `run_grabber`
    /// see it immediately.
    pub async fn run_grabber(self: &Arc<Self>, id: &str) -> Result<()> {
        self.store
            .get_grabber_project(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        {
            let runs = self.grabber_runs.lock().await;
            if runs.contains_key(id) {
                return Err(EngineError::InvalidState(
                    "This site grabber is already running".to_string(),
                ));
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.store
            .update_grabber_project(id, "exploring", None, 0, 0, 0, 0, &[])?;
        self.grabber_runs
            .lock()
            .await
            .insert(id.to_string(), Arc::clone(&stop));
        let this = Arc::clone(self);
        let project_id = id.to_string();
        let stop_clone = Arc::clone(&stop);
        tokio::spawn(async move {
            this.run_grabber_task(project_id, stop_clone).await;
        });
        Ok(())
    }

    /// Ask a running grabber to stop. The run checks the flag
    /// between pages and between files, then records status
    /// `stopped` — it never kills a download mid-chunk.
    pub async fn stop_grabber(&self, id: &str) -> Result<()> {
        let runs = self.grabber_runs.lock().await;
        let Some(stop) = runs.get(id) else {
            return Err(EngineError::InvalidState(
                "This site grabber is not running".to_string(),
            ));
        };
        stop.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// The crawl itself. Breadth-first from the start URL, same host
    /// only, depth-limited, page-capped, at least 500 ms between page
    /// fetches, robots.txt honoured. Matched files then go through
    /// the SAME `add_download` path as everything else (probe,
    /// category auto-assign, duplicate-path guard, proxy, speed
    /// limiter), tagged with the project id, with 2 attempts per
    /// file before the failure is recorded and the run moves on.
    async fn run_grabber_task(self: Arc<Self>, id: String, stop: Arc<AtomicBool>) {
        let project = match self.store.get_grabber_project(&id) {
            Ok(Some(p)) => p,
            _ => {
                self.grabber_runs.lock().await.remove(&id);
                return;
            }
        };
        // Re-check the exclusion list at run time, not only at
        // creation: the user may have excluded the site after the
        // project was created (BUG-018). Refuse with the same
        // message `create_grabber_project` gives, BEFORE fetching
        // robots.txt or any page — zero network requests — and in
        // explore-only mode too (the run lists nothing).
        {
            let settings = self.store.get_settings().unwrap_or_default();
            if let Some(host) = url_host(&project.start_url) {
                if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                    let msg = format!(
                        "This site is excluded in Settings (excluded sites): {host}"
                    );
                    let _ = self.store.update_grabber_project(
                        &id, "failed", Some(&msg), 0, 0, 0, 0, &[],
                    );
                    self.grabber_runs.lock().await.remove(&id);
                    return;
                }
            }
        }
        // The queue the project files into may have been deleted
        // after the project was created (creation validates it, but
        // that is no guarantee at run time). Fail the run here with
        // a clear reason, BEFORE fetching robots.txt or any page —
        // crawling a site whose files have nowhere to go would
        // waste the crawl and fail every file one by one.
        match self.store.get_queue(&project.queue_id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                let msg = format!(
                    "The queue this project files into no longer exists: {} — pick another queue and run it again",
                    project.queue_id
                );
                let _ = self.store.update_grabber_project(
                    &id, "failed", Some(&msg), 0, 0, 0, 0, &[],
                );
                self.grabber_runs.lock().await.remove(&id);
                return;
            }
            Err(e) => {
                let msg = format!("Could not read the project's queue: {e}");
                let _ = self.store.update_grabber_project(
                    &id, "failed", Some(&msg), 0, 0, 0, 0, &[],
                );
                self.grabber_runs.lock().await.remove(&id);
                return;
            }
        }
        let filters: GrabberFilters = project.filters.clone();
        let mut pages_explored: i64 = 0;
        let mut matched: Vec<MatchedFile> = Vec::new();
        let mut downloaded: i64 = 0;
        let mut failed: i64 = 0;
        let mut last_completed_path: Option<String> = None;
        let mut seen_files: HashSet<String> = HashSet::new();
        let save = |status: &str, error: Option<&str>, pages: i64, matched: &[MatchedFile], dl: i64, fl: i64| {
            let _ = self.store.update_grabber_project(
                &id,
                status,
                error,
                pages,
                matched.len() as i64,
                dl,
                fl,
                matched,
            );
        };
        save("exploring", None, 0, &matched, 0, 0);

        // Robots: fetch once for the start host and keep the body —
        // sub-pages are checked against the same rules without a
        // request per page. A missing robots.txt (404) or a fetch
        // failure allows crawling; only a fetched Disallow blocks.
        // The per-page check below also covers the start URL, so a
        // blocked start page fails the run with a clear status.
        let robots_body = self.fetch_robots(&project.start_url).await;

        let mut queue: VecDeque<(String, i64)> = VecDeque::new();
        queue.push_back((project.start_url.clone(), 0));
        let mut visited: HashSet<String> = HashSet::new();
        let mut first_page = true;
        let mut crawl_error: Option<String> = None;
        while let Some((url, depth)) = queue.pop_front() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if pages_explored >= filters.max_pages {
                break; // hard cap reached: stop cleanly, status done
            }
            if !visited.insert(url.clone()) {
                continue;
            }
            // Polite delay between page fetches (not before the first).
            if !first_page {
                tokio::time::sleep(POLITE_DELAY).await;
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            first_page = false;
            // Robots check per page path (same host, same rules).
            if let Some(body) = &robots_body {
                if let Ok(parsed) = reqwest::Url::parse(&url) {
                    if robots_disallows(body, parsed.path()) {
                        if pages_explored == 0 && depth == 0 {
                            crawl_error = Some(
                                "This site's robots.txt does not allow Fetchline to crawl this page"
                                    .to_string(),
                            );
                        }
                        continue; // blocked sub-page: skip it, keep crawling
                    }
                }
            }
            let html = match self.fetch_grabber_page(&url).await {
                Ok(Some(html)) => html,
                Ok(None) => {
                    // Login-required (401/403) or another non-success
                    // status. The start page failing this way ends the
                    // run with a clear reason; a sub-page is skipped
                    // and the crawl continues.
                    if pages_explored == 0 && depth == 0 {
                        // Distinguish login from other failures by
                        // re-checking the status directly.
                        let status = self
                            .client()
                            .get(&url)
                            .send()
                            .await
                            .map(|r| r.status().as_u16())
                            .unwrap_or(0);
                        crawl_error = if status == 401 || status == 403 {
                            Some("This page needs a login — Fetchline does not sign in to sites, so the grabber cannot continue".to_string())
                        } else {
                            Some(format!(
                                "The start page could not be read (server replied {status})"
                            ))
                        };
                    }
                    continue;
                }
                Err(e) => {
                    if pages_explored == 0 && depth == 0 {
                        crawl_error = Some(format!("The start page could not be read: {e}"));
                    }
                    continue;
                }
            };
            pages_explored += 1;
            let links = extract_links(&html, &url);
            for link in links {
                if !same_host(&link, &project.start_url) {
                    continue; // never follow off-site links
                }
                let filename = crate::links::filename_from_link_url(&link);
                let kind = crate::links::classify_kind(&filename);
                if is_page_url(&link) {
                    if depth < filters.depth {
                        queue.push_back((link, depth + 1));
                    }
                    continue;
                }
                if !type_matches(kind, &filters.types) {
                    continue;
                }
                if seen_files.insert(link.clone()) {
                    matched.push(MatchedFile {
                        url: link,
                        filename,
                        kind: kind.label().to_ascii_lowercase(),
                        size: None,
                        status: "matched".to_string(),
                        error: None,
                        download_id: None,
                    });
                }
            }
            save(
                "exploring",
                None,
                pages_explored,
                &matched,
                0,
                0,
            );
        }
        if let Some(err) = crawl_error {
            save("failed", Some(&err), pages_explored, &matched, 0, 0);
            self.grabber_runs.lock().await.remove(&id);
            return;
        }
        if stop.load(Ordering::SeqCst) {
            save("stopped", Some("Stopped by the user"), pages_explored, &matched, downloaded, failed);
            self.grabber_runs.lock().await.remove(&id);
            return;
        }
        let capped = pages_explored >= filters.max_pages;
        if project.mode == "explore" {
            // Explore-only: list what matched, download nothing.
            let _ = capped;
            save("done", None, pages_explored, &matched, 0, 0);
            self.grabber_runs.lock().await.remove(&id);
            return;
        }
        save("downloading", None, pages_explored, &matched, 0, 0);
        for i in 0..matched.len() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let file = matched[i].clone();
            match self
                .grabber_download_file(&id, &project.queue_id, &file)
                .await
            {
                Ok(download_id) => {
                    downloaded += 1;
                    matched[i].status = "downloaded".to_string();
                    matched[i].download_id = Some(download_id.clone());
                    // The completion action's open-file path is the
                    // last file that actually finished.
                    last_completed_path = self
                        .store
                        .get_download(&download_id)
                        .ok()
                        .flatten()
                        .map(|r| r.save_path);
                }
                Err(err) => {
                    failed += 1;
                    matched[i].status = "failed".to_string();
                    matched[i].error = Some(err);
                }
            }
            save(
                "downloading",
                None,
                pages_explored,
                &matched,
                downloaded,
                failed,
            );
        }
        if stop.load(Ordering::SeqCst) {
            save("stopped", Some("Stopped by the user"), pages_explored, &matched, downloaded, failed);
        } else {
            save("done", None, pages_explored, &matched, downloaded, failed);
            // On-completion action (Milestone 4): armed through the
            // EXISTING queue completion machinery — the same
            // `PendingAction`, the same `queue-action-pending`
            // event, the same 60 s countdown dialog the UI already
            // shows for a queue. The action is registered under the
            // project's target queue, so `get_pending_action`,
            // `execute_queue_action` and `dismiss_queue_action`
            // all work on it unchanged — there is no second
            // countdown and no second cancel path.
            //
            // It is armed ONLY when the run finished cleanly
            // (`done`, not stopped or failed) AND actually
            // downloaded at least one file — the BUG-003 lesson:
            // a queue that downloaded nothing must not arm its
            // action, and the same holds for a grabber project
            // that matched/downloaded zero files, or ran in
            // explore-only mode (which returns before this point
            // with downloaded = 0). A run stopped after some
            // files finished does not arm it either, mirroring
            // the queue drain (stopped early → no action).
            if project.mode == "download"
                && downloaded >= 1
                && project.on_complete_action != "none"
            {
                let queue_name = self
                    .store
                    .get_queue(&project.queue_id)
                    .ok()
                    .flatten()
                    .map(|q| q.name)
                    .unwrap_or_else(|| project.queue_id.clone());
                self.register_completion_action(
                    &project.queue_id,
                    queue_name,
                    &project.on_complete_action,
                    project.on_complete_program.clone(),
                    last_completed_path,
                )
                .await;
            }
        }
        self.grabber_runs.lock().await.remove(&id);
    }

    /// Fetch a robots.txt body for a URL's origin, or `None` when
    /// there is none / it could not be read. A missing robots.txt
    /// allows crawling; only a fetched Disallow blocks.
    async fn fetch_robots(&self, url: &str) -> Option<String> {
        let parsed = reqwest::Url::parse(url).ok()?;
        let origin = format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.host_str()?,
            parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
        );
        let resp = self.client().get(format!("{origin}/robots.txt")).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Fetch one crawl page as HTML. `Ok(None)` means the server
    /// answered with a non-success status (401/403 login walls and
    /// 404s included) — the caller decides whether that ends the
    /// run. `Err` is a transport failure.
    async fn fetch_grabber_page(&self, url: &str) -> Result<Option<String>> {
        let resp = self.client().get(url).send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let bytes = resp.bytes().await?;
        if bytes.len() > crate::grabber::MAX_PAGE_BYTES {
            return Err(EngineError::InvalidState(
                "This page is too large to crawl".to_string(),
            ));
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Download one matched file through the real `add_download`
    /// path, with up to 2 attempts (recon F06). Attempt 1 adds and
    /// starts the download; attempt 2 resumes the same download
    /// after a failure (or re-adds it when the add itself failed).
    /// After the second failure the error is returned and the run
    /// moves on — never retried forever. The file is filed into the
    /// project's queue (`queue_id`) through the same `add_download`
    /// queue parameter every other caller uses.
    async fn grabber_download_file(
        self: &Arc<Self>,
        project_id: &str,
        queue_id: &str,
        file: &MatchedFile,
    ) -> std::result::Result<String, String> {
        let mut last_err = "The download failed".to_string();
        let mut download_id: Option<String> = None;
        for attempt in 0..MAX_FILE_ATTEMPTS {
            let id = match &download_id {
                Some(id) => id.clone(),
                None => match self
                    .add_download(&file.url, None, None, None, Some(queue_id.to_string()))
                    .await
                {
                    Ok((id, _)) => {
                        let _ = self.store.set_download_grabber_project(&id, project_id);
                        download_id = Some(id.clone());
                        id
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        if attempt + 1 >= MAX_FILE_ATTEMPTS {
                            return Err(last_err);
                        }
                        continue;
                    }
                },
            };
            let start = if attempt == 0 {
                self.start_download(&id).await
            } else {
                self.resume_download(&id).await
            };
            if let Err(e) = start {
                last_err = e.to_string();
                if attempt + 1 >= MAX_FILE_ATTEMPTS {
                    return Err(last_err);
                }
                continue;
            }
            match self.wait_grabber_download(&id).await {
                Ok(()) => return Ok(id),
                Err(e) => {
                    last_err = e;
                    if attempt + 1 >= MAX_FILE_ATTEMPTS {
                        return Err(last_err);
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Wait for a grabber download to reach a terminal status.
    /// Complete wins; error / paused / deleted / a 120 s stall all
    /// count as a failed attempt (the caller retries once, then
    /// gives up on the file).
    async fn wait_grabber_download(&self, id: &str) -> std::result::Result<(), String> {
        for _ in 0..600 {
            match self.store.get_download(id) {
                Ok(Some(row)) => match row.status.as_str() {
                    "complete" => return Ok(()),
                    "error" => {
                        return Err(row
                            .error_message
                            .unwrap_or_else(|| "The download failed".to_string()))
                    }
                    "paused" | "deleted" => {
                        return Err("The download stopped before finishing".to_string())
                    }
                    _ => {}
                },
                _ => return Err("The download disappeared".to_string()),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = self.pause_download(id).await;
        Err("The download timed out".to_string())
    }

    /// Add many URLs at once (S10 batch, and the shared path behind
    /// Download All). Each line is added through `add_download`, so
    /// probing, category auto-assign, the duplicate-path guard and
    /// queue filing are all reused — there is no second download path.
    ///
    /// Blank lines are ignored. Invalid lines (bad URL, probe failure)
    /// and duplicates inside the batch are reported with their 1-based
    /// line number and skipped — one bad line never fails the batch.
    /// `start_now` starts every created download; `queue_id` files
    /// them in that queue instead (both together: file in the queue
    /// and start the queue's drain is not done here — the UI picks
    /// one). A queue that does not exist, or an unknown category, is
    /// rejected up front, before anything is added.
    pub async fn add_batch_downloads(
        self: &Arc<Self>,
        urls: &[String],
        folder: Option<String>,
        category_id: Option<String>,
        queue_id: Option<String>,
        start_now: bool,
    ) -> Result<BatchResult> {
        if let Some(q) = &queue_id {
            if self.store.get_queue(q)?.is_none() {
                return Err(EngineError::NotFound(q.clone()));
            }
        }
        if let Some(c) = &category_id {
            if !c.trim().is_empty()
                && !self
                    .store
                    .list_categories()?
                    .iter()
                    .any(|cat| &cat.id == c)
            {
                return Err(EngineError::NotFound(c.clone()));
            }
        }
        let mut created = Vec::new();
        let mut invalid = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (i, raw) in urls.iter().enumerate() {
            let line = i + 1;
            let url = raw.trim();
            if url.is_empty() {
                continue;
            }
            if !seen.insert(url.to_string()) {
                invalid.push(InvalidLine {
                    line,
                    url: url.to_string(),
                    error: "Duplicate link — already added from an earlier line".to_string(),
                });
                continue;
            }
            match self
                .add_download(url, None, folder.clone(), category_id.clone(), queue_id.clone())
                .await
            {
                Ok((id, _)) => {
                    if start_now {
                        if let Err(e) = self.start_download(&id).await {
                            invalid.push(InvalidLine {
                                line,
                                url: url.to_string(),
                                error: format!("Added, but could not start: {e}"),
                            });
                            continue;
                        }
                    }
                    created.push(id);
                }
                Err(e) => invalid.push(InvalidLine {
                    line,
                    url: url.to_string(),
                    error: e.to_string(),
                }),
            }
        }
        Ok(BatchResult { created, invalid })
    }

    /// Download All (S07): create one download per selected link from
    /// a page checklist. The selected links were already probed by
    /// `fetch_page_links`; they go through the same batch path, so
    /// validation, dedup and category auto-assign are shared with S10.
    pub async fn download_all_links(
        self: &Arc<Self>,
        _page_url: &str,
        links: Vec<String>,
        folder: Option<String>,
        category_id: Option<String>,
        queue_id: Option<String>,
        start_now: bool,
    ) -> Result<BatchResult> {
        self.add_batch_downloads(&links, folder, category_id, queue_id, start_now)
            .await
    }

    // -- categories (pass-through to the store) ---------------------------

    pub fn create_category(&self, name: &str, folder: &str, extensions: &[String]) -> Result<CategoryRow> {
        self.store.create_category(name, folder, extensions)
    }

    pub fn delete_category(&self, id: &str) -> Result<()> {
        self.store.delete_category(id)
    }

    pub fn move_to_category(&self, download_id: &str, category_id: &str) -> Result<()> {
        self.store.move_to_category(download_id, category_id)
    }

    /// Change a category's save folder and/or file extensions
    /// (S09 Save To + File Types tabs).
    pub fn update_category(
        &self,
        id: &str,
        folder: Option<&str>,
        extensions: Option<&[String]>,
    ) -> Result<CategoryRow> {
        self.store.update_category(id, folder, extensions)
    }

    // -- queues -----------------------------------------------------------

    pub fn create_queue(&self, name: &str) -> Result<QueueRow> {
        self.store.create_queue(name)
    }

    /// All queues, with the live `running` flag filled in.
    pub async fn list_queues(&self) -> Result<Vec<QueueRow>> {
        let mut queues = self.store.list_queues()?;
        let runs = self.queue_runs.lock().await;
        for q in &mut queues {
            q.running = runs.contains_key(&q.id);
        }
        Ok(queues)
    }

    pub async fn get_queue(&self, id: &str) -> Result<Option<QueueRow>> {
        let mut row = self.store.get_queue(id)?;
        if let Some(q) = &mut row {
            q.running = self.queue_runs.lock().await.contains_key(&q.id);
        }
        Ok(row)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_queue(
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
        let row = self.store.update_queue(
            id,
            schedule_type,
            start_at,
            stop_at,
            start_on_startup,
            on_complete_action,
            on_complete_program,
            sync_interval_minutes,
        )?;
        // A re-saved schedule is a new schedule: a one-time queue that
        // already ran may be scheduled again.
        self.scheduled_done.lock().await.remove(id);
        Ok(row)
    }

    pub fn add_to_queue(&self, download_id: &str, queue_id: &str) -> Result<()> {
        self.store.add_to_queue(download_id, queue_id)
    }

    pub fn remove_from_queue(&self, download_id: &str) -> Result<()> {
        self.store.remove_from_queue(download_id)
    }

    pub fn list_queue_files(&self, queue_id: &str) -> Result<Vec<DownloadRow>> {
        self.store.list_queue_files(queue_id)
    }

    pub fn reorder_queue(&self, queue_id: &str, download_id: &str, new_index: usize) -> Result<()> {
        self.store.reorder_queue(queue_id, download_id, new_index)
    }

    /// Start draining a queue in the background, one file at a time.
    /// A periodic synchronisation queue cannot be drained: its files
    /// are finished downloads that stay in the queue to be re-probed,
    /// and a drain would drop them (a finished file leaves a queue).
    /// Sync those with `run_sync_queue` instead — that is the
    /// one-time / periodic exclusivity, enforced where it matters.
    pub async fn start_queue(self: &Arc<Self>, id: &str) -> Result<()> {
        let queue = self
            .store
            .get_queue(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if queue.schedule_type == "periodic" {
            return Err(EngineError::InvalidState(
                "This is a periodic synchronisation queue — use “Sync now” instead of starting it as a one-time queue".to_string(),
            ));
        }
        {
            let runs = self.queue_runs.lock().await;
            if runs.contains_key(id) {
                return Err(EngineError::InvalidState(
                    "This queue is already running".to_string(),
                ));
            }
        }
        if self.store.list_queue_files(id)?.is_empty() {
            return Err(EngineError::InvalidState(
                "This queue has no files yet".to_string(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let this = Arc::clone(self);
        let queue_id = id.to_string();
        let stop_clone = Arc::clone(&stop);
        tokio::spawn(async move {
            this.run_queue(queue_id, stop_clone).await;
        });
        self.queue_runs.lock().await.insert(id.to_string(), stop);
        Ok(())
    }

    /// Ask a running queue to stop after the current file. The current
    /// file is paused (not cancelled), so it can be resumed later.
    pub async fn stop_queue(&self, id: &str) -> Result<()> {
        let runs = self.queue_runs.lock().await;
        let Some(stop) = runs.get(id) else {
            return Err(EngineError::InvalidState("This queue is not running".to_string()));
        };
        stop.store(true, Ordering::SeqCst);
        if let Ok(files) = self.store.list_queue_files(id) {
            for f in &files {
                if f.status == "downloading" {
                    let _ = self.pause_download(&f.id).await;
                    break;
                }
            }
        }
        Ok(())
    }

    /// One scheduler tick: start queues whose start time arrived, stop
    /// queues whose stop time passed. Pure time logic lives here so the
    /// tests can drive it with a fake clock.
    ///
    /// Queues whose stored times are invalid (written before
    /// `update_queue` validated them — BUG-005/006 legacy rows) are
    /// reported, not silently skipped: the tick still processes every
    /// other queue, then returns an error naming the bad ones, so the
    /// caller can surface it. Re-saving such a queue from the dialog
    /// now validates and clears the bad times.
    pub async fn scheduler_tick_at(self: &Arc<Self>, now: i64) -> Result<()> {
        // Weekly update check (S09): rides on this same 30 s tick
        // — there is no second scheduler. The check itself runs
        // in a detached task, so a slow update server cannot
        // stall queue scheduling; a due check is spawned here,
        // before the queue loop, so a legacy malformed queue
        // (which makes the tick return an error below) cannot
        // block it either.
        self.maybe_spawn_weekly_update_check(now);
        // Download quota (S12): the window rolls over on this
        // same 30 s tick — there is no second scheduler. When the
        // window end passed, the usage resets and the downloads
        // the quota paused resume.
        let _ = Arc::clone(self).check_quota_rollover_at(now).await;
        let mut invalid: Vec<String> = Vec::new();
        for q in self.store.list_queues()? {
            // Validate the stored window first: a malformed time or an
            // inverted window can only come from a pre-validation DB.
            let bad_time = |v: &Option<String>| -> bool {
                v.as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .is_some_and(|s| parse_iso_to_unix(s).is_none())
            };
            let start = q.start_at.as_deref().and_then(parse_iso_to_unix);
            let stop = q.stop_at.as_deref().and_then(parse_iso_to_unix);
            let window_bad = matches!((start, stop), (Some(s), Some(e)) if e <= s);
            if bad_time(&q.start_at) || bad_time(&q.stop_at) || window_bad {
                invalid.push(format!("{} ({})", q.name, q.id));
                continue;
            }
            let draining = self.queue_runs.lock().await.contains_key(&q.id);
            let syncing = self.sync_runs.lock().await.contains_key(&q.id);
            if draining || syncing {
                if let Some(stop_at) = stop {
                    if now >= stop_at {
                        if draining {
                            let _ = self.stop_queue(&q.id).await;
                        }
                        if syncing {
                            // A sync run stops after its current file,
                            // like a queue drain — it never kills a
                            // download mid-chunk.
                            if let Some(flag) = self.sync_runs.lock().await.get(&q.id) {
                                flag.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
                continue;
            }
            // Periodic synchronisation (S05/F04): re-probe the queue
            // on its interval, on this same 30 s tick — there is no
            // second scheduler. A sync run is spawned (it can take
            // as long as a re-download) and guarded by `sync_runs`,
            // so a later tick never starts a second one while the
            // first is still going.
            if q.schedule_type == "periodic" {
                if let Some(start_at) = start {
                    if now < start_at {
                        continue;
                    }
                }
                if let Some(stop_at) = stop {
                    if now >= stop_at {
                        continue;
                    }
                }
                let Some(interval) = q.sync_interval_minutes else {
                    // A periodic queue without an interval can only
                    // come from a database written before sync
                    // existed; re-saving it validates and fills the
                    // interval in, so the tick leaves it alone.
                    continue;
                };
                let due = match q.last_sync_at.as_deref().and_then(parse_iso_to_unix) {
                    None => true,
                    Some(last) => now >= last + interval * 60,
                };
                if !due || q.file_count == 0 {
                    continue;
                }
                let this = Arc::clone(self);
                let queue_id = q.id.clone();
                tokio::spawn(async move {
                    let _ = this.run_sync_queue(&queue_id).await;
                });
                continue;
            }
            if q.schedule_type != "one_time" {
                continue;
            }
            let Some(start_at) = start else {
                continue;
            };
            if now < start_at {
                continue;
            }
            if let Some(stop_at) = stop {
                if now >= stop_at {
                    continue; // the window already passed
                }
            }
            if self.scheduled_done.lock().await.contains(&q.id) {
                continue;
            }
            if q.file_count == 0 {
                continue;
            }
            let _ = self.start_queue(&q.id).await;
        }
        if !invalid.is_empty() {
            return Err(EngineError::InvalidState(format!(
                "Some queues have an invalid schedule and were skipped: {}. Re-save their schedules.",
                invalid.join(", ")
            )));
        }
        // Site grabber (S06): the same tick starts idle projects
        // whose one-time start time arrived. There is no second
        // scheduler — the project schedule lives in
        // `schedule_json` and this tick is the only trigger. A
        // project that already ran is not idle, so it never
        // restarts on a later tick.
        for p in self.store.list_grabber_projects()? {
            if p.status != "idle" {
                continue;
            }
            let Some(start_at) = p.schedule_start_at.as_deref().and_then(parse_iso_to_unix) else {
                continue;
            };
            if now < start_at {
                continue;
            }
            // A site excluded after the project was created must not
            // be crawled by the scheduler either (BUG-018): fail it
            // here with the same message instead of starting it, so
            // it does not stay idle and get retried on every tick.
            {
                let settings = self.store.get_settings().unwrap_or_default();
                if let Some(host) = url_host(&p.start_url) {
                    if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                        let _ = self.store.update_grabber_project(
                            &p.id,
                            "failed",
                            Some(&format!(
                                "This site is excluded in Settings (excluded sites): {host}"
                            )),
                            0,
                            0,
                            0,
                            0,
                            &[],
                        );
                        continue;
                    }
                }
            }
            let _ = self.run_grabber(&p.id).await;
        }
        Ok(())
    }

    /// Scheduler tick against the real clock (called every 30 s).
    pub async fn scheduler_tick(self: &Arc<Self>) -> Result<()> {
        self.scheduler_tick_at(now_unix()).await
    }

    /// Start every queue flagged "start on startup" (called once at
    /// launch). Queues with no files are skipped, not errored. A
    /// periodic queue syncs at startup instead of draining — its
    /// files are finished downloads that stay in the queue.
    pub async fn start_startup_queues(self: &Arc<Self>) -> Result<()> {
        for q in self.store.list_queues()? {
            if q.start_on_startup && q.file_count > 0 {
                if q.schedule_type == "periodic" {
                    let this = Arc::clone(self);
                    let queue_id = q.id.clone();
                    tokio::spawn(async move {
                        let _ = this.run_sync_queue(&queue_id).await;
                    });
                } else {
                    let _ = self.start_queue(&q.id).await;
                }
            }
        }
        Ok(())
    }

    // -- periodic synchronisation (S05/F04) -----------------------------

    /// Is a sync run in progress for this queue?
    pub async fn sync_running(&self, queue_id: &str) -> bool {
        self.sync_runs.lock().await.contains_key(queue_id)
    }

    /// Run one sync over a periodic queue, inline, so the caller can
    /// await its result (the manual "Sync now" path and the tests).
    /// The 30 s tick spawns this instead, so a long re-download
    /// never blocks the tick.
    ///
    /// Rules, all enforced here and in `sync_one_file`:
    /// - only a `periodic` queue syncs (anything else is an error);
    /// - a queue being drained as a one-time queue never syncs at
    ///   the same time, and a second sync never starts while one is
    ///   running — sync never fights the one-time scheduler;
    /// - each file is HEAD-probed (through the configured proxy,
    ///   under the speed limiter when it re-downloads) and compared
    ///   with the stored size/ETag/Last-Modified; only a file that
    ///   differs is re-downloaded, byte-identical, over the same
    ///   save path;
    /// - a file the server answers 404 for is reported `removed`,
    ///   not silently dropped; a failed probe or re-download is
    ///   `failed` and is retried at the next sync, never in a loop;
    /// - the outcome is recorded on the queue (`record_sync`), so
    ///   the UI can show the last sync and its changed files.
    pub async fn run_sync_queue(self: &Arc<Self>, id: &str) -> Result<SyncResult> {
        let queue = self
            .store
            .get_queue(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if queue.schedule_type != "periodic" {
            return Err(EngineError::InvalidState(
                "This queue is not a periodic synchronisation queue — choose “Repeat regularly” for it first".to_string(),
            ));
        }
        if self.queue_runs.lock().await.contains_key(id) {
            return Err(EngineError::InvalidState(
                "This queue is already running".to_string(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut runs = self.sync_runs.lock().await;
            if runs.contains_key(id) {
                return Err(EngineError::InvalidState(
                    "This queue is already syncing".to_string(),
                ));
            }
            runs.insert(id.to_string(), Arc::clone(&stop));
        }
        let result = self.sync_queue_inner(id, &stop).await;
        self.sync_runs.lock().await.remove(id);
        result
    }

    async fn sync_queue_inner(
        self: &Arc<Self>,
        queue_id: &str,
        stop: &Arc<AtomicBool>,
    ) -> Result<SyncResult> {
        let settings = self.store.get_settings().unwrap_or_default();
        let mut result = SyncResult {
            queue_id: queue_id.to_string(),
            changed: 0,
            unchanged: 0,
            removed: 0,
            failed: 0,
            skipped: 0,
            files: Vec::new(),
        };
        for file in self.store.list_queue_files(queue_id)? {
            if stop.load(Ordering::SeqCst) {
                result.skipped += 1;
                result.files.push(SyncFileResult {
                    download_id: file.id,
                    filename: file.filename,
                    status: "skipped".to_string(),
                    message: Some(
                        "Sync stopped — the stop time arrived".to_string(),
                    ),
                });
                continue;
            }
            let outcome = self.sync_one_file(&file, &settings.excluded_sites).await;
            match outcome.status.as_str() {
                "changed" => result.changed += 1,
                "unchanged" => result.unchanged += 1,
                "removed" => result.removed += 1,
                "failed" => result.failed += 1,
                _ => result.skipped += 1,
            }
            result.files.push(outcome);
        }
        let _ = self.store.record_sync(
            queue_id,
            &unix_to_iso(now_unix()),
            result.changed as i64,
            result.unchanged as i64,
            result.removed as i64,
            result.failed as i64,
            result.skipped as i64,
        );
        Ok(result)
    }

    /// Probe one file and decide what sync does with it.
    async fn sync_one_file(
        self: &Arc<Self>,
        file: &DownloadRow,
        excluded_sites: &[String],
    ) -> SyncFileResult {
        let skipped = |message: &str| SyncFileResult {
            download_id: file.id.clone(),
            filename: file.filename.clone(),
            status: "skipped".to_string(),
            message: Some(message.to_string()),
        };
        let failed = |message: String| SyncFileResult {
            download_id: file.id.clone(),
            filename: file.filename.clone(),
            status: "failed".to_string(),
            message: Some(message),
        };
        // Excluded sites are respected before any network request —
        // the same refusal `add_download` gives, checked again here
        // because a sync probes directly instead of adding.
        if let Some(host) = url_host(&file.url) {
            if crate::store::site_is_excluded(&host, excluded_sites) {
                return skipped(&format!(
                    "This site is excluded in Settings (excluded sites): {host}"
                ));
            }
        }
        match file.status.as_str() {
            "complete" => {}
            // A re-download that failed on the last sync is retried
            // here: resume re-probes and continues (or restarts) it,
            // once per sync — never in a tight loop.
            "error" => return self.sync_retry_failed_file(file).await,
            // Mid-download sync choice: SKIP, do not touch it. A sync
            // never resets, pauses or rewrites a file that is
            // downloading — the download owns its segments and its
            // `.part` file until it finishes, and the sync re-checks
            // the file at the next run. (Pinned by
            // `sync_skips_a_file_that_is_mid_download_and_leaves_it_byte_identical`.)
            _ => {
                return skipped(
                    "This file has not finished downloading yet — start it first",
                )
            }
        }
        // A finished file whose local copy is gone is re-downloaded
        // whatever the probe says: there is nothing to keep.
        let local_missing = !Path::new(&file.save_path).exists();
        let info = match self.probe_url(&file.url).await {
            Ok(info) => info,
            Err(EngineError::Status(404, _)) => {
                return SyncFileResult {
                    download_id: file.id.clone(),
                    filename: file.filename.clone(),
                    status: "removed".to_string(),
                    message: Some(
                        "The file is no longer on the server (404) — it was not deleted locally"
                            .to_string(),
                    ),
                }
            }
            Err(e) => return failed(e.to_string()),
        };
        let changed = match remote_changed(&info, file) {
            Some(changed) => local_missing || changed,
            // The server sends no size and no validator at all, so
            // nothing can be compared: report that honestly instead
            // of re-downloading on every sync (or pretending the
            // file is unchanged when it may not be).
            None => {
                return skipped(
                    "Cannot tell whether this file changed — the server sends no size, ETag or Last-Modified",
                )
            }
        };
        if !changed {
            // Adopt any validator the server only just started
            // sending, so the next sync can compare against it.
            let _ = self.store.update_known_remote(
                &file.id,
                info.size.map(|s| s as i64),
                info.etag.as_deref(),
                info.last_modified.as_deref(),
            );
            return SyncFileResult {
                download_id: file.id.clone(),
                filename: file.filename.clone(),
                status: "unchanged".to_string(),
                message: None,
            };
        }
        match self.redownload_changed_file(file, &info).await {
            Ok(()) => SyncFileResult {
                download_id: file.id.clone(),
                filename: file.filename.clone(),
                status: "changed".to_string(),
                message: None,
            },
            Err(e) => failed(e),
        }
    }

    /// Retry a file whose last sync re-download failed. Resume
    /// re-probes first: a changed file restarts from zero, an
    /// unchanged one continues from its last checkpoint.
    async fn sync_retry_failed_file(self: &Arc<Self>, file: &DownloadRow) -> SyncFileResult {
        match self.resume_download(&file.id).await {
            Ok(()) => match self.wait_for_download(&file.id).await {
                Ok(()) => SyncFileResult {
                    download_id: file.id.clone(),
                    filename: file.filename.clone(),
                    status: "changed".to_string(),
                    message: Some(
                        "Re-downloaded after the previous attempt failed".to_string(),
                    ),
                },
                Err(e) => SyncFileResult {
                    download_id: file.id.clone(),
                    filename: file.filename.clone(),
                    status: "failed".to_string(),
                    message: Some(e),
                },
            },
            Err(e) => SyncFileResult {
                download_id: file.id.clone(),
                filename: file.filename.clone(),
                status: "failed".to_string(),
                message: Some(e.to_string()),
            },
        }
    }

    /// Re-download a changed file over its existing save path. The
    /// finished file on disk stays put until the new download
    /// renames its `.part` over it, so a failed re-download never
    /// destroys the copy the user already has. The re-download goes
    /// through the normal engine path (segments, speed limiter,
    /// proxy client), so it is byte-checked the same way.
    async fn redownload_changed_file(
        self: &Arc<Self>,
        file: &DownloadRow,
        info: &ProbeInfo,
    ) -> std::result::Result<(), String> {
        // Re-read the row: the user may have started the file by hand
        // after the sync listed it, and starting it twice would fail.
        if let Ok(Some(row)) = self.store.get_download(&file.id) {
            if row.status == "downloading" {
                return Err(
                    "This file started downloading while the sync was running — it will be checked again at the next sync"
                        .to_string(),
                );
            }
        }
        let part = part_path(&file.save_path);
        let _ = std::fs::remove_file(&part);
        self.store
            .delete_segments(&file.id)
            .map_err(|e| e.to_string())?;
        self.store
            .reset_download(
                &file.id,
                info.size.map(|s| s as i64),
                info.etag.as_deref(),
                info.last_modified.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        let ranges = initial_ranges(info, self.max_connections());
        self.store
            .create_segments(&file.id, &ranges)
            .map_err(|e| e.to_string())?;
        self.start_download(&file.id)
            .await
            .map_err(|e| e.to_string())?;
        self.wait_for_download(&file.id).await
    }

    /// Wait for a re-download to reach `complete` (or `error`),
    /// polling the stored status like the queue runner does.
    ///
    /// The pre-start status is stale until the run flips it: a file
    /// being retried after a failure still reads `error` (with the
    /// old message) until its new run sets `downloading`, so an
    /// `error` is only terminal when the run is no longer registered
    /// — otherwise the retry would report the previous failure at
    /// once and never actually re-download (BUG-020). `start_download`
    /// registers the run synchronously and `run()` unregisters it
    /// when done, so the running map is the exact guard here; the
    /// status alone cannot tell a stale error from a fresh one.
    async fn wait_for_download(&self, id: &str) -> std::result::Result<(), String> {
        for _ in 0..600 {
            // Up to 120 s — a re-download is a normal download, and
            // the sync must not report it before it is done.
            match self.store.get_download(id) {
                Ok(Some(row)) => match row.status.as_str() {
                    "complete" => return Ok(()),
                    // A re-download that paused is terminal for the
                    // waiter: without this, a download paused by the
                    // quota (S12) — or by the user mid-sync — would
                    // sit here for the whole 120 s timeout before
                    // the sync reported it. The sync retries it at
                    // the next run, like any other failure.
                    "paused" => {
                        return Err(if row.quota_paused {
                            "The re-download paused — the download quota is reached".to_string()
                        } else {
                            "The re-download was paused".to_string()
                        });
                    }
                    "error" => {
                        let still_running = self.running.lock().await.contains_key(id);
                        if !still_running {
                            return Err(row
                                .error_message
                                .unwrap_or_else(|| "The download failed".to_string()));
                        }
                        // Stale pre-start error: the new run is
                        // registered and has not flipped the status
                        // yet. Keep waiting.
                    }
                    _ => {}
                },
                _ => return Err("The download disappeared".to_string()),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err("The re-download did not finish in time".to_string())
    }

    // -- queue completion actions -----------------------------------------

    pub async fn pending_action(&self, queue_id: &str) -> Option<PendingAction> {
        self.pending_actions.lock().await.get(queue_id).cloned()
    }

    /// Register a completion action for a queue id and tell the UI
    /// about it (the `queue-action-pending` event). This is the ONE
    /// place a `PendingAction` is created: the queue drain and the
    /// site grabber both arm through it, so the 60 s countdown
    /// dialog, the execute path and the cancel path are the same
    /// for both — there is no second countdown. The key is the
    /// queue id, so `pending_action`, `execute_queue_action` and
    /// `dismiss_queue_action` all work on a grabber-armed action
    /// unchanged, and the app's startup poll (`get_pending_action`
    /// per queue) surfaces a countdown left waiting from a run
    /// that ended before the window opened.
    async fn register_completion_action(
        &self,
        queue_id: &str,
        queue_name: String,
        action: &str,
        program: Option<String>,
        file_path: Option<String>,
    ) {
        let pending = PendingAction {
            queue_id: queue_id.to_string(),
            queue_name,
            action: action.to_string(),
            program,
            file_path,
            created_at_unix: now_unix(),
        };
        self.pending_actions
            .lock()
            .await
            .insert(queue_id.to_string(), pending.clone());
        let _ = self.action_events.send(pending);
    }

    /// Run a confirmed completion action (called by the UI after the
    /// countdown, or at once for open-file / run-program).
    pub async fn execute_queue_action(&self, queue_id: &str) -> Result<()> {
        let action = self
            .pending_actions
            .lock()
            .await
            .remove(queue_id)
            .ok_or_else(|| EngineError::NotFound(queue_id.to_string()))?;
        crate::queue::execute_action(&action)
    }

    /// The user cancelled the countdown: drop the action, run nothing.
    pub async fn dismiss_queue_action(&self, queue_id: &str) -> Result<()> {
        self.pending_actions.lock().await.remove(queue_id);
        Ok(())
    }

    /// Drain one queue, one file at a time, in queue order.
    /// A finished file leaves the queue; a failed file stops the run
    /// (it stays in the queue with its error); stopping pauses the
    /// current file and ends the run. When the queue empties cleanly,
    /// the one-time schedule is marked done and the completion action
    /// (if any) is registered for the UI countdown.
    async fn run_queue(self: Arc<Self>, queue_id: String, stop: Arc<AtomicBool>) {
        let mut last_completed: Option<String> = None;
        let mut completed_any = false;
        let mut stopped_early = false;
        let mut blocked_by_error = false;
        loop {
            if stop.load(Ordering::SeqCst) {
                stopped_early = true;
                break;
            }
            let files = match self.store.list_queue_files(&queue_id) {
                Ok(f) => f,
                Err(_) => break,
            };
            let Some(next) = files
                .iter()
                .find(|d| matches!(d.status.as_str(), "queued" | "paused" | "downloading" | "error" | "complete"))
            else {
                break; // nothing left to do
            };
            let next = next.clone();
            let started_from = next.status.clone();
            match next.status.as_str() {
                "error" => {
                    blocked_by_error = true;
                    break;
                }
                // A finished file does not belong in a queue (it got
                // there by a direct start while queued, or from data
                // made before the add_to_queue guard): drop it from
                // the queue and move on. It must not count as a
                // download, or the completion action would fire after
                // the queue downloaded nothing.
                "complete" => {
                    let _ = self.store.remove_from_queue(&next.id);
                    continue;
                }
                "queued" => {
                    if self.start_download(&next.id).await.is_err() {
                        blocked_by_error = true;
                        break;
                    }
                }
                "paused" => {
                    if self.resume_download(&next.id).await.is_err() {
                        blocked_by_error = true;
                        break;
                    }
                }
                "downloading" => { /* already running — just wait for it */ }
                _ => continue,
            }
            // Wait for this file to finish, fail, or be paused.
            // On stop we pause the file AND wait for the pause to
            // land: if the run ended while the download task was
            // still shutting down, a later start_queue would collide
            // with the still-registered run ("already running") and
            // the queue would stall with its files stuck.
            //
            // The run task flips the status to "downloading"
            // asynchronously, so the pre-start status is stale until
            // we have seen "downloading": without this guard, a
            // resumed file still reading "paused" on the first poll
            // would end the run at once — and the outer loop would
            // then resume it a second time.
            let id = next.id.clone();
            let mut paused_by_stop = false;
            let mut seen_downloading = started_from == "downloading";
            loop {
                if stop.load(Ordering::SeqCst) && !paused_by_stop {
                    let _ = self.pause_download(&id).await;
                    paused_by_stop = true;
                }
                match self.store.get_download(&id) {
                    Ok(Some(row)) => match row.status.as_str() {
                        "downloading" => {
                            seen_downloading = true;
                        }
                        "complete" => {
                            last_completed = Some(row.save_path);
                            completed_any = true;
                            let _ = self.store.remove_from_queue(&id);
                            break;
                        }
                        "paused" | "deleted" => {
                            if row.status == started_from && !seen_downloading {
                                // Stale pre-start status: the run
                                // task has not flipped it yet.
                            } else {
                                if paused_by_stop {
                                    stopped_early = true;
                                }
                                break;
                            }
                        }
                        "error" => {
                            blocked_by_error = true;
                            break;
                        }
                        _ => {}
                    },
                    _ => break,
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if stopped_early || blocked_by_error {
                break;
            }
        }
        self.queue_runs.lock().await.remove(&queue_id);
        if stopped_early || blocked_by_error {
            return;
        }
        // The queue drained cleanly.
        if let Ok(Some(queue)) = self.store.get_queue(&queue_id) {
            if queue.schedule_type == "one_time" {
                self.scheduled_done.lock().await.insert(queue_id.clone());
            }
            if completed_any {
                // Sounds (S09): a clean drain that downloaded
                // something plays the queue-finished event (a drain
                // of nothing plays nothing — the BUG-003 rule).
                self.emit_sound("queue_finished");
                // Dial-Up (S09): "hang up after completion" maps
                // onto this same action path, but only when the
                // queue has no other completion action — the
                // queue's own action wins, and a queue never arms
                // two actions at once. The connection name travels
                // in the program slot (see `queue::execute_action`).
                let settings = self.store.get_settings().unwrap_or_default();
                let effective = effective_completion_action(
                    &queue.on_complete_action,
                    settings.dialup_hangup_after_completion,
                );
                if let Some(action) = effective {
                    let program = if action == "hangup" {
                        settings.dialup_connection_name.clone()
                    } else {
                        queue.on_complete_program.clone()
                    };
                    self.register_completion_action(
                        &queue_id,
                        queue.name,
                        action,
                        program,
                        last_completed,
                    )
                    .await;
                }
            }
        }
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRow>> {
        self.store.list_downloads()
    }

    pub fn get_download(&self, id: &str) -> Result<Option<DownloadRow>> {
        self.store.get_download(id)
    }

    /// Start (or restart) the download loop for `id` in the background.
    ///
    /// The download quota (S12) gates the start: an expired window
    /// is rolled first (so a start after the window end is never
    /// refused for a window that is already over), and a reached
    /// quota refuses the start with the window end in the message
    /// — the download stays queued, and the queue runner stops
    /// there instead of failing the file.
    pub async fn start_download(self: &Arc<Self>, id: &str) -> Result<()> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.status == "complete" {
            return Err(EngineError::InvalidState("This download is already complete".to_string()));
        }
        // Download quota (S12): roll an expired window, then refuse
        // a start while the quota is reached. The message names the
        // resume time so the UI can show it without a second call.
        let now = now_unix();
        let _ = self.maybe_rollover_quota_at(now);
        if self.quota_reached_at(now)? {
            let status = self.get_quota_status_at(now)?;
            let resume = status
                .window_end_unix
                .map(unix_to_iso)
                .unwrap_or_else(|| "when the quota window ends".to_string());
            return Err(EngineError::InvalidState(format!(
                "The download quota is reached ({} MB every {} hours) — downloads resume at {resume}",
                status.limit_mb.unwrap_or(0),
                status.window_hours.unwrap_or(0),
            )));
        }
        {
            let running = self.running.lock().await;
            if running.contains_key(id) {
                return Err(EngineError::InvalidState("This download is already running".to_string()));
            }
        }
        // A start clears the quota-paused flag: the download is
        // running again, whether it was started by the rollover, a
        // reset, or the user. (A quota-paused download started by
        // hand while the quota is reached never gets here — the
        // gate above refuses it first.)
        let _ = self.store.set_quota_paused(id, false);
        // Per-download cap (S12): this run's own token bucket, read
        // from the row at start time — the effective rate is the min
        // of this bucket and the global one.
        let per_download_limiter = Arc::new(RateLimiter::new(
            row.speed_limit_kbps
                .map(|k| (k.max(0) as u64).saturating_mul(1024))
                .unwrap_or(0),
        ));
        let pause = Arc::new(AtomicBool::new(false));
        let retry = Arc::new(RetrySlot::new());
        // Shared last-progress clock (§7.2, Round 56): the
        // supervisor loop in `run_inner` stamps this whenever the
        // downloaded total grows, and the independent watchdog
        // task below reads it from the map entry. Initialised to
        // the run's start, so a download that never receives its
        // first byte still has a well-defined stall clock.
        let last_progress_ms = Arc::new(AtomicU64::new(now_millis()));
        let this = Arc::clone(self);
        let id = id.to_string();
        let pause_clone = Arc::clone(&pause);
        let retry_clone = Arc::clone(&retry);
        let limiter_clone = Arc::clone(&per_download_limiter);
        let progress_clone = Arc::clone(&last_progress_ms);
        let handle = Arc::new(tokio::spawn(async move {
            this.run(id, pause_clone, retry_clone, limiter_clone, progress_clone)
                .await;
        }));
        // The watchdog's id, cloned before the insert below moves
        // `row.id` into the map key.
        let watch_id = row.id.clone();
        // Round 57 (§9.2f): the thread watchdog's entry — the same
        // shared state as the `running` map entry, built before the
        // insert moves the originals in.
        let thread_entry = Arc::new(ThreadWatchEntry {
            last_progress_ms: Arc::clone(&last_progress_ms),
            retry: Arc::clone(&retry),
            pause: Arc::clone(&pause),
            handle: Arc::clone(&handle),
        });
        self.running.lock().await.insert(
            row.id,
            Running {
                pause,
                handle,
                retry,
                per_download_limiter,
                last_progress_ms,
            },
        );
        self.thread_watchdogs
            .lock()
            .unwrap()
            .insert(watch_id.clone(), Arc::clone(&thread_entry));
        // Round 56 (§7.2): the independent run watchdog — a
        // separate task per run that watches the shared
        // last-progress timestamp and aborts the run after
        // `run_watchdog_timeout` of no progress. It is a different
        // task from the run itself, so it keeps running even when
        // the supervisor loop wedges (which is exactly what it is
        // watching for). It reads the CURRENT map entry, so a stale
        // watchdog from an earlier run of the same download can
        // never abort a newer run that is making progress.
        {
            let watch_mgr = Arc::clone(self);
            let thread_id = watch_id.clone();
            tokio::spawn(async move {
                watch_mgr.run_watchdog(watch_id).await;
            });
            // Round 57 (§9.2f): the truly-independent watchdog —
            // a plain `std::thread`, NOT a tokio task. The Round
            // 56 tokio watchdog is a task on the same runtime it
            // watches: when the runtime wedges (two simultaneous
            // blocking calls on the VPS's 2 workers), its timers
            // never fire either. This thread sleeps on the OS, so
            // it keeps checking while the runtime is frozen. On a
            // stall it logs + aborts the run exactly like the
            // tokio watchdog — it never restarts the process
            // (auto-restart is a pending user decision).
            let thread_mgr = Arc::clone(self);
            let timeout_secs = self.run_watchdog_timeout_secs();
            std::thread::spawn(move || {
                Self::thread_watchdog(thread_mgr, thread_id, thread_entry, timeout_secs);
            });
        }
        Ok(())
    }

    /// The retry wait of a running download, if a segment is in its
    /// backoff (S03). `None` when the download is not running or no
    /// segment is waiting — the dialog polls this only as a fallback;
    /// live progress events carry the same state.
    pub async fn get_retry_state(&self, id: &str) -> Option<RetryInfo> {
        let running = self.running.lock().await;
        running.get(id).map(|r| r.retry.info()).unwrap_or(None)
    }

    /// Skip the backoff of a download that is waiting for a retry
    /// (S03, "Retry now"): the waiting segment tries again at once.
    /// Errors when the download is not running, or when no segment
    /// is currently waiting — there is nothing to hurry then.
    pub async fn retry_now(&self, id: &str) -> Result<()> {
        let running = self.running.lock().await;
        let r = running
            .get(id)
            .ok_or_else(|| EngineError::InvalidState("This download is not running".to_string()))?;
        if r.retry.info().is_none() {
            return Err(EngineError::InvalidState(
                "This download is not waiting for a retry".to_string(),
            ));
        }
        r.retry.retry_now.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// The folder a finished download lives in, for "Open folder"
    /// (S03). Only the validation lives here so it can be tested
    /// without opening anything: the download must exist, and the
    /// folder must still be on disk — a folder deleted (or moved)
    /// after the download finished is a clear error, not a silent
    /// no-op and not a crash.
    pub fn folder_for_download(&self, id: &str) -> Result<PathBuf> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        let folder = Path::new(&row.save_path)
            .parent()
            .map(|p| p.to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                EngineError::InvalidState(format!(
                    "The file \"{}\" has no folder to open",
                    row.filename
                ))
            })?;
        if !folder.is_dir() {
            return Err(EngineError::InvalidState(format!(
                "The folder no longer exists: {}",
                folder.display()
            )));
        }
        Ok(folder)
    }

    /// Open the folder of a finished download in the system file
    /// manager (S03). Validation first (`folder_for_download`), then
    /// the same platform opener the queue completion action uses
    /// (`cmd /C start` on Windows, `open` on macOS, `xdg-open` on
    /// Linux) — no second opener was written.
    pub fn open_folder(&self, id: &str) -> Result<()> {
        let folder = self.folder_for_download(id)?;
        crate::queue::open_path(&folder.to_string_lossy())
    }

    // -- download history (Round 53) --------------------------------------

    /// History rows, newest first; `query` filters by filename or
    /// URL (empty = everything). A thin pass-through to the store.
    pub fn list_history(&self, query: &str) -> Result<Vec<HistoryRow>> {
        self.store.list_history(query)
    }

    /// Empty the whole download history. Downloads are untouched.
    pub fn clear_history(&self) -> Result<()> {
        self.store.clear_history()
    }

    /// Re-download a finished file from its history row: a fresh
    /// download through the REAL `add_download` path (probe,
    /// excluded-sites refusal, duplicate handling), from the final
    /// URL when the probe followed a redirect, otherwise the
    /// original URL — into the same folder, under the same name
    /// and category. The history row itself is untouched, so the
    /// same entry can be re-downloaded again.
    pub async fn redownload_from_history(
        self: &Arc<Self>,
        history_id: &str,
    ) -> Result<(String, ProbeInfo)> {
        let entry = self
            .store
            .get_history_entry(history_id)?
            .ok_or_else(|| EngineError::NotFound(history_id.to_string()))?;
        let url = entry.final_url.as_deref().unwrap_or(&entry.url);
        self.add_download(
            url,
            Some(entry.filename),
            Some(entry.folder),
            entry.category_id,
            None,
        )
        .await
    }

    /// Open the folder a history entry's file lives in, with the
    /// same platform opener `open_folder` uses. The entry must
    /// exist, and the folder must still be on disk — a folder
    /// deleted (or moved) after the download finished is a clear
    /// error, not a silent no-op.
    pub fn open_history_folder(&self, history_id: &str) -> Result<()> {
        let entry = self
            .store
            .get_history_entry(history_id)?
            .ok_or_else(|| EngineError::NotFound(history_id.to_string()))?;
        let folder = Path::new(&entry.folder);
        if !folder.is_dir() {
            return Err(EngineError::InvalidState(format!(
                "The folder no longer exists: {}",
                folder.display()
            )));
        }
        crate::queue::open_path(&entry.folder)
    }

    // -- antivirus hook (S14) --------------------------------------------

    /// The stored scan result for a download, or `None` when it
    /// was never scanned.
    pub fn get_scan_result(&self, id: &str) -> Result<Option<ScanResult>> {
        self.store.get_scan_result(id)
    }

    fn begin_scan(&self, id: &str) -> bool {
        self.scanning.lock().unwrap().insert(id.to_string())
    }

    fn end_scan(&self, id: &str) {
        self.scanning.lock().unwrap().remove(id);
    }

    /// Run the configured scanner against a finished download's
    /// file and record the result on the row (S14). Only a
    /// `complete` download can be scanned — the scanner must see
    /// the finished file, never a partial one. A second scan
    /// while one is already running is refused. The result is
    /// re-emitted as a completion progress event, so a dialog
    /// open on the download shows the scan outcome without a
    /// refresh. A scan outcome is never an error return: a threat,
    /// a failure and a timeout are all results the dialog shows.
    pub async fn run_antivirus_scan(self: &Arc<Self>, id: &str) -> Result<ScanResult> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.status != "complete" {
            return Err(EngineError::InvalidState(
                "Only a finished download can be scanned — this one is still downloading or has not finished".to_string(),
            ));
        }
        let settings = self.store.get_settings()?;
        // No scanner (or scanning off) is a stored result, not an
        // error — and it is re-emitted, so a dialog open on the
        // download shows it without a refresh.
        if !settings.antivirus_enabled {
            let result = ScanResult::not_configured("Antivirus scanning is turned off in Settings.");
            let _ = self.store.set_scan_result(id, &result);
            self.emit_snapshot(id, "complete").await;
            return Ok(result);
        }
        let Some(command) = settings
            .antivirus_command
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .map(str::to_string)
        else {
            let result =
                ScanResult::not_configured("No antivirus scanner is configured in Settings.");
            let _ = self.store.set_scan_result(id, &result);
            self.emit_snapshot(id, "complete").await;
            return Ok(result);
        };
        if !self.begin_scan(id) {
            return Err(EngineError::InvalidState(
                "A scan is already running for this download".to_string(),
            ));
        }
        let result = run_scan(
            &command,
            Path::new(&row.save_path),
            settings.antivirus_timeout_secs,
        )
        .await;
        self.end_scan(id);
        let _ = self.store.set_scan_result(id, &result);
        // The dialog (and the main list, via the event) learns the
        // outcome without polling: a fresh completion event now
        // carries the finished scan result.
        self.emit_snapshot(id, "complete").await;
        Ok(result)
    }

    /// The Settings > Downloads "Test" button (S14): run the
    /// command the user typed — saved or not — against a small
    /// throwaway file and report the result. The timeout comes
    /// from the dialog (the settings value when saved). A missing
    /// command is an error here, so the button cannot report a
    /// success for a scanner that was never run.
    pub async fn test_antivirus_scanner(
        &self,
        command: &str,
        timeout_secs: i64,
    ) -> Result<ScanResult> {
        test_scanner(command, timeout_secs).await
    }

    /// Open a finished download in its default application (S14
    /// completion dialog). Refused — with a clear error, not a
    /// silent no-op — when the scanner flagged the file as a
    /// threat: a threat is never auto-opened. The file must still
    /// exist; a deleted file is a clear error too.
    pub fn open_file(&self, id: &str) -> Result<()> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.status != "complete" {
            return Err(EngineError::InvalidState(
                "Only a finished download can be opened".to_string(),
            ));
        }
        if row.scan_status.as_deref() == Some(SCAN_INFECTED) {
            return Err(EngineError::InvalidState(
                "The antivirus scan found a threat in this file, so it will not be opened. Delete it or quarantine it from the completion dialog.".to_string(),
            ));
        }
        if !Path::new(&row.save_path).is_file() {
            return Err(EngineError::InvalidState(format!(
                "The file no longer exists: {}",
                row.save_path
            )));
        }
        crate::queue::open_path(&row.save_path)
    }

    /// Quarantine a finished download the scanner flagged as a
    /// threat (S14): move the file into a `Quarantine` folder next
    /// to where it was downloaded and point the row at the new
    /// location. Explicit user action only — nothing ever calls
    /// this on its own, and a file that was not flagged is
    /// refused. Returns the new path.
    pub fn quarantine_download(&self, id: &str) -> Result<String> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.scan_status.as_deref() != Some(SCAN_INFECTED) {
            return Err(EngineError::InvalidState(
                "Only a file the antivirus scan flagged as a threat can be quarantined".to_string(),
            ));
        }
        let path = Path::new(&row.save_path);
        if !path.is_file() {
            return Err(EngineError::InvalidState(format!(
                "The file no longer exists: {}",
                row.save_path
            )));
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| {
            EngineError::InvalidState(format!(
                "The file \"{}\" has no folder to quarantine it from",
                row.filename
            ))
        })?;
        let quarantine_dir = parent.join("Quarantine");
        std::fs::create_dir_all(&quarantine_dir)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| row.filename.clone());
        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut dest = quarantine_dir.join(format!("{stem}{ext}"));
        let mut n = 2;
        while dest.exists() {
            dest = quarantine_dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::rename(path, &dest)?;
        let dest_str = dest.to_string_lossy().to_string();
        self.store.update_save_path(id, &dest_str)?;
        Ok(dest_str)
    }

    /// Ask a running download to stop at the next chunk boundary, then
    /// persist its progress. The run loop sets status `paused`.
    pub async fn pause_download(&self, id: &str) -> Result<()> {
        let running = self.running.lock().await;
        match running.get(id) {
            Some(r) => {
                r.pause.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err(EngineError::InvalidState("This download is not running".to_string())),
        }
    }

    /// Resume a paused (or failed) download. The server is re-probed
    /// first; a changed file restarts the download from zero.
    pub async fn resume_download(self: &Arc<Self>, id: &str) -> Result<()> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.status == "downloading" {
            // Round 55 (defect 4): a row can say `downloading`
            // while no run exists for it — a wedged run, or a crash
            // before startup reconciliation ran. That is a stale
            // status, not a running download: reset it and resume
            // instead of refusing with "already running" (which is
            // what made Resume a silent no-op on wedged rows).
            // A genuinely running download still refuses.
            let running = self.running.lock().await;
            if running.contains_key(id) {
                return Err(EngineError::InvalidState(
                    "This download is already running".to_string(),
                ));
            }
            drop(running);
            let _ = self.store.set_status(
                id,
                "paused",
                Some("Interrupted — resuming from the bytes already downloaded"),
            );
        }
        if row.status == "complete" {
            return Err(EngineError::InvalidState("This download is already complete".to_string()));
        }
        // Browser cookies (S13 finish): a paused cookie download
        // resumes with its cookies — without them the re-probe
        // would fail with 401 and the resume could never work.
        let cookies = self.cookies.lock().unwrap().get(id).cloned();
        let info = self
            .probe_url_with_cookies(&row.url, cookies.as_deref())
            .await
            .map_err(|e| {
                let _ = self.store.set_status(id, "error", Some(&e.to_string()));
                e
            })?;
        let changed = info.size.map(|s| s as i64) != row.size_bytes
            || (info.etag.is_some()
                && row.etag.is_some()
                && info.etag != row.etag);
        if changed {
            // The remote file changed: never stitch old bytes onto new content.
            let part = part_path(&row.save_path);
            let _ = std::fs::remove_file(&part);
            self.store.delete_segments(id)?;
            self.store.reset_download(
                id,
                info.size.map(|s| s as i64),
                info.etag.as_deref(),
                info.last_modified.as_deref(),
            )?;
            let ranges = initial_ranges(&info, self.max_connections());
            self.store.create_segments(id, &ranges)?;
        }
        self.start_download(id).await
    }

    /// Refresh the address of a paused or failed download (Round
    /// 51): an expired link can be replaced without losing
    /// progress. The new URL is re-probed FIRST through the same
    /// probe path `add_download` uses (the saved site login for
    /// the new host, the active proxy, the download's browser
    /// cookies when they belong to the same host):
    /// - the probe fails → error, and the old URL and all progress
    ///   are kept untouched;
    /// - the probe shows the SAME file (size and ETag — the same
    ///   rule `resume_download` uses) → the row is pointed at the
    ///   new address and the download starts again from the bytes
    ///   already downloaded;
    /// - the probe shows a DIFFERENT file → `Mismatch` is returned
    ///   and nothing is changed; only an explicit confirm
    ///   (`force = true`) switches, and then the download restarts
    ///   from zero — never stitching old bytes onto new content.
    /// Only a paused or failed download can be refreshed; a running
    /// or finished one is refused. The quota gate is checked before
    /// anything is switched, so a refresh never leaves a switched
    /// row that cannot start.
    pub async fn refresh_download_address(
        self: &Arc<Self>,
        id: &str,
        new_url: &str,
        force: bool,
    ) -> Result<RefreshAddressResult> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        if row.status != "paused" && row.status != "error" {
            return Err(EngineError::InvalidState(format!(
                "Only a paused or failed download can get a new address (this one is {})",
                row.status
            )));
        }
        let new_url = new_url.trim();
        if new_url.is_empty() {
            return Err(EngineError::InvalidUrl("Enter a new link".to_string()));
        }
        // BUG-011: an excluded site is refused before any network
        // request, like `add_download`.
        if let Some(host) = url_host(new_url) {
            let settings = self.store.get_settings().unwrap_or_default();
            if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
                return Err(EngineError::InvalidState(format!(
                    "This site is excluded in Settings (excluded sites): {host}"
                )));
            }
        }
        // The run task removes itself from `running` just after it
        // records the pause; a refresh landing in that gap would
        // switch the URL and then fail to start — wait it out
        // (bounded) instead of erroring on a race the user did not
        // cause.
        for _ in 0..20 {
            if !self.running.lock().await.contains_key(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if self.running.lock().await.contains_key(id) {
            return Err(EngineError::InvalidState(
                "This download is still stopping — try again in a moment".to_string(),
            ));
        }
        // The quota gate first: a refresh that switches the row and
        // then cannot start would strand the new address on a paused
        // download — refuse up front instead.
        let now = now_unix();
        let _ = self.maybe_rollover_quota_at(now);
        if self.quota_reached_at(now)? {
            let status = self.get_quota_status_at(now)?;
            let resume = status
                .window_end_unix
                .map(unix_to_iso)
                .unwrap_or_else(|| "when the quota window ends".to_string());
            return Err(EngineError::InvalidState(format!(
                "The download quota is reached ({} MB every {} hours) — downloads resume at {resume}",
                status.limit_mb.unwrap_or(0),
                status.window_hours.unwrap_or(0),
            )));
        }
        // Browser cookies (S13 finish): a paused cookie download
        // carries its session to the new address — but only when
        // the new address is on the SAME host; another host must
        // never receive this site's cookies.
        let cookies = self.cookies.lock().unwrap().get(id).cloned();
        let same_host = url_host(new_url) == url_host(&row.url);
        let cookies = if same_host { cookies } else { None };
        let info = self
            .probe_url_with_cookies(new_url, cookies.as_deref())
            .await?;
        let changed = info.size.map(|s| s as i64) != row.size_bytes
            || (info.etag.is_some() && row.etag.is_some() && info.etag != row.etag);
        if changed && !force {
            return Ok(RefreshAddressResult::Mismatch {
                size_bytes: info.size.map(|s| s as i64),
                etag: info.etag,
                filename: info
                    .filename
                    .or_else(|| filename_from_url(&info.final_url)),
            });
        }
        if changed {
            // Confirmed different file: never stitch old bytes onto
            // new content — the same restart `resume_download`
            // does when the remote file changes: drop the partial
            // file and the old segments, reset the row, and lay out
            // fresh segments for the new file.
            let part = part_path(&row.save_path);
            let _ = std::fs::remove_file(&part);
            self.store.delete_segments(id)?;
            self.store.reset_download(
                id,
                info.size.map(|s| s as i64),
                info.etag.as_deref(),
                info.last_modified.as_deref(),
            )?;
            let ranges = initial_ranges(&info, self.max_connections());
            self.store.create_segments(id, &ranges)?;
        }
        self.store.update_download_address(
            id,
            new_url,
            Some(&info.final_url),
            info.size.map(|s| s as i64),
            info.etag.as_deref(),
            info.last_modified.as_deref(),
            info.accept_ranges,
        )?;
        self.start_download(id).await?;
        Ok(RefreshAddressResult::Switched {
            kept_bytes: if changed { 0 } else { row.downloaded_bytes },
            size_bytes: info.size.map(|s| s as i64),
        })
    }

    /// Clipboard monitoring (Round 52): check clipboard text for a
    /// fresh download link and, when there is one, return the offer
    /// the shell shows ("Download this link?"). Copying a link never
    /// downloads anything — the user must accept the offer. Returns
    /// `None` when monitoring is off, the text is not a file link,
    /// the same text was already seen, the URL was already offered
    /// or dismissed, the host is excluded, or the URL is already
    /// downloading/downloaded. An offered URL is remembered, so a
    /// second call with the same text (or a re-copy of the same
    /// link) offers nothing.
    pub fn check_clipboard_offer(&self, text: &str) -> Result<Option<ClipboardOffer>> {
        let settings = self.store.get_settings().unwrap_or_default();
        if !settings.clipboard_monitoring {
            return Ok(None);
        }
        let Some(offer) = extract_download_link(text) else {
            return Ok(None);
        };
        let mut monitor = self.clipboard.lock().unwrap();
        let hash = ClipboardMonitor::hash_text(text.trim());
        if hash == monitor.last_text_hash {
            return Ok(None);
        }
        monitor.last_text_hash = hash;
        if monitor.seen.iter().any(|u| u == &offer.url) {
            return Ok(None);
        }
        let host = url_host(&offer.url).unwrap_or_default();
        if crate::store::site_is_excluded(&host, &settings.excluded_sites) {
            return Ok(None);
        }
        if self.store.url_exists(&offer.url)? {
            return Ok(None);
        }
        monitor.remember(&offer.url);
        Ok(Some(offer))
    }

    /// Clipboard monitoring (Round 52): record a URL as dismissed
    /// (or already handled), so it is never offered again this
    /// session. Infallible and idempotent.
    pub fn dismiss_clipboard_offer(&self, url: &str) {
        self.clipboard.lock().unwrap().remember(url);
    }

    /// Stop a download and mark it deleted; the partial file is removed.
    pub async fn cancel_download(&self, id: &str) -> Result<()> {
        let row = self
            .store
            .get_download(id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        {
            let running = self.running.lock().await;
            if let Some(r) = running.get(id) {
                r.pause.store(true, Ordering::SeqCst);
                r.handle.abort();
            }
        }
        let _ = std::fs::remove_file(part_path(&row.save_path));
        self.store.set_status(id, "deleted", None)?;
        self.running.lock().await.remove(id);
        // Round 57 (§9.2f): a cancelled run has no thread
        // watchdog any more — the thread exits on its own when
        // its entry disappears.
        self.thread_watchdogs.lock().unwrap().remove(id);
        // Browser cookies (S13 finish): a deleted download keeps no
        // session behind.
        self.cookies.lock().unwrap().remove(id);
        Ok(())
    }

    // -- the run loop ------------------------------------------------------

    async fn run(
        self: Arc<Self>,
        id: String,
        pause: Arc<AtomicBool>,
        retry: Arc<RetrySlot>,
        per_download_limiter: Arc<RateLimiter>,
        // Shared last-progress clock (§7.2, Round 56), passed
        // through to `run_inner` so the supervisor loop can stamp
        // it — the same Arc the `running` map entry and the
        // watchdog task hold.
        last_progress_ms: Arc<AtomicU64>,
    ) {
        let outcome = self
            .run_inner(
                &id,
                &pause,
                &retry,
                &per_download_limiter,
                &last_progress_ms,
            )
            .await;
        // The run is over for every outcome: paused, complete, error.
        // No segment is waiting any more, whatever the outcome was.
        retry.clear_wait();
        self.running.lock().await.remove(&id);
        // Round 57 (§9.2f): the thread watchdog's entry goes with
        // the run — without this the thread would keep watching a
        // finished run (and, on a later stall of a NEW run of the
        // same download, could mistake the old entry for the
        // current one; the `ptr_eq` check below is the backstop).
        self.thread_watchdogs.lock().unwrap().remove(&id);
        if let Err(e) = outcome {
            let _ = self.store.set_status(&id, "error", Some(&e.to_string()));
            self.emit_snapshot(&id, "error").await;
        }
        // Sounds (S09): read the stored status, not the outcome —
        // `run_inner` also returns Ok for a pause, and a pause is
        // neither a completion nor a failure, so it plays nothing.
        // Browser cookies (S13 finish) follow the same read: a
        // terminal status (complete / error) drops them — a restart
        // would drop them anyway, and a stale session must never be
        // reused. A pause keeps them, so pause/resume in one
        // session keeps working.
        match self.store.get_download(&id).ok().flatten().map(|r| r.status) {
            Some(s) if s == "complete" => {
                self.cookies.lock().unwrap().remove(&id);
                self.emit_sound("download_complete");
            }
            Some(s) if s == "error" => {
                self.cookies.lock().unwrap().remove(&id);
                self.emit_sound("download_failed");
            }
            _ => {}
        }
    }

    /// Independent run watchdog (§7.2, Round 56): a separate task
    /// per run that watches the shared last-progress timestamp and
    /// aborts the run after `run_watchdog_timeout` (default 180 s)
    /// of no progress. It exists because the supervisor loop in
    /// `run_inner` can itself wedge — the in-loop watchdog then
    /// never fires either — while a task that only reads a
    /// timestamp keeps running. It reads the CURRENT `running` map
    /// entry on every check, so a stale watchdog from an earlier
    /// run of the same download can never abort a newer run that
    /// is making progress; and it returns as soon as the entry is
    /// gone, so a finished run's watchdog exits on its own.
    /// Suppressed while a segment waits out its retry backoff (a
    /// waiting segment is not a wedged run), exactly like the
    /// in-loop watchdog. The abort mirrors `cancel_download` —
    /// pause flag, handle abort, retry cleared, error status,
    /// snapshot — so a later Resume can restart the download.
    /// Order matters: the map entry is removed FIRST (that is what
    /// unblocks Resume/start), then the run is aborted, then the
    /// row is marked error. If the store itself is wedged, the
    /// status write blocks this task — but the entry removal and
    /// the abort already happened, and `resume_download`'s
    /// stale-row recovery then applies to the row.
    /// No sound is played here: `emit_sound` reads the store, and
    /// a watchdog that is aborting a wedged run must not block on
    /// the store for a cosmetic event.
    async fn run_watchdog(self: Arc<Self>, id: String) {
        let timeout_secs = self.run_watchdog_timeout_secs();
        let timeout = Duration::from_secs(timeout_secs);
        // Check often enough to fire near the deadline without
        // spinning: a sixth of the timeout, clamped to 1–10 s.
        let check_every = ((timeout_secs / 6).max(1)).min(10);
        loop {
            tokio::time::sleep(Duration::from_secs(check_every)).await;
            let entry = {
                let running = self.running.lock().await;
                match running.get(&id) {
                    Some(r) => (
                        r.last_progress_ms.load(Ordering::Relaxed),
                        r.retry.info().is_some(),
                    ),
                    None => return, // run ended — nothing to watch
                }
            };
            let (last_ms, retry_waiting) = entry;
            if retry_waiting {
                continue;
            }
            if now_millis().saturating_sub(last_ms) < timeout.as_millis() as u64 {
                continue;
            }
            // Stalled. Read the row first (store) to decide whether
            // an error status applies: a run whose row is already
            // complete/paused/errored only has a leaked map entry,
            // and touching its status would be wrong.
            let row_status = self
                .store
                .get_download(&id)
                .ok()
                .flatten()
                .map(|r| r.status);
            let entry = self.running.lock().await.remove(&id);
            // Round 57 (§9.2f): the thread watchdog's entry goes
            // with the run, so the thread exits instead of firing
            // a second time on a run this watchdog already aborted.
            self.thread_watchdogs.lock().unwrap().remove(&id);
            let Some(r) = entry else { return };
            if row_status.as_deref() == Some("downloading") {
                r.pause.store(true, Ordering::SeqCst);
                r.handle.abort();
                r.retry.clear_wait();
                let msg = format!(
                    "No data arrived for {timeout_secs} seconds — the connection appears to have stalled"
                );
                if let Err(e) = self.store.set_status(&id, "error", Some(&msg)) {
                    self.store.log(&format!(
                        "run watchdog could not set error status for {id}: {e}"
                    ));
                }
                self.emit_snapshot(&id, "error").await;
                self.cookies.lock().unwrap().remove(&id);
                self.store.log(&format!(
                    "run watchdog aborted {id} after {timeout_secs}s without progress"
                ));
            } else {
                // Stale entry for a finished/paused/errored run:
                // drop it so it can never make the download look
                // running, and abort its handle for good measure.
                r.handle.abort();
            }
            return;
        }
    }

    /// Truly-independent run watchdog (§9.2f, Round 57): a plain
    /// `std::thread` (NOT a tokio task) that watches one run's
    /// shared last-progress timestamp and aborts the run after
    /// `timeout_secs` of no progress. It exists because the tokio
    /// run watchdog (§7.2) is a task on the same runtime it
    /// watches — when the runtime wedges, its timers never fire
    /// either, and the r56 live test showed exactly that (a stall
    /// at 7–8% with no watchdog line in the log). This thread
    /// sleeps on the OS, so it keeps checking while the runtime is
    /// frozen. The stall action mirrors the tokio watchdog — pause
    /// flag, handle abort, retry cleared, error status — so a
    /// later Resume can restart the download. It never restarts
    /// the process (auto-restart is a pending user decision): it
    /// logs + aborts only. It logs FIRST (the store write below
    /// may itself block if the store is the wedge point — the log
    /// line is the one fact this thread exists to record, and it
    /// must not depend on the store). No progress event is
    /// emitted: `emit_snapshot` is async and needs the very
    /// runtime that is wedged — the DB status (which the main
    /// table polls) is the source of truth here. Suppressed while
    /// a segment waits out its retry backoff, exactly like the
    /// tokio watchdog. The thread exits when its registry entry is
    /// removed (run ended, cancelled, or the tokio watchdog fired)
    /// or replaced (a newer run of the same download started —
    /// the `ptr_eq` check, re-checked immediately before acting,
    /// so a stale thread can never abort a newer run).
    fn thread_watchdog(
        mgr: Arc<Self>,
        id: String,
        entry: Arc<ThreadWatchEntry>,
        timeout_secs: u64,
    ) {
        // Check often enough to fire near the deadline without
        // spinning: a sixth of the timeout, clamped to 1–10 s —
        // the same cadence as the tokio watchdog.
        let check_every = ((timeout_secs / 6).max(1)).min(10);
        loop {
            std::thread::sleep(Duration::from_secs(check_every));
            // Stale? A newer run of the same download replaced
            // this entry, or the run ended and removed it — the
            // thread exits either way.
            {
                let map = mgr.thread_watchdogs.lock().unwrap();
                match map.get(&id) {
                    Some(current) if Arc::ptr_eq(current, &entry) => {}
                    _ => return,
                }
            }
            if entry.retry.info().is_some() {
                continue; // retry backoff — not a wedge
            }
            if now_millis().saturating_sub(entry.last_progress_ms.load(Ordering::Relaxed))
                < timeout_secs * 1000
            {
                continue;
            }
            // Stalled — but re-check the entry first: a new run
            // of the same download may have started since the
            // check above, and aborting its handle or marking its
            // row error would be wrong.
            {
                let map = mgr.thread_watchdogs.lock().unwrap();
                match map.get(&id) {
                    Some(current) if Arc::ptr_eq(current, &entry) => {}
                    _ => return,
                }
            }
            mgr.store.log(&format!(
                "thread watchdog aborted {id} after {timeout_secs}s without progress (tokio watchdog did not fire)"
            ));
            {
                let mut map = mgr.thread_watchdogs.lock().unwrap();
                map.remove(&id);
            }
            entry.pause.store(true, Ordering::SeqCst);
            entry.handle.abort();
            entry.retry.clear_wait();
            let msg = format!(
                "No data arrived for {timeout_secs} seconds — the connection appears to have stalled"
            );
            if let Err(e) = mgr.store.set_status(&id, "error", Some(&msg)) {
                mgr.store.log(&format!(
                    "thread watchdog could not set error status for {id}: {e}"
                ));
            }
            mgr.cookies.lock().unwrap().remove(&id);
            return;
        }
    }

    async fn run_inner(
        self: &Arc<Self>,
        id: &str,
        pause: &Arc<AtomicBool>,
        retry: &Arc<RetrySlot>,
        per_download_limiter: &Arc<RateLimiter>,
        // Shared last-progress clock (§7.2, Round 56): stamped
        // whenever the downloaded total grows, read by the
        // independent watchdog task.
        last_progress_ms: &Arc<AtomicU64>,
    ) -> Result<()> {
        let row = self.store.get_download(id)?.ok_or_else(|| EngineError::NotFound(id.to_string()))?;
        let segments = self.store.get_segments(id)?;
        if segments.is_empty() {
            return Err(EngineError::InvalidState("This download has no segments".to_string()));
        }
        self.store.set_status(id, "downloading", None)?;

        let part = part_path(&row.save_path);
        if let Some(parent) = part.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new().create(true).write(true).open(&part)?;
        if let Some(size) = row.size_bytes {
            if size > 0 {
                file.set_len(size as u64)?;
            }
        }
        let file = Arc::new(StdMutex::new(file));

        let mut states: Vec<Arc<StdMutex<SegState>>> = segments
            .into_iter()
            .map(|r| {
                let end = r.end_byte.map(|e| e as u64);
                let done = r.downloaded_bytes as u64;
                Arc::new(StdMutex::new(SegState { row: r, done, end }))
            })
            .collect();
        // Site logins (S09): the saved credential for this host,
        // looked up once per run and carried by every segment — a
        // login-gated file downloads instead of failing with 401 on
        // its first segment. The credential lives only in this run's
        // tasks, never on the shared client. The active proxy is
        // looked up once per run too, so FTP segments route through
        // the FTP proxy and NTLM segments can fall back to the
        // handshake when the proxy answers 407.
        let site_auth = self.site_auth_for_url(&row.url);
        let proxy = self.active_proxy();
        // Browser cookies (S13 finish): the validated `Cookie`
        // header for this download, looked up once per run from
        // memory (the drain set it) and carried by every segment
        // like the site credential. A download with no entry runs
        // exactly as before.
        let cookies = self.cookies.lock().unwrap().get(id).cloned();
        // Download quota (S12): this run's gate, built from the
        // settings in force at start time. `None` when the quota
        // is off — segment code then skips the gate entirely.
        // Per-download cap (S12): the run's own bucket arrives as
        // a parameter, built in `start_download` from the row.
        let quota_gate: Option<Arc<QuotaGate>> = match self.quota_limit() {
            Ok(Some((limit_bytes, window_secs))) => Some(Arc::new(QuotaGate::new(
                Arc::clone(&self.store),
                limit_bytes,
                window_secs,
                Arc::clone(pause),
            ))),
            _ => None,
        };
        let mut tasks = JoinSet::new();
        for state in &states {
            let st = state.lock().unwrap();
            let done = st.done;
            let total = segment_len_of(&st);
            drop(st);
            if done < total {
                tasks.spawn(run_segment(
                    // Round 55: segments use the no-total-timeout
                    // client — a slow download must never be cut
                    // off by a total budget.
                    self.segment_client(),
                    Arc::clone(&self.limiter),
                    Arc::clone(per_download_limiter),
                    quota_gate.clone(),
                    self.max_retries.load(Ordering::Relaxed).max(1),
                    row.url.clone(),
                    site_auth.clone(),
                    proxy.clone(),
                    cookies.clone(),
                    Arc::clone(state),
                    Arc::clone(&file),
                    Arc::clone(pause),
                    Arc::clone(retry),
                ));
            }
        }

        // Round 55: checkpoint on the first progress tick, not
        // after a full interval — a download that finishes in ~1 s
        // used to report nothing at all. Pre-age the clock so the
        // first tick checkpoints at once.
        let mut last_checkpoint = Instant::now() - CHECKPOINT_INTERVAL;
        let mut last_emit = Instant::now();
        let mut prev_total = total_done(&states);
        let mut prev_instant = Instant::now();
        let mut speed_bps: u64 = 0;
        // Round 55 (run watchdog): when the run last made any
        // progress. A run that makes no progress at all for
        // RUN_STALL_TIMEOUT is wedged — see the tick branch below.
        let mut last_byte_at = Instant::now();

        loop {
            if pause.load(Ordering::SeqCst) {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                self.checkpoint(id, &states, speed_bps).await;
                self.store.set_status(id, "paused", None)?;
                // Quota pause (S12): only a pause the gate caused
                // marks the row — a user's own pause must never be
                // auto-resumed on rollover.
                if quota_gate.as_ref().is_some_and(|g| g.hit()) {
                    let _ = self.store.set_quota_paused(id, true);
                }
                // A segment aborted mid-backoff leaves a stale wait
                // behind: clear it before the snapshot, or the dialog
                // would show a countdown for a paused download.
                retry.clear_wait();
                self.emit_snapshot(id, "paused").await;
                return Ok(());
            }
            if tasks.is_empty() {
                break;
            }
            tokio::select! {
                finished = tasks.join_next() => {
                    match finished {
                        Some(Ok(Err(e))) => {
                            if pause.load(Ordering::SeqCst) {
                                // A segment stopped because we paused:
                                // that is not a failure.
                                tasks.abort_all();
                                while tasks.join_next().await.is_some() {}
                                self.checkpoint(id, &states, 0).await;
                                self.store.set_status(id, "paused", None)?;
                                if quota_gate.as_ref().is_some_and(|g| g.hit()) {
                                    let _ = self.store.set_quota_paused(id, true);
                                }
                                retry.clear_wait();
                                self.emit_snapshot(id, "paused").await;
                                return Ok(());
                            }
                            // A segment exhausted its retries: fail the download.
                            tasks.abort_all();
                            while tasks.join_next().await.is_some() {}
                            self.checkpoint(id, &states, 0).await;
                            retry.clear_wait();
                            return Err(e);
                        }
                        Some(Ok(Ok(()))) | Some(Err(_)) => {
                            // One segment finished (or was aborted): try a
                            // dynamic split while connections are idle.
                            self.try_split(id, &mut states, &mut tasks, pause, retry, &row, &file, &site_auth, &proxy, &cookies, per_download_limiter, &quota_gate).await;
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep(PROGRESS_INTERVAL) => {
                    let now = Instant::now();
                    let total = total_done(&states);
                    // `last_byte_at` is the local clock for the
                    // in-loop watchdog below; `last_progress_ms` is
                    // the SHARED clock the independent watchdog
                    // reads (§7.2, Round 56) — both are stamped
                    // together whenever the total grows, so the two
                    // watchdogs can never disagree about when
                    // progress last happened.
                    if total > prev_total {
                        last_byte_at = now;
                        last_progress_ms.store(now_millis(), Ordering::Relaxed);
                    }
                    let dt = now.duration_since(prev_instant).as_secs_f64();
                    if dt > 0.0 {
                        speed_bps = ((total.saturating_sub(prev_total)) as f64 / dt) as u64;
                    }
                    prev_total = total;
                    prev_instant = now;
                    if last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
                        self.checkpoint(id, &states, speed_bps).await;
                        last_checkpoint = Instant::now();
                    }
                    if last_emit.elapsed() >= PROGRESS_INTERVAL {
                        self.emit(id, &row, &states, speed_bps, "downloading", Some(retry)).await;
                        last_emit = Instant::now();
                    }
                    // Run watchdog (Round 55, defects 1 + 4): a run
                    // that makes no progress at all for
                    // RUN_STALL_TIMEOUT is wedged — a hung segment
                    // task, or a stalled FTP / Kerberos / NTLM read
                    // the per-chunk watchdog cannot see. Fail it
                    // with a clear message instead of leaving
                    // status `downloading` forever (which also made
                    // Resume refuse it as "already running" and
                    // Pause a no-op). Suppressed while a segment
                    // waits out its retry backoff: a waiting segment
                    // is not a wedged run.
                    if !tasks.is_empty()
                        && retry.info().is_none()
                        && last_byte_at.elapsed() >= RUN_STALL_TIMEOUT
                    {
                        tasks.abort_all();
                        while tasks.join_next().await.is_some() {}
                        self.checkpoint(id, &states, 0).await;
                        retry.clear_wait();
                        // Round 57 (§9.4): log the firing
                        // explicitly — the r56 live test had a
                        // stalled run with no watchdog line at all,
                        // and the log could not say whether the
                        // in-loop watchdog, the tokio watchdog, or
                        // neither had run.
                        self.store.log(&format!(
                            "in-loop watchdog fired for {id} after {}s without progress",
                            RUN_STALL_TIMEOUT.as_secs()
                        ));
                        return Err(EngineError::InvalidState(format!(
                            "No data arrived for {} seconds — the connection appears to have stalled",
                            RUN_STALL_TIMEOUT.as_secs()
                        )));
                    }
                }
            }
        }

        // All segments done: verify, then rename atomically.
        let total = total_done(&states);
        if let Some(size) = row.size_bytes {
            if total != size as u64 {
                return Err(EngineError::InvalidState(format!(
                    "Download finished with {total} of {size} bytes"
                )));
            }
        }
        self.checkpoint(id, &states, 0).await;
        drop(file);
        std::fs::rename(&part, &row.save_path)?;
        self.store.mark_complete(id)?;
        // Antivirus hook (S14): the scan runs AFTER the rename,
        // so the scanner always sees the finished file, never the
        // `.part`. No scanner configured (or scanning turned off)
        // is recorded at once, so the completion dialog can say
        // "no scanner configured" instead of showing nothing. A
        // configured scanner runs on a detached task — a slow
        // scanner never blocks this completion path, the queue
        // drain, or the UI — and its result is stored on the row
        // and re-emitted as a completion event when it lands.
        match self.store.get_settings() {
            Ok(settings) if settings.antivirus_enabled => {
                match settings.antivirus_command.as_deref().filter(|c| !c.trim().is_empty()) {
                    Some(_) => {
                        let _ = self
                            .store
                            .set_scan_result(id, &ScanResult::scanning());
                        let mgr = Arc::clone(self);
                        let scan_id = id.to_string();
                        tokio::spawn(async move {
                            let _ = mgr.run_antivirus_scan(&scan_id).await;
                        });
                    }
                    None => {
                        let _ = self.store.set_scan_result(
                            id,
                            &ScanResult::not_configured(
                                "No antivirus scanner is configured in Settings.",
                            ),
                        );
                    }
                }
            }
            Ok(_) => {
                let _ = self.store.set_scan_result(
                    id,
                    &ScanResult::not_configured(
                        "Antivirus scanning is turned off in Settings.",
                    ),
                );
            }
            Err(_) => {}
        }
        self.emit(id, &row, &states, 0, "complete", Some(retry)).await;
        Ok(())
    }

    /// Halve the largest unfinished segment when a connection is idle.
    async fn try_split(
        &self,
        _download_id: &str,
        states: &mut Vec<Arc<StdMutex<SegState>>>,
        tasks: &mut JoinSet<std::result::Result<(), EngineError>>,
        pause: &Arc<AtomicBool>,
        retry: &Arc<RetrySlot>,
        row: &DownloadRow,
        file: &Arc<StdMutex<std::fs::File>>,
        site_auth: &Option<(String, String)>,
        proxy: &Option<ProxyConfig>,
        cookies: &Option<String>,
        per_download_limiter: &Arc<RateLimiter>,
        quota_gate: &Option<Arc<QuotaGate>>,
    ) {
        if tasks.len() >= self.max_connections() {
            return;
        }
        // Largest remaining segment. Round 55: an open-ended
        // segment (unknown size, `end` is `None`) is never split —
        // its length reads as `u64::MAX`, so the midpoint lands
        // near 2^63 and the split was only ever refused by the
        // schema CHECKs (a latent corruption risk: a mid that
        // wraps into a valid-looking range would have created a
        // bogus segment). The DB CHECK stays as the safety net;
        // the guard here is the application-level one.
        let mut best: Option<(usize, u64)> = None;
        for (i, s) in states.iter().enumerate() {
            let st = s.lock().unwrap();
            if st.end.is_none() {
                continue;
            }
            let remaining = segment_len_of(&st).saturating_sub(st.done);
            if remaining > MIN_SPLIT_REMAINING && best.map(|(_, r)| remaining > r).unwrap_or(true) {
                best = Some((i, remaining));
            }
        }
        let Some((idx, remaining)) = best else { return };
        let state = Arc::clone(&states[idx]);
        let (mid, new_row) = {
            let mut st = state.lock().unwrap();
            let start = st.row.start_byte as u64;
            // The split point must lie strictly inside the remaining
            // range: after the bytes already downloaded, at or before
            // the segment's end. Anything else is a bug, not a split
            // — refuse it here instead of letting the store fail it.
            let Some(end) = st.end else { return };
            let mid = start + st.done + remaining / 2;
            if mid <= start + st.done || mid > end {
                return;
            }
            // Shorten the running segment first (DB trigger forbids overlap).
            match self.store.split_segment(&st.row, mid as i64) {
                Ok(new_row) => {
                    st.end = Some(mid - 1);
                    st.row.end_byte = Some((mid - 1) as i64);
                    (mid, new_row)
                }
                Err(_) => return, // another split raced us; skip this tick
            }
        };
        let new_end = new_row_end(&new_row);
        let new_state = Arc::new(StdMutex::new(SegState {
            row: new_row,
            done: 0,
            end: new_end,
        }));
        tasks.spawn(run_segment(
            // Round 55: segments use the no-total-timeout client.
            self.segment_client(),
            Arc::clone(&self.limiter),
            Arc::clone(per_download_limiter),
            quota_gate.clone(),
            self.max_retries.load(Ordering::Relaxed).max(1),
            row.url.clone(),
            site_auth.clone(),
            proxy.clone(),
            cookies.clone(),
            Arc::clone(&new_state),
            Arc::clone(file),
            Arc::clone(pause),
            Arc::clone(retry),
        ));
        // The supervisor must keep the new state: progress, checkpoint
        // and completion all read from `states`.
        states.push(new_state);
        let _ = mid;
    }

    async fn checkpoint(&self, id: &str, states: &[Arc<StdMutex<SegState>>], speed_bps: u64) {
        // Snapshot the in-memory state first (fast, no IO): the
        // totals and per-segment progress the store write needs.
        let total: i64 = states.iter().map(|s| s.lock().unwrap().done as i64).sum();
        let segs: Vec<(String, i64, &'static str)> = states
            .iter()
            .map(|s| {
                let st = s.lock().unwrap();
                let status = if st.done >= segment_len_of(&st) {
                    "done"
                } else {
                    "active"
                };
                (st.row.id.clone(), st.done as i64, status)
            })
            .collect();
        // Round 57 (§9.2b): the store calls run on a blocking
        // thread, NOT on a tokio worker. `Store` is a synchronous
        // SQLite wrapper around a `std::sync::Mutex<Connection>`
        // — every checkpoint used to run inline on the worker
        // that drives the supervisor loop, and a store call that
        // blocks (a contended or wedged connection) occupied
        // that worker for its whole duration. The await below
        // only parks the supervisor task (the worker is free
        // while the blocking thread writes); a checkpoint failure
        // stays diagnostic, not fatal, exactly as before.
        let store = Arc::clone(&self.store);
        let id = id.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let size = store
                .get_download(&id)
                .ok()
                .flatten()
                .and_then(|r| r.size_bytes);
            let eta = if speed_bps > 0 {
                size.map(|s| (s - total).max(0) / speed_bps as i64)
            } else {
                None
            };
            // Round 56 (§7.1d): a failed checkpoint used to be
            // swallowed with `let _ =` — the download's progress
            // was silently not persisted, and the live test could
            // only see the frozen UI, never the cause. The error
            // is now logged (throttled: a checkpoint runs every
            // second, so a persistently failing store logs at most
            // once per second — the checkpoint itself is the
            // throttle) while the run continues: a checkpoint
            // failure is diagnostic, not fatal.
            if let Err(e) = store.checkpoint(&id, total, speed_bps as i64, eta) {
                store.log(&format!("checkpoint failed for {id}: {e}"));
            }
            for (seg_id, done, status) in segs {
                if let Err(e) = store.update_segment(&seg_id, done, status) {
                    store.log(&format!(
                        "checkpoint failed for segment {seg_id}: {e}"
                    ));
                }
            }
        })
        .await;
    }

    async fn emit(
        &self,
        id: &str,
        row: &DownloadRow,
        states: &[Arc<StdMutex<SegState>>],
        speed_bps: u64,
        status: &str,
        retry: Option<&RetrySlot>,
    ) {
        let total: u64 = states.iter().map(|s| s.lock().unwrap().done).sum();
        let eta = match (row.size_bytes, speed_bps) {
            (Some(size), s) if s > 0 => Some(((size as u64).saturating_sub(total)) / s),
            _ => None,
        };
        let segments = states
            .iter()
            .map(|s| {
                let st = s.lock().unwrap();
                SegmentProgress {
                    segment_index: st.row.segment_index,
                    start_byte: st.row.start_byte,
                    end_byte: st.row.end_byte,
                    downloaded_bytes: st.done as i64,
                    status: if st.done >= segment_len_of(&st) { "done".into() } else { "active".into() },
                }
            })
            .collect();
        let event = ProgressEvent {
            id: id.to_string(),
            filename: row.filename.clone(),
            downloaded_bytes: total,
            size_bytes: row.size_bytes.map(|s| s as u64),
            speed_bps,
            eta_seconds: eta,
            status: status.to_string(),
            segments,
            // The live retry wait, if any — the dialog's countdown
            // reads this, so it is the engine's own deadline.
            retry: retry.and_then(|r| r.info()),
            // The scan result, once known: the completion event
            // carries `not_configured` / `scanning` (stored just
            // before this emit), and the detached scan's later
            // event carries the finished result. Mid-download
            // events carry none — no scan has happened yet.
            scan: if status == "complete" {
                self.store.get_scan_result(id).ok().flatten()
            } else {
                None
            },
        };
        // S03 progress window: keep the latest event per download
        // (see the `last_progress` field). The window polls it —
        // the frontend listener never fires (BUG-044).
        self.last_progress
            .lock()
            .unwrap()
            .insert(id.to_string(), event.clone());
        let _ = self.events.send(event);
    }

    /// The latest progress event for a download, or `None` when
    /// it has never emitted one (queued, or added before this
    /// session started). The progress window polls this every
    /// 500 ms for its live speed, segments, retry countdown and
    /// scan result — the same event the `download-progress`
    /// broadcast carries to the (deaf) frontend listener.
    pub fn last_progress(&self, id: &str) -> Option<ProgressEvent> {
        self.last_progress.lock().unwrap().get(id).cloned()
    }

    /// A snapshot event for a terminal state (paused / error): no
    /// segment is waiting then, so `retry` is always `None`.
    async fn emit_snapshot(&self, id: &str, status: &str) {
        if let Ok(Some(row)) = self.store.get_download(id) {
            let segments = self.store.get_segments(id).unwrap_or_default();
            let states: Vec<Arc<StdMutex<SegState>>> = segments
                .into_iter()
                .map(|r| {
                    let end = r.end_byte.map(|e| e as u64);
                    let done = r.downloaded_bytes as u64;
                    Arc::new(StdMutex::new(SegState { row: r, done, end }))
                })
                .collect();
            self.emit(id, &row, &states, 0, status, None).await;
        }
    }
}

// -- segment task ----------------------------------------------------------

/// Cross-platform positional write: write `buf` at absolute `offset`
/// in `file` and return the bytes written — the same semantics on
/// both platforms (a partial write is possible, like `File::write`).
/// Unix uses `FileExt::write_at`; Windows uses
/// `FileExt::seek_write` (the Unix-only import broke the first
/// Windows build, BUG-027). Callers keep their existing logic
/// unchanged; only this primitive is platform-gated.
#[cfg(unix)]
fn write_at_pos(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.write_at(buf, offset)
}

/// Cross-platform positional write: write `buf` at absolute `offset`
/// in `file` and return the bytes written — the same semantics on
/// both platforms (a partial write is possible, like `File::write`).
/// Windows uses `FileExt::seek_write`; Unix uses `FileExt::write_at`
/// (the Unix-only import broke the first Windows build, BUG-027).
/// Callers keep their existing logic unchanged; only this primitive
/// is platform-gated.
#[cfg(windows)]
fn write_at_pos(file: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_write(buf, offset)
}

/// Download one segment, retrying with exponential backoff.
/// Stops early when paused, when its (possibly split) end is reached,
/// or when the segment is complete. `max_attempts` comes from
/// settings.max_retries (at least 1: a single attempt, no retry).
/// While a failed attempt waits out its backoff, the wait is
/// published on the run's retry slot (S03) so the progress dialog
/// can show a countdown; "Retry now" cuts the wait short by setting
/// the slot's flag, and pausing ends it. The backoff itself is the
/// pre-existing rule — `1 << attempt` seconds, capped at 16 s —
/// unchanged: this only makes it visible and interruptible.
async fn run_segment(
    client: Arc<Client>,
    limiter: Arc<RateLimiter>,
    per_download: Arc<RateLimiter>,
    quota: Option<Arc<QuotaGate>>,
    max_attempts: u32,
    url: String,
    site_auth: Option<(String, String)>,
    proxy: Option<ProxyConfig>,
    cookies: Option<String>,
    state: Arc<StdMutex<SegState>>,
    file: Arc<StdMutex<std::fs::File>>,
    pause: Arc<AtomicBool>,
    retry: Arc<RetrySlot>,
) -> std::result::Result<(), EngineError> {
    let mut attempt: u32 = 0;
    loop {
        match download_segment_once(
            &client,
            &limiter,
            &per_download,
            &quota,
            &url,
            &site_auth,
            &proxy,
            &cookies,
            &state,
            &file,
            &pause,
        )
        .await
        {
            Ok(()) => return Ok(()),
            // Pausing is a clean stop, not a failure: report success so
            // the supervisor's pause path (not the error path) runs.
            Err(EngineError::InvalidState(msg)) if msg == "paused" => return Ok(()),
            Err(e) => {
                attempt += 1;
                if attempt >= max_attempts {
                    return Err(e);
                }
                let backoff = Duration::from_secs(1 << attempt.min(4));
                retry.set_wait(attempt, max_attempts, &e.to_string(), backoff);
                // Sleep in short slices instead of one long sleep, so
                // a pause or a "Retry now" is noticed within 100 ms
                // instead of only when the whole backoff expires.
                let deadline = Instant::now() + backoff;
                loop {
                    if pause.load(Ordering::SeqCst)
                        || retry.retry_now.swap(false, Ordering::SeqCst)
                    {
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                retry.clear_wait();
                if pause.load(Ordering::SeqCst) {
                    return Ok(());
                }
            }
        }
    }
}

/// Round 55 (defect 1): the error a stalled request fails with.
/// The segment retries with backoff, and after the attempts are
/// exhausted the download goes to `error` with this message —
/// instead of hanging forever at 0% with status `downloading`.
fn stall_error() -> EngineError {
    EngineError::InvalidState(format!(
        "No data arrived for {} seconds — the connection appears to have stalled",
        SEGMENT_STALL_TIMEOUT.as_secs()
    ))
}

async fn download_segment_once(
    client: &Client,
    limiter: &RateLimiter,
    per_download: &RateLimiter,
    quota: &Option<Arc<QuotaGate>>,
    url: &str,
    site_auth: &Option<(String, String)>,
    proxy: &Option<ProxyConfig>,
    cookies: &Option<String>,
    state: &Arc<StdMutex<SegState>>,
    file: &Arc<StdMutex<std::fs::File>>,
    pause: &Arc<AtomicBool>,
) -> std::result::Result<(), EngineError> {
    let (start, end) = {
        let st = state.lock().unwrap();
        (st.row.start_byte as u64 + st.done, st.end)
    };
    // FTP: the FTP transport, not reqwest — RETR over PASV, with
    // REST resume, through the FTP proxy when one is configured.
    if url.starts_with("ftp://") {
        return download_segment_ftp(limiter, per_download, quota, url, proxy, start, end, state, file, pause).await;
    }
    let range = match end {
        Some(e) => format!("bytes={start}-{e}"),
        None => format!("bytes={start}-"),
    };
    // Site logins (S09): the saved credential for this host rides
    // on the segment request, like on the probe — per request, so
    // the shared client never carries one site's password to
    // another site.
    //
    // The request is built by a closure so the proxy-auth fallback
    // can re-issue it: `proxy_basic` adds the proxy's Basic
    // credentials as a `Proxy-Authorization` header (reqwest has no
    // per-request proxy-auth API, and the client only sends Basic
    // preemptively when the auth setting is `basic` — see
    // `proxy.rs`).
    let build_req = |proxy_basic: Option<(&str, &str)>| {
        let mut req = client.get(url).header(RANGE, range.clone());
        if let Some((user, pass)) = site_auth {
            req = req.basic_auth(user, Some(pass));
        }
        // Browser cookies (S13 finish): the validated `Cookie`
        // header for this download rides on the segment request
        // too, for this host only — a login-only file downloads
        // instead of failing with 401 on its first segment. FTP
        // segments return before this point (no `Cookie` header
        // exists in FTP), and the raw Kerberos/NTLM retries below
        // do not carry them (a cookie-gated file behind such a
        // proxy fails with the origin's 401, clearly).
        if let Some(header) = cookies {
            if !header.is_empty() {
                req = req.header(reqwest::header::COOKIE, header);
            }
        }
        if let Some((user, pass)) = proxy_basic {
            req = req.header(
                "proxy-authorization",
                basic_proxy_auth_header(user, pass),
            );
        }
        req
    };
    // Round 55: the headers must arrive within the stall
    // timeout — a connection that stalls after connect (TLS
    // handshake, a server that never responds) used to block the
    // segment — and the probe, through the same client — for
    // minutes with no error at all.
    let mut resp = match tokio::time::timeout(SEGMENT_STALL_TIMEOUT, build_req(None).send()).await
    {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(stall_error()),
    };
    let mut status = resp.status();
    if status.as_u16() == 407 {
        // The proxy refused the request: walk the proxy-auth
        // fallback order (Kerberos, then NTLM, then Basic — see
        // `proxy::proxy_auth_order`). A failed scheme falls through
        // to the next one; the raw Kerberos/NTLM attempts pump the
        // body themselves and return `Ok(())` on success. The
        // Basic attempt re-issues the reqwest request and falls
        // through to the normal handling below. `auth: basic`
        // already sent its credentials preemptively, so a 407 there
        // means they were refused — no walker, the status error
        // below reports it.
        if let Some(cfg) = proxy.as_ref().filter(|c| c.proxy_type == "http") {
            if resolve_proxy_auth(&cfg.auth) != PROXY_AUTH_BASIC {
                let offered = offered_proxy_auth_schemes(
                    resp.headers()
                        .get_all("proxy-authenticate")
                        .iter()
                        .filter_map(|v| v.to_str().ok()),
                );
                let order = proxy_auth_order(&cfg.auth, &offered, cfg.has_basic_creds());
                if order.is_empty() {
                    return Err(crate::proxy::proxy_auth_not_offered_error(&cfg.auth, &offered));
                }
                let mut last_err: Option<EngineError> = None;
                let mut retried_with_basic = false;
                for scheme in order {
                    match scheme {
                        ProxyAuthScheme::Kerberos => {
                            if !url.starts_with("http://") {
                                return Err(EngineError::InvalidState(
                                    "Kerberos (Negotiate) proxy authentication is only supported for http:// URLs — \
                                     https:// through a Kerberos proxy needs a TLS tunnel after the \
                                     handshake, which is not built"
                                        .to_string(),
                                ));
                            }
                            let initial_input = crate::kerberos::negotiate_response_token(
                                resp.headers()
                                    .get_all("proxy-authenticate")
                                    .iter()
                                    .filter_map(|v| v.to_str().ok()),
                            );
                            match download_segment_kerberos(
                                limiter,
                                per_download,
                                quota,
                                url,
                                cfg,
                                start,
                                end,
                                state,
                                file,
                                pause,
                                initial_input,
                            )
                            .await
                            {
                                Ok(()) => return Ok(()),
                                Err(e) => {
                                    last_err = Some(e);
                                    continue;
                                }
                            }
                        }
                        ProxyAuthScheme::Ntlm => {
                            if !url.starts_with("http://") {
                                return Err(EngineError::InvalidState(
                                    "NTLM proxy authentication is only supported for http:// URLs — \
                                     https:// through an NTLM proxy needs a TLS tunnel after the \
                                     handshake, which is not built"
                                        .to_string(),
                                ));
                            }
                            match download_segment_ntlm(limiter, per_download, quota, url, cfg, start, end, state, file, pause).await
                            {
                                Ok(()) => return Ok(()),
                                Err(e) => {
                                    last_err = Some(e);
                                    continue;
                                }
                            }
                        }
                        ProxyAuthScheme::Basic => {
                            let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) else {
                                return Err(EngineError::InvalidState(
                                    "The proxy needs Basic authentication — set the proxy username and password"
                                        .to_string(),
                                ));
                            };
                            resp = match tokio::time::timeout(
                                SEGMENT_STALL_TIMEOUT,
                                build_req(Some((user, pass))).send(),
                            )
                            .await
                            {
                                Ok(Ok(resp)) => resp,
                                Ok(Err(e)) => return Err(e.into()),
                                Err(_) => return Err(stall_error()),
                            };
                            status = resp.status();
                            retried_with_basic = true;
                            break;
                        }
                    }
                }
                if !retried_with_basic {
                    return Err(last_err.unwrap_or_else(|| {
                        EngineError::Status(status.as_u16(), format!("{status}"))
                    }));
                }
            }
        }
    }
    if status.as_u16() == 416 {
        // Range not satisfiable: the segment is already complete.
        return Ok(());
    }
    if status.is_success() && status.as_u16() != 206 && start > 0 {
        // We asked for a later range and the server sent the whole file
        // from the start instead: it does not really support ranges.
        // Fail loudly instead of writing the wrong bytes at this offset.
        return Err(EngineError::Status(
            status.as_u16(),
            "Server ignored the range request".to_string(),
        ));
    }
    if !status.is_success() && status.as_u16() != 206 {
        return Err(EngineError::Status(status.as_u16(), format!("{status}")));
    }
    let mut stream = resp.bytes_stream();
    // Round 55: each chunk must arrive within the stall timeout —
    // a body stream that goes quiet forever used to hang the whole
    // run at 0% with no error. A timeout fails this attempt; the
    // retry backoff (run_segment) tries again, and after the
    // attempts are exhausted the download goes to `error`.
    loop {
        let chunk = match tokio::time::timeout(SEGMENT_STALL_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return Err(stall_error()),
        };
        if pause.load(Ordering::SeqCst) {
            return Err(EngineError::InvalidState("paused".to_string()));
        }
        let chunk = chunk?;
        let (offset, limit) = {
            let st = state.lock().unwrap();
            let offset = st.row.start_byte as u64 + st.done;
            let limit = st.end.map(|e| e + 1 - offset).unwrap_or(u64::MAX);
            (offset, limit)
        };
        let take = (chunk.len() as u64).min(limit) as usize;
        if take == 0 {
            return Ok(());
        }
        // Global speed cap (S12): wait for the tokens for these
        // bytes before writing them. Unlimited returns at once.
        limiter.acquire(take as u64).await;
        // Per-download cap (S12): a second token bucket alongside
        // the global one — the effective rate is the min of the two.
        per_download.acquire(take as u64).await;
        {
            let f = file.lock().unwrap();
            write_at_pos(&f, &chunk[..take], offset)?;
        }
        {
            let mut st = state.lock().unwrap();
            st.done += take as u64;
        }
        // Download quota (S12): count the bytes just written.
        // When the quota is reached the gate sets the pause flag,
        // and the next chunk boundary stops this segment through
        // the existing pause path.
        if let Some(gate) = quota {
            gate.record(take as u64, now_unix());
        }
        if (take as u64) < chunk.len() as u64 {
            return Ok(()); // reached the (split) end of this segment
        }
    }
    Ok(())
}

/// One FTP segment: RETR over PASV with REST resume, pumped into
/// the segment's file at the right offsets under the speed limiter.
/// The chunk loop mirrors `download_segment_once` (same offsets,
/// same limiter, same pause rule, same split-end stop) — the two
/// cannot share a function because reqwest's byte stream and the
/// FTP reader are different types.
async fn download_segment_ftp(
    limiter: &RateLimiter,
    per_download: &RateLimiter,
    quota: &Option<Arc<QuotaGate>>,
    url: &str,
    proxy: &Option<ProxyConfig>,
    start: u64,
    end: Option<u64>,
    state: &Arc<StdMutex<SegState>>,
    file: &Arc<StdMutex<std::fs::File>>,
    pause: &Arc<AtomicBool>,
) -> std::result::Result<(), EngineError> {
    let mut reader = crate::ftp::ftp_retr(url, proxy.as_ref(), start, end).await?;
    let mut buf = vec![0u8; 65536];
    loop {
        if pause.load(Ordering::SeqCst) {
            return Err(EngineError::InvalidState("paused".to_string()));
        }
        let n = reader.read_chunk(&mut buf).await?;
        if n == 0 {
            break;
        }
        let (offset, limit) = {
            let st = state.lock().unwrap();
            let offset = st.row.start_byte as u64 + st.done;
            let limit = st.end.map(|e| e + 1 - offset).unwrap_or(u64::MAX);
            (offset, limit)
        };
        let take = (n as u64).min(limit) as usize;
        if take == 0 {
            break;
        }
        limiter.acquire(take as u64).await;
        // Per-download cap (S12): a second token bucket alongside
        // the global one — the effective rate is the min of the two.
        per_download.acquire(take as u64).await;
        {
            let f = file.lock().unwrap();
            write_at_pos(&f, &buf[..take], offset)?;
        }
        {
            let mut st = state.lock().unwrap();
            st.done += take as u64;
        }
        // Download quota (S12): count the bytes just written.
        // When the quota is reached the gate sets the pause flag,
        // and the next chunk boundary stops this segment through
        // the existing pause path.
        if let Some(gate) = quota {
            gate.record(take as u64, now_unix());
        }
        if (take as u64) < n as u64 {
            break; // reached the (split) end of this segment
        }
    }
    reader.finish().await?;
    Ok(())
}

/// One HTTP segment through a Kerberos proxy: the SPNEGO
/// handshake and the ranged GET share one connection
/// (`kerberos::kerberos_request_with_initiator`). Same chunk loop
/// as the NTLM path above — the two cannot share a function
/// because the handshake that opens the connection differs.
/// `initial_input` is the response token from the 407 that
/// triggered this segment's fallback, when it carried one.
async fn download_segment_kerberos(
    limiter: &RateLimiter,
    per_download: &RateLimiter,
    quota: &Option<Arc<QuotaGate>>,
    url: &str,
    proxy: &ProxyConfig,
    start: u64,
    end: Option<u64>,
    state: &Arc<StdMutex<SegState>>,
    file: &Arc<StdMutex<std::fs::File>>,
    pause: &Arc<AtomicBool>,
    initial_input: Option<Vec<u8>>,
) -> std::result::Result<(), EngineError> {
    let initiator = crate::kerberos::new_kerberos_initiator(&proxy.host)?;
    let (status, _headers, mut reader) = crate::kerberos::kerberos_request_with_initiator(
        proxy,
        "GET",
        url,
        Some((start, end)),
        initiator,
        initial_input,
    )
    .await?;
    if status == 416 {
        // Range not satisfiable: the segment is already complete.
        return Ok(());
    }
    if status == 200 && start > 0 {
        return Err(EngineError::Status(
            status,
            "Server ignored the range request".to_string(),
        ));
    }
    if status == 407 {
        return Err(EngineError::InvalidState(
            "The proxy refused the Kerberos (Negotiate) authentication (407) — \
             check that the proxy trusts this account and that the Kerberos ticket is valid"
                .to_string(),
        ));
    }
    if status != 200 && status != 206 {
        return Err(EngineError::Status(status, format!("HTTP status {status}")));
    }
    let mut buf = vec![0u8; 65536];
    loop {
        if pause.load(Ordering::SeqCst) {
            return Err(EngineError::InvalidState("paused".to_string()));
        }
        let n = reader.read_chunk(&mut buf).await?;
        if n == 0 {
            break;
        }
        let (offset, limit) = {
            let st = state.lock().unwrap();
            let offset = st.row.start_byte as u64 + st.done;
            let limit = st.end.map(|e| e + 1 - offset).unwrap_or(u64::MAX);
            (offset, limit)
        };
        let take = (n as u64).min(limit) as usize;
        if take == 0 {
            break;
        }
        limiter.acquire(take as u64).await;
        // Per-download cap (S12): a second token bucket alongside
        // the global one — the effective rate is the min of the two.
        per_download.acquire(take as u64).await;
        {
            let f = file.lock().unwrap();
            write_at_pos(&f, &buf[..take], offset)?;
        }
        {
            let mut st = state.lock().unwrap();
            st.done += take as u64;
        }
        // Download quota (S12): count the bytes just written.
        // When the quota is reached the gate sets the pause flag,
        // and the next chunk boundary stops this segment through
        // the existing pause path.
        if let Some(gate) = quota {
            gate.record(take as u64, now_unix());
        }
        if (take as u64) < n as u64 {
            break; // reached the (split) end of this segment
        }
    }
    Ok(())
}

/// One HTTP segment through an NTLM proxy: the Type 1/2/3 handshake
/// and the ranged GET share one connection (`ntlm::ntlm_request`).
/// Same chunk loop as the FTP path above.
async fn download_segment_ntlm(
    limiter: &RateLimiter,
    per_download: &RateLimiter,
    quota: &Option<Arc<QuotaGate>>,
    url: &str,
    proxy: &ProxyConfig,
    start: u64,
    end: Option<u64>,
    state: &Arc<StdMutex<SegState>>,
    file: &Arc<StdMutex<std::fs::File>>,
    pause: &Arc<AtomicBool>,
) -> std::result::Result<(), EngineError> {
    let (status, _headers, mut reader) =
        crate::ntlm::ntlm_request(proxy, "GET", url, Some((start, end))).await?;
    if status == 416 {
        // Range not satisfiable: the segment is already complete.
        return Ok(());
    }
    if status == 200 && start > 0 {
        return Err(EngineError::Status(
            status,
            "Server ignored the range request".to_string(),
        ));
    }
    if status != 200 && status != 206 {
        return Err(EngineError::Status(status, format!("HTTP status {status}")));
    }
    let mut buf = vec![0u8; 65536];
    loop {
        if pause.load(Ordering::SeqCst) {
            return Err(EngineError::InvalidState("paused".to_string()));
        }
        let n = reader.read_chunk(&mut buf).await?;
        if n == 0 {
            break;
        }
        let (offset, limit) = {
            let st = state.lock().unwrap();
            let offset = st.row.start_byte as u64 + st.done;
            let limit = st.end.map(|e| e + 1 - offset).unwrap_or(u64::MAX);
            (offset, limit)
        };
        let take = (n as u64).min(limit) as usize;
        if take == 0 {
            break;
        }
        limiter.acquire(take as u64).await;
        // Per-download cap (S12): a second token bucket alongside
        // the global one — the effective rate is the min of the two.
        per_download.acquire(take as u64).await;
        {
            let f = file.lock().unwrap();
            write_at_pos(&f, &buf[..take], offset)?;
        }
        {
            let mut st = state.lock().unwrap();
            st.done += take as u64;
        }
        // Download quota (S12): count the bytes just written.
        // When the quota is reached the gate sets the pause flag,
        // and the next chunk boundary stops this segment through
        // the existing pause path.
        if let Some(gate) = quota {
            gate.record(take as u64, now_unix());
        }
        if (take as u64) < n as u64 {
            break; // reached the (split) end of this segment
        }
    }
    Ok(())
}

// -- helpers ---------------------------------------------------------------

/// Length of a segment, from its already-locked state.
/// NOTE: this must take the guard, not the Arc — locking the same
/// std Mutex twice on one thread deadlocks (that bug hung the run loop).
fn segment_len_of(st: &SegState) -> u64 {
    match st.end {
        Some(e) => (e as u64 + 1).saturating_sub(st.row.start_byte as u64),
        None => u64::MAX,
    }
}

fn new_row_end(row: &SegmentRow) -> Option<u64> {
    row.end_byte.map(|e| e as u64)
}

fn total_done(states: &[Arc<StdMutex<SegState>>]) -> u64 {
    states.iter().map(|s| s.lock().unwrap().done).sum()
}

fn part_path(save_path: &str) -> PathBuf {
    PathBuf::from(format!("{save_path}.part"))
}

/// Compare a HEAD probe with what sync last stored about a file.
/// Size, ETag and Last-Modified are compared only when both sides
/// have a value: a server that never sends an ETag must not make
/// every sync re-download the file. Returns `Some(changed)`, or
/// `None` when nothing at all can be compared (no size on either
/// side and no validator on either side) — the caller reports that
/// honestly instead of guessing.
fn remote_changed(info: &ProbeInfo, row: &DownloadRow) -> Option<bool> {
    let mut compared = false;
    if let (Some(new), Some(old)) = (info.size.map(|s| s as i64), row.size_bytes) {
        compared = true;
        if new != old {
            return Some(true);
        }
    }
    if let (Some(new), Some(old)) = (info.etag.as_deref(), row.etag.as_deref()) {
        compared = true;
        if new != old {
            return Some(true);
        }
    }
    if let (Some(new), Some(old)) = (info.last_modified.as_deref(), row.last_modified.as_deref())
    {
        compared = true;
        if new != old {
            return Some(true);
        }
    }
    if compared {
        Some(false)
    } else {
        None
    }
}

/// Initial segment ranges for a probe: 4 segments when the file is large
/// and range-capable, otherwise a single segment.
fn initial_ranges(info: &ProbeInfo, max_connections: usize) -> Vec<(i64, Option<i64>)> {
    match (info.size, info.accept_ranges) {
        (Some(size), true) if size >= MIN_SIZE_TO_SEGMENT => {
            let n = INITIAL_SEGMENTS.min(max_connections).max(1) as u64;
            let chunk = size / n;
            (0..n)
                .map(|i| {
                    let start = (i * chunk) as i64;
                    let end = if i == n - 1 { (size - 1) as i64 } else { ((i + 1) * chunk - 1) as i64 };
                    (start, Some(end))
                })
                .collect()
        }
        (Some(size), true) => vec![(0, Some((size - 1) as i64))],
        _ => vec![(0, None)],
    }
}

fn sanitize_filename(name: String) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "download".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The host part of a URL, lowercased, or `None` when it does not
/// parse. Used for the excluded-sites check (BUG-011).
pub(crate) fn url_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

pub(crate) fn filename_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    let decoded = percent_decode(last);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path that does not overwrite an existing file — or one already
/// reserved by another download: `name (2).ext`, …
fn unique_path(folder: &Path, name: &str, is_taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    let path = folder.join(name);
    if !is_taken(&path) {
        return path;
    }
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = Path::new(name).extension().and_then(|s| s.to_str()).unwrap_or("");
    let mut n = 2;
    loop {
        let candidate = if ext.is_empty() {
            folder.join(format!("{stem} ({n})"))
        } else {
            folder.join(format!("{stem} ({n}).{ext}"))
        };
        if !is_taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Play a WAV file through the platform sound path (S09 Sounds
/// tab). On Windows this spawns PowerShell's `SoundPlayer` in a
/// detached process (`PlaySync`, so the whole file plays) — the
/// call returns the file that was handed to the player, not a
/// guarantee anyone heard it: there is no audio device on this
/// VM and the Windows branch has never run here, like the
/// shutdown/hibernate branches. On any other OS this is an error
/// rather than a silent no-op, so the Test button can never claim
/// success for audio it did not play.
fn play_wav_file(path: &str) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        let escaped = path.replace('\'', "''");
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(New-Object Media.SoundPlayer '{escaped}').PlaySync()"),
            ])
            .spawn()
            .map_err(EngineError::from)?;
        Ok(path.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err(EngineError::InvalidState(
            "Sound playback is only available on Windows".to_string(),
        ))
    }
}

fn default_downloads_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        PathBuf::from(home).join("Downloads")
    } else {
        PathBuf::from("downloads")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    /// BUG-027 regression: the segment writer used Unix-only
    /// `FileExt::write_at`, which broke the first Windows build.
    /// `write_at_pos` must write at exact offsets on every platform:
    /// chunks written out of order (and one overlapping rewrite) must
    /// read back byte-exact.
    #[test]
    fn write_at_pos_writes_out_of_order_byte_exact() {
        let path = std::env::temp_dir().join(format!(
            "fetchline-write-at-pos-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let mut f = std::fs::File::create(&path).unwrap();
            // Pre-size like the segment writer does for a known size.
            f.set_len(12).unwrap();
            // Out of order: tail first, then head, then an overlap.
            let n = write_at_pos(&f, b"world", 6).unwrap();
            assert_eq!(n, 5);
            let n = write_at_pos(&f, b"hello ", 0).unwrap();
            assert_eq!(n, 6);
            let n = write_at_pos(&f, b"!", 11).unwrap();
            assert_eq!(n, 1);
            // Overlapping rewrite: replace the space at offset 5.
            let n = write_at_pos(&f, b"-", 5).unwrap();
            assert_eq!(n, 1);
            // Zero-length write at the end is a no-op, not an error.
            let n = write_at_pos(&f, b"", 12).unwrap();
            assert_eq!(n, 0);
            f.flush().unwrap();
        }
        let mut back = Vec::new();
        {
            let mut f = std::fs::File::open(&path).unwrap();
            f.seek(SeekFrom::Start(0)).unwrap();
            f.read_to_end(&mut back).unwrap();
        }
        assert_eq!(&back, b"hello-world!");
        // A read at a non-zero offset sees the same bytes.
        let mut f = std::fs::File::open(&path).unwrap();
        f.seek(SeekFrom::Start(6)).unwrap();
        let mut tail = [0u8; 5];
        f.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"world");
        let _ = std::fs::remove_file(&path);
    }
}
