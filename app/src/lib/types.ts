// Types mirror the Rust structs in engine/src (serde field names are
// snake_case on both sides of the Tauri bridge).

export type DownloadStatus =
  | 'queued'
  | 'downloading'
  | 'paused'
  | 'complete'
  | 'error'
  | 'deleted'

export interface DownloadRow {
  id: string
  url: string
  final_url: string | null
  filename: string
  save_path: string
  category_id: string
  queue_id: string | null
  size_bytes: number | null
  downloaded_bytes: number
  status: DownloadStatus
  speed_bps: number
  eta_seconds: number | null
  /** ETag validator from the probe, when the server sent one —
   *  used on resume (and on a refresh) to detect a changed file. */
  etag: string | null
  /** Last-Modified validator from the probe, when present. */
  last_modified: string | null
  error_message: string | null
  created_at: string
  completed_at: string | null
  /** Last attempt / last activity (UTC ISO-8601): the row's
   *  `updated_at` — bumped on every status change and on the 2 s
   *  checkpoint while downloading. Shown as "Last Try" (Round 44). */
  updated_at: string
  /** Site-grabber project that produced this download, if any
   *  (Round 44: the categories tree filters by project). */
  grabber_project_id: string | null
  /** Per-download speed cap in KB/s (S12); null = no cap. */
  speed_limit_kbps: number | null
  /** Whether the probe found the server range-capable (S03
   *  progress window): true = resumable, false = no ranges,
   *  null = unknown (added before this was stored). */
  accept_ranges: boolean | null
  /** Paused because the download quota was reached (S12). */
  quota_paused: boolean
  /** Antivirus scan result (S14): the outcome of the scanner
   *  hook run on completion, or null when never scanned. */
  scan_status: string | null
  /** The scanner's own report, or the reason the scan failed /
   *  was skipped. */
  scan_detail: string | null
  /** When the scan finished (UTC ISO-8601), or null while
   *  scanning / never scanned. */
  scan_scanned_at: string | null
  /** Free-text note from the Add URL dialog's Description field
   *  (Round 67, IDM's Download File Info has one). */
  description?: string | null
  /** Round 71 (D4) — a Completed download whose file no longer
   *  exists on disk (deleted outside the app). The row stays
   *  listed as Finished, but the UI says the file is missing
   *  instead of offering to open a file that is not there. */
  file_missing?: boolean
  /** Round 72 (D) — live connection/part counts, written by the
   *  ~1 s checkpoint while a download runs: how many connections
   *  are transferring right now, and how many parts the file is
   *  split into. The table shows both ("2 connections · 6 parts")
   *  so parts growing past connections never looks like the
   *  setting is ignored. 0 when not downloading. */
  active_connections: number
  segment_count: number
}

export interface SegmentProgress {
  segment_index: number
  start_byte: number
  end_byte: number | null
  downloaded_bytes: number
  status: string
}

/** One segment's retry wait, as the S03 dialog shows it: `attempt`
 *  counts the failures so far (1 = the first failure),
 *  `max_attempts` is the total attempts the settings allow, and
 *  `retry_in_secs` counts down from the engine's backoff deadline —
 *  it is recomputed on every progress event, so the dialog's
 *  countdown is driven by the engine, not by a UI timer.
 *  Round 72 (C): `max_attempts` is 0 for a patient tail wait —
 *  there is no attempt limit, and the dialog says so instead of
 *  showing "attempt N of 0". */
export interface RetryInfo {
  attempt: number
  max_attempts: number
  retry_in_secs: number
  error: string
}

/** Round 62 — per-download completion options (progress window
 *  Options tab). Mirrors the Rust `DownloadCompletionOptions`:
 *  `close_after` exits the app when the download completes,
 *  `shutdown_after` shuts the PC down after the window's 30 s
 *  cancellable countdown. In memory only. */
export interface DownloadCompletionOptions {
  close_after: boolean
  shutdown_after: boolean
}

export interface ProgressEvent {
  id: string
  filename: string
  downloaded_bytes: number
  size_bytes: number | null
  speed_bps: number
  eta_seconds: number | null
  status: string
  segments: SegmentProgress[]
  /** Round 72 (D) — how many connections are actively
   *  transferring right now (segments with a live task), as
   *  distinct from `segments.length`: the part count grows past
   *  the connection count as splits happen, and the progress
   *  window shows "N connections · M parts". */
  active_connections: number
  /** Set while a segment waits out its retry backoff; null otherwise. */
  retry: RetryInfo | null
  /** Antivirus scan result (S14), once known: set on the
   *  completion event and on the later event the detached scan
   *  emits; null while the download is still running. */
  scan: ScanResult | null
}

/** One antivirus scan's result (S14), as stored on the download
 *  row and shown in the completion dialog. `status` is one of
 *  `clean` | `infected` | `scan_failed` | `not_configured` |
 *  `timed_out` | `scanning`. */
export interface ScanResult {
  status: string
  detail: string | null
  scanned_at: string | null
}

export interface CategoryRow {
  id: string
  name: string
  default_folder: string
  file_extensions: string[]
  is_builtin: boolean
}

export interface ProbeInfo {
  size: number | null
  accept_ranges: boolean
  etag: string | null
  last_modified: string | null
  filename: string | null
  final_url: string
}

/** Round 51 — refresh download address: the engine's answer to a
 *  new link for a paused/failed download. `switched` means the row
 *  now points at the new link and the download is running again
 *  (`kept_bytes` is the progress carried over — 0 when a confirmed
 *  different file restarted from zero). `mismatch` means the new
 *  link serves a different file and NOTHING was changed — the dialog
 *  shows the new file's facts and only switches on confirm. */
export type RefreshAddressResult =
  | { outcome: 'switched'; kept_bytes: number; size_bytes: number | null }
  | { outcome: 'mismatch'; size_bytes: number | null; etag: string | null; filename: string | null }

/** Round 67 — one link the browser extension caught, waiting
 *  for the Add URL dialog. Mirrors the engine's `PendingLinkRow`:
 *  `source` is 'browser' | 'video' | 'download_all', `status`
 *  is 'pending' while it waits. `cookies` is the browser's
 *  cookies for the URL as a JSON array (or null) — the dialog's
 *  probe and confirm send them, so a cookie-gated link works. */
export interface PendingLinkRow {
  id: string
  url: string
  referrer: string | null
  source: string
  status: string
  download_id: string | null
  error: string | null
  cookies: string | null
  created_at: string
}

/** Round 52 — clipboard monitoring: the engine's offer for a
 *  download link copied to the clipboard. Mirrors the engine's
 *  `ClipboardOffer` (url + the filename the probe derived). */
export interface ClipboardOffer {
  url: string
  filename: string
}

/** Round 53 — download history: one finished download, as the
 *  history view shows it. Mirrors the engine's `HistoryRow`: the
 *  record survives removal from the main table, so a finished
 *  file can be found again, re-downloaded, or its folder opened.
 *  `completed_at` is a UTC ISO-8601 string; `size_bytes` is null
 *  when the size was never known. */
export interface HistoryRow {
  id: string
  download_id: string
  filename: string
  url: string
  final_url: string | null
  size_bytes: number | null
  folder: string
  category_id: string | null
  completed_at: string
}

/** Mirrors engine QueueRow. `type` is always "queue" for now; the
 *  scheduler fields are UTC ISO strings or null when unset. */
export interface QueueRow {
  id: string
  name: string
  type: string
  is_main: boolean
  schedule_type: string
  start_at: string | null
  stop_at: string | null
  start_on_startup: boolean
  on_complete_action: string
  on_complete_program: string | null
  /** Periodic synchronisation (S05/F04): minutes between sync runs,
   *  null unless the queue repeats. Last sync time + per-outcome
   *  counts (0 when never synced). */
  sync_interval_minutes: number | null
  last_sync_at: string | null
  last_sync_changed: number
  last_sync_unchanged: number
  last_sync_removed: number
  last_sync_failed: number
  last_sync_skipped: number
  running: boolean
  file_count: number
}

export type QueueScheduleType = 'none' | 'one_time' | 'periodic'
export type QueueCompleteAction = 'none' | 'open_file' | 'run_program' | 'shutdown' | 'hibernate'

export interface QueueUpdate {
  schedule_type: QueueScheduleType
  start_at: string | null
  stop_at: string | null
  start_on_startup: boolean
  on_complete_action: QueueCompleteAction
  on_complete_program: string | null
  /** Minutes between sync runs; only for `periodic`, null otherwise. */
  sync_interval_minutes: number | null
}

/** One file's outcome in a sync run (engine/src/manager.rs).
 *  `status` is unchanged | changed | removed | failed | skipped. */
export interface SyncFileResult {
  download_id: string
  filename: string
  status: string
  message: string | null
}

/** The outcome of one sync run over a periodic queue. */
export interface SyncResult {
  queue_id: string
  changed: number
  unchanged: number
  removed: number
  failed: number
  skipped: number
  files: SyncFileResult[]
}

/** The result of one update check (engine/src/update.rs).
 *  `update_available` only means the dialog should ask the
 *  user — the check never installs anything, and
 *  `updater_configured` is false until the Tauri updater plugin
 *  and a signed release pipeline exist. */
export interface UpdateCheckResult {
  current_version: string
  latest_version: string
  update_available: boolean
  changelog: string[]
  download_url: string | null
  checked_at: string
  source_url: string
  updater_configured: boolean
}

/** A completion action waiting for the UI (engine/src/queue.rs).
 *  Only shutdown / hibernate wait for a countdown; the others run
 *  as soon as the UI confirms them. */
export interface PendingAction {
  queue_id: string
  queue_name: string
  action: string
  program: string | null
  file_path: string | null
  created_at_unix: number
}

export type LinkKind = 'video' | 'audio' | 'image' | 'document' | 'archive' | 'program' | 'other'

/** One link from a page, as returned by `fetch_page_links`
 *  (engine/src/links.rs). `error` means the probe failed — the link
 *  is listed but cannot be selected. */
export interface PageLink {
  url: string
  filename: string
  kind: LinkKind
  size: number | null
  error: string | null
  already_exists: boolean
}

/** One line of a batch that was skipped, with its 1-based line number. */export interface InvalidLine {
  line: number
  url: string
  error: string
}

/** Result of adding a batch (S10) or a Download All selection (S07). */
export interface BatchResult {
  created: string[]
  invalid: InvalidLine[]
}

/** One video or audio track found on a page, as returned by
 *  `detect_page_videos` (engine/src/video.rs). `error` means the
 *  probe failed — the item is listed but cannot be downloaded.
 *  `live` marks a stream (an .m3u8 playlist): it is offered as-is,
 *  a playlist file, never merged into one video. */
export interface DetectedVideo {
  url: string
  filename: string
  format: string
  quality: string
  size: number | null
  error: string | null
  live: boolean
  downloadable: boolean
  /** Honest note for playlist/manifest items (saved as-is, never
   *  merged); null for plain files. */
  note: string | null
}

/** One file the site grabber (S06) matched. `status` is `matched`,
 *  `downloaded` or `failed` (with `error` saying why). */
export interface MatchedFile {
  url: string
  filename: string
  kind: string
  size: number | null
  status: string
  error: string | null
  download_id: string | null
}

export type GrabberStatus = 'idle' | 'exploring' | 'downloading' | 'done' | 'failed' | 'stopped'

/** One site-grabber project (engine/src/grabber.rs). `filters`
 *  holds the file-type list, explore depth and page limit;
 *  `schedule_start_at` is the optional one-time start time the
 *  existing scheduler uses. `queue_id` is the queue grabbed files
 *  go to (the main queue when the project names none);
 *  `on_complete_action` / `on_complete_program` reuse the existing
 *  queue completion actions, armed only when the run actually
 *  downloaded at least one file. */
export interface GrabberProjectRow {
  id: string
  name: string
  start_url: string
  filters: {
    types: string[]
    depth: number
    max_pages: number
  }
  mode: 'explore' | 'download'
  schedule_start_at: string | null
  queue_id: string
  on_complete_action: QueueCompleteAction
  on_complete_program: string | null
  status: GrabberStatus
  pages_explored: number
  files_matched: number
  files_downloaded: number
  files_failed: number
  error_message: string | null
  matched_files: MatchedFile[]
  created_at: string
}

/** App settings (S09), singleton row. `speed_limit_kbps` is null for
 *  unlimited; `default_action` is what a new download does by
 *  default; `duplicate_action` is what happens when the file already
 *  exists. `start_on_startup` is a stored flag only — registering
 *  the app in Windows autostart is out of scope for now. */
/** One saved site login, as the UI sees it. There is no password
 *  field on purpose — only `has_password`, so the dialog can show
 *  "password saved" without ever receiving the secret. */
export interface SiteLoginRow {
  id: string
  host: string
  username: string
  auth_type: string
  has_password: boolean
}

/** What the UI sends when saving a site login. `password: null`
 *  keeps the stored password; a value sets it. */
export interface SiteLoginInput {
  id: string | null
  host: string
  username: string
  password: string | null
  auth_type: string
}

/** One event's sound setting. `file` is a WAV path or null. */
export interface SoundSetting {
  event: string
  enabled: boolean
  file: string | null
}

export interface SoundSettingInput {
  event: string
  enabled: boolean
  file: string | null
}

export interface Settings {
  default_download_folder: string
  max_connections: number
  speed_limit_kbps: number | null
  language: string
  /** The engine still stores a theme (and accepts 'light'), but
   *  the UI is dark only since Round 45 — nothing reads this for
   *  theming any more, and Settings no longer offers the choice. */
  theme: 'light' | 'dark'
  browser_integration: boolean
  /** Clipboard monitoring (Round 52): watch the system clipboard
   *  and offer a copied download link (default true). Off means
   *  the poller still runs but the engine never offers a link. */
  clipboard_monitoring: boolean
  /** Dial-Up tab (S09 finish): legacy settings. The connection
   *  name may be null (none set); auto-dial and redial are
   *  honoured only where a dial-up connection exists (Windows
   *  RAS); hang-up after completion maps onto the on-completion
   *  action path. Redial attempts are 0–10, never download
   *  retries. */
  dialup_connection_name: string | null
  dialup_auto_dial: boolean
  dialup_hangup_after_completion: boolean
  dialup_redial_attempts: number
  /** Start a link the browser caught downloading as soon as the
   *  desktop app drains it (default true); false leaves it queued
   *  for the user to start by hand. Milestone 2 / S13 finish. */
  browser_start_drained: boolean
  /** Progress window (S03): open a separate progress window when
   *  a download starts (default true). Off leaves the table as
   *  the only progress view; double-clicking a row still opens
   *  its window. */
  show_progress_window: boolean
  /** "Download complete" dialog (Round 66): when a download
   *  finishes, its progress window closes and a separate dialog
   *  shows the file, size, URL and save path (default true).
   *  Off means no dialog — the progress window stays open with
   *  its Open file / Open folder footer. */
  show_download_complete_dialog: boolean
  /** Notification sounds master switch (Round 68): when off,
   *  neither the completion sound nor the error alert plays. */
  sounds_enabled: boolean
  antivirus_command: string | null
  /** Antivirus hook (S14): the on/off switch for the scan
   *  (default on — an empty command still means no scan) and
   *  the scan timeout in seconds (5–600, default 60). */
  antivirus_enabled: boolean
  antivirus_timeout_secs: number
  file_types_auto: string[]
  excluded_sites: string[]
  check_updates_weekly: boolean
  /** Update check: Fetchline's own release server (configurable;
   *  the default is a placeholder to replace with the real
   *  release server — see engine/src/update.rs). */
  update_url: string
  /** When the last update check ran (UTC ISO-8601), or null when
   *  no check has run yet. The weekly timer reads this. */
  last_update_check_at: string | null
  start_on_startup: boolean
  default_action: 'start_now' | 'save_for_later'
  duplicate_action: 'rename' | 'overwrite'
  confirm_delete: boolean
  connection_timeout_secs: number
  max_retries: number
  /** Download quota (S12): the on/off switch, the limit in MB
   *  every `quota_hours` hours (null = no quota configured). The
   *  live window (usage + window end) is not stored here — it
   *  comes from `getQuotaStatus`. */
  quota_enabled: boolean
  quota_mb: number | null
  quota_hours: number | null
}

/** The download quota's live state (S12), as the status bar and
 *  the Settings Connection tab show it. */
export interface QuotaStatus {
  enabled: boolean
  limit_mb: number | null
  window_hours: number | null
  used_bytes: number
  window_start_unix: number | null
  window_end_unix: number | null
  reached: boolean
  quota_paused_downloads: number
}

/** One proxy row, as the UI sees it. There is no password field on
 *  purpose — only `has_password`, so the dialog can show "password
 *  saved" without ever receiving the secret. */
export interface ProxyConfigRow {
  id: string
  name: string
  proxy_type: string
  host: string
  port: number
  username: string | null
  use_for: string[]
  bypass: string[]
  enabled: boolean
  has_password: boolean
  /** basic | ntlm | kerberos | negotiate-auto */
  auth: string
}

/** What the UI sends when saving a proxy. `password: null` keeps the
 *  stored password; `''` clears it; a value sets it. `auth: null`
 *  keeps the stored auth setting; a value sets it. */
export interface ProxyConfigInput {
  proxy_type: string
  host: string
  port: number
  username: string | null
  password: string | null
  use_for: string[]
  bypass: string[]
  enabled: boolean
  auth: string | null
}
