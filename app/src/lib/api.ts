// Tauri bridge. Every command name matches architecture.md exactly.
// When the UI runs outside the desktop app (vite preview), invoke throws
// and callers surface the error instead of silently faking data.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  BatchResult,
  CategoryRow,
  ClipboardOffer,
  DetectedVideo,
  DownloadCompletionOptions,
  DownloadRow,
  GrabberProjectRow,
  HistoryRow,
  PageLink,
  PendingAction,
  PendingLinkRow,
  ProbeInfo,
  ProgressEvent,
  ProxyConfigInput,
  ProxyConfigRow,
  QueueRow,
  QueueUpdate,
  QuotaStatus,
  RefreshAddressResult,
  ScanResult,
  Settings,
  SiteLoginInput,
  SiteLoginRow,
  SoundSetting,
  SoundSettingInput,
  SyncResult,
  UpdateCheckResult,
  YtdlpResolveResult,
  YtdlpStatus,
} from './types'

/** Reject when an invoke does not settle in time (Round 56,
 * §7.3): a hung backend must REJECT instead of pending forever,
 * so the UI can declare "backend not responding" instead of
 * piling up invokes. The original promise keeps running in the
 * background — its result is ignored — which is the point: the
 * UI moves on while the hung call stays hung. */
export function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: number | undefined
  const timeout = new Promise<never>((_, reject) => {
    timer = window.setTimeout(() => reject(new Error(message)), ms)
  })
  return Promise.race([
    promise.finally(() => {
      if (timer !== undefined) window.clearTimeout(timer)
    }),
    timeout,
  ])
}

/** Webview heartbeat (§7.4, Round 56): each window's JS pings
 * this every 5 s with its label (`main`,
 * `progress-<download id>`); Rust logs "heartbeat lost" when
 * the pings stop for 30 s. */
export async function webviewHeartbeat(label: string): Promise<void> {
  return invoke('webview_heartbeat', { label })
}

/** Round 71 (C3) — show-when-ready: the window was built hidden
 *  (`visible: false`) so it never flashes a blank first paint;
 *  the frontend calls this once its root component mounts, and
 *  Rust shows the window WITH content. A failure is swallowed
 *  by the caller — the 3 s Rust fallback shows the window
 *  anyway, so a missed signal can never hide a window forever. */
export async function windowReady(): Promise<void> {
  return invoke('window_ready')
}

/** Reload a window's webview (§7.4, Round 56): the recovery
 * action the "Backend not responding" banner offers. The main
 * window reloads in place; a progress window is closed and
 * rebuilt from scratch. */
export async function reloadWebviewWindow(label: string): Promise<void> {
  return invoke('reload_webview_window', { label })
}

export async function listDownloads(): Promise<DownloadRow[]> {
  return invoke<DownloadRow[]>('list_downloads')
}

export async function getCategories(): Promise<CategoryRow[]> {
  return invoke<CategoryRow[]>('get_categories')
}

export async function createCategory(args: {
  name: string
  folder: string
  extensions: string[]
}): Promise<CategoryRow> {
  return invoke<CategoryRow>('create_category', args)
}

export async function deleteCategory(id: string): Promise<void> {
  return invoke('delete_category', { id })
}

export async function moveToCategory(downloadId: string, categoryId: string): Promise<void> {
  return invoke('move_to_category', { downloadId, categoryId })
}

export async function addDownload(args: {
  url: string
  filename?: string | null
  folder?: string | null
  category?: string | null
  queue?: string | null
}): Promise<[string, ProbeInfo]> {
  return invoke<[string, ProbeInfo]>('add_download', args)
}

export async function startDownload(id: string): Promise<void> {
  return invoke('start_download', { id })
}

/** Round 67 — the Add URL dialog's queue: pending
 *  browser-extension rows, oldest first. */
export async function listPendingLinks(): Promise<PendingLinkRow[]> {
  return invoke<PendingLinkRow[]>('list_pending_links')
}

/** Round 67 — decide one pending row from the Add URL dialog:
 *  'accepted' with the created download id, or 'rejected' with
 *  the reason (Cancel / close dismisses a capture). */
export async function resolvePendingLink(args: {  id: string
  status: 'accepted' | 'rejected'
  downloadId?: string | null
  error?: string | null
}): Promise<void> {
  return invoke('resolve_pending_link', {
    id: args.id,
    status: args.status,
    downloadId: args.downloadId ?? null,
    error: args.error ?? null,
  })
}

/** Round 67 — probe a URL for the Add URL dialog (file size and
 *  name). `cookies` is a browser capture's stored cookies JSON,
 *  or null — a cookie-gated link probes 401 without them.
 *  `headers` is a resolved video's headers JSON (User-Agent /
 *  Referer from yt-dlp's `http_headers`), or null — a stream
 *  host that expects them answers the global "Fetchline/0.1"
 *  UA with 403. */
export async function probeUrl(
  url: string,
  cookies?: string | null,
  headers?: string | null,
): Promise<ProbeInfo> {
  return invoke<ProbeInfo>('probe_url', { url, cookies: cookies ?? null, headers: headers ?? null })
}

/** Round 78 — the installed yt-dlp's version and sidecar
 *  presence, for the Settings "Video downloads" section. */
export async function ytdlpVersion(): Promise<YtdlpStatus> {
  return invoke<YtdlpStatus>('ytdlp_version')
}

/** Round 78 — resolve a platform video page from the app
 *  itself (a YouTube URL pasted into Add URL): title + trimmed
 *  quality list. No cookies — the app has no browser session. */
export async function ytdlpResolve(url: string): Promise<YtdlpResolveResult> {
  return invoke<YtdlpResolveResult>('ytdlp_resolve', { url })
}

/** Round 78 — download the newest yt-dlp.exe from its GitHub
 *  release (SHA-256 verified against the release's published
 *  checksums) and return the fresh status. */
export async function ytdlpUpdate(): Promise<YtdlpStatus> {
  return invoke<YtdlpStatus>('ytdlp_update')
}

/** Round 67 — confirm one pending browser/video capture through
 *  the real add path, with the Add URL dialog's choices. Returns
 *  the created download id and the probe info. */
export async function confirmPendingLink(args: {
  id: string
  filename?: string | null
  folder?: string | null
  category?: string | null
  queue?: string | null
  speedLimitKbps?: number | null
  startNow: boolean
}): Promise<[string, ProbeInfo]> {
  return invoke<[string, ProbeInfo]>('confirm_pending_link', {
    id: args.id,
    filename: args.filename ?? null,
    folder: args.folder ?? null,
    category: args.category ?? null,
    queue: args.queue ?? null,
    speedLimitKbps: args.speedLimitKbps ?? null,
    startNow: args.startNow,
  })
}

/** Round 74 (13) — read one pending browser-extension capture
 *  for the standalone Add URL window (`get_pending_link` on the
 *  backend; the window's `__ADD_URL_ID__` init script carries
 *  the link id). */
export async function getPendingLink(linkId: string): Promise<PendingLinkRow> {
  return invoke<PendingLinkRow>('get_pending_link', { linkId })
}

/** Round 74 (13) — open (or focus, when it already exists) the
 *  standalone Add URL window for a captured link, and pop it in
 *  front over whatever window the user has open. Tauri v2 commands
 *  take camelCase argument keys by default, so the Rust parameter
 *  `link_id` is sent as `linkId` (Round 75: sending `link_id`
 *  failed every call with "missing required key linkId"). */
export async function openAddUrlWindow(linkId: string): Promise<void> {
  return invoke('open_add_url_window', { linkId })
}

/** Round 67 — set the Add URL dialog's Description note on a
 *  download (null / empty clears it). */
export async function setDownloadDescription(id: string, description?: string | null): Promise<void> {
  return invoke('set_download_description', { id, description: description ?? null })
}

export async function pauseDownload(id: string): Promise<void> {
  return invoke('pause_download', { id })
}

export async function resumeDownload(id: string): Promise<void> {
  return invoke('resume_download', { id })
}

/** Round 72 (C) — Redownload in place: reset the row's segments
 *  and start it over from scratch without deleting the row first.
 *  A probe failure leaves the row Failed with the new error. */
export async function redownloadDownload(id: string): Promise<void> {
  return invoke('redownload_download', { id })
}

/** Round 51 — refresh download address: point a paused/failed
 *  download at a new link. The engine re-probes the link first:
 *  same file → the row switches and resumes from the downloaded
 *  bytes; different file → a mismatch result and nothing changes
 *  (pass force=true only after the user confirms, which restarts
 *  from zero); probe failure → an error, old link kept. */
export async function refreshDownloadAddress(
  id: string,
  url: string,
  force: boolean,
): Promise<RefreshAddressResult> {
  return invoke<RefreshAddressResult>('refresh_download_address', { id, url, force })
}

export async function cancelDownload(id: string): Promise<void> {
  return invoke('cancel_download', { id })
}

/** Skip the backoff of a download that is waiting for a retry
 *  (S03, "Retry now"): the waiting segment tries again at once.
 *  Fails when the download is not waiting for a retry. */
export async function retryNow(id: string): Promise<void> {
  return invoke('retry_now', { id })
}

/** Open the folder of a finished download in the system file
 *  manager (S03). Fails with a clear error when the folder no
 *  longer exists instead of opening nothing. */
export async function openFolder(id: string): Promise<void> {
  return invoke('open_folder', { id })
}

/** Open (or focus, when it already exists) the separate progress
 *  window for a download (S03). The window is a real OS window,
 *  titled "<percent>% <filename>"; closing it never cancels the
 *  download. */
export async function openProgressWindow(id: string): Promise<void> {
  return invoke('open_progress_window', { downloadId: id })
}

/** Round 62 — per-download completion options (progress window
 *  Options tab): "Close program after download" and "Shut down
 *  computer after download". Stored in memory on the Rust side;
 *  the window acts on them when its download completes. */
export async function setDownloadCompletionOptions(
  id: string,
  options: DownloadCompletionOptions,
): Promise<void> {
  return invoke('set_download_completion_options', {
    downloadId: id,
    closeAfter: options.close_after,
    shutdownAfter: options.shutdown_after,
  })
}

export async function getDownloadCompletionOptions(id: string): Promise<DownloadCompletionOptions> {
  return invoke<DownloadCompletionOptions>('get_download_completion_options', { downloadId: id })
}

/** Round 62 — exit the whole app ("Close program after
 *  download"). */
export async function exitApp(): Promise<void> {
  return invoke('exit_app')
}

/** Round 62 — shut the PC down now ("Shut down computer after
 *  download", after the window's 30 s countdown). Windows only. */
export async function shutdownComputer(): Promise<void> {
  return invoke('shutdown_computer')
}

/** Round 62 — free bytes on the volume holding `path` (main
 *  window status bar). Windows only; rejects elsewhere. */
export async function getFreeDiskSpace(path: string): Promise<number> {
  return invoke<number>('get_free_disk_space', { path })
}

/** The latest progress event for a download (S03) — the progress
 *  window's live feed. It polls this instead of listening for
 *  `download-progress` events: the listener was removed in Round 39
 *  (BUG-044) and polling is the window's verified live path. (The
 *  main window's `queue-action-pending` / `update-available`
 *  listeners work again since Round 40 added the capability file.) */
export async function getProgressSnapshot(id: string): Promise<ProgressEvent | null> {
  return invoke<ProgressEvent | null>('get_progress_snapshot', { downloadId: id })
}

export async function deleteDownload(id: string, deleteFile: boolean): Promise<void> {
  return invoke('delete_download', { id, deleteFile })
}

/** Round 53 — download history: finished downloads, recorded so
 *  they survive removal from the main table. `query` matches the
 *  filename or the URL (empty = everything, newest first). */
export async function listDownloadHistory(query: string): Promise<HistoryRow[]> {
  return invoke<HistoryRow[]>('list_download_history', { query })
}

/** Round 53 — download history: erase every history record. The
 *  files on disk are kept — only the records go. */
export async function clearDownloadHistory(): Promise<void> {
  return invoke('clear_download_history')
}

/** Round 53 — download history: download a finished file again
 *  from its recorded link. Returns the new download's id and
 *  probe info, exactly like `add_download`. */
export async function redownloadFromHistory(id: string): Promise<[string, ProbeInfo]> {
  return invoke<[string, ProbeInfo]>('redownload_from_history', { id })
}

/** Round 53 — download history: open the folder a finished
 *  download was saved to. Fails with a clear error when the
 *  folder no longer exists. */
export async function openHistoryFolder(id: string): Promise<void> {
  return invoke('open_history_folder', { id })
}

export async function listQueues(): Promise<QueueRow[]> {
  return invoke<QueueRow[]>('list_queues')
}

export async function getQueue(id: string): Promise<QueueRow | null> {
  return invoke<QueueRow | null>('get_queue', { id })
}

export async function getQueueFiles(queueId: string): Promise<DownloadRow[]> {
  return invoke<DownloadRow[]>('get_queue_files', { queueId })
}

export async function createQueue(name: string): Promise<QueueRow> {
  return invoke<QueueRow>('create_queue', { name })
}

/** Round 74 (6) — delete a queue. The backend command is
 *  `delete_queue(id)`; the main queue cannot be deleted. The UI
 *  never calls this for the main queue. */
export async function deleteQueue(id: string): Promise<void> {
  return invoke('delete_queue', { id })
}

/** Round 74 (6) — rename a queue. The backend command is
 *  `rename_queue(id, name)`. */
export async function renameQueue(id: string, name: string): Promise<QueueRow> {
  return invoke<QueueRow>('rename_queue', { id, name })
}

export async function updateQueue(id: string, update: QueueUpdate): Promise<QueueRow> {
  // Round 75: Tauri v2 commands take camelCase argument keys — the
  // snake_case keys sent before made EVERY call fail with
  // "missing required key scheduleType" (the Rust parameters are
  // schedule_type/start_on_startup/on_complete_action), so queue
  // scheduling could never be saved from the UI.
  return invoke<QueueRow>('update_queue', {
    id,
    scheduleType: update.schedule_type,
    startAt: update.start_at,
    stopAt: update.stop_at,
    startOnStartup: update.start_on_startup,
    onCompleteAction: update.on_complete_action,
    onCompleteProgram: update.on_complete_program,
    syncIntervalMinutes: update.sync_interval_minutes,
  })
}

/** Periodic synchronisation (S05/F04): sync a queue now, outside its
 *  schedule, and wait for the result — which files changed, which
 *  were unchanged, which are gone from the server. */
export async function syncQueueNow(id: string): Promise<SyncResult> {
  return invoke<SyncResult>('sync_queue_now', { id })
}

export async function addToQueue(downloadId: string, queueId: string): Promise<void> {
  return invoke('add_to_queue', { downloadId, queueId })
}

export async function removeFromQueue(downloadId: string): Promise<void> {
  return invoke('remove_from_queue', { downloadId })
}

export async function reorderQueue(
  queueId: string,
  downloadId: string,
  newIndex: number,
): Promise<void> {
  return invoke('reorder_queue', { queueId, downloadId, newIndex })
}

export async function startQueue(id: string): Promise<void> {
  return invoke('start_queue', { id })
}

export async function stopQueue(id: string): Promise<void> {
  return invoke('stop_queue', { id })
}

export async function executeQueueAction(queueId: string): Promise<void> {
  return invoke('execute_queue_action', { queueId })
}

export async function dismissQueueAction(queueId: string): Promise<void> {
  return invoke('dismiss_queue_action', { queueId })
}

export async function getPendingAction(queueId: string): Promise<PendingAction | null> {
  return invoke<PendingAction | null>('get_pending_action', { queueId })
}

export async function fetchPageLinks(pageUrl: string): Promise<PageLink[]> {
  return invoke<PageLink[]>('fetch_page_links', { pageUrl })
}

/** Video grabber (S08): scan a page for downloadable video/audio.
 *  Only openly downloadable streams are returned; dead links carry
 *  their probe error, and an .m3u8 playlist is flagged `live`. */
export async function detectPageVideos(pageUrl: string): Promise<DetectedVideo[]> {
  return invoke<DetectedVideo[]>('detect_page_videos', { pageUrl })
}

/** Site grabber (S06): create a project, run/stop it, and read its
 *  live progress. The engine enforces the crawl rules (same host
 *  only, depth ≤ 5, page cap, 500 ms between pages, robots.txt).
 *  `queueId` is the queue grabbed files go to (the main queue when
 *  omitted); `onCompleteAction` reuses the existing queue
 *  completion actions and is armed only when the run actually
 *  downloaded something. */
export async function createGrabberProject(args: {
  name: string
  startUrl: string
  types: string[]
  depth: number
  maxPages: number
  mode: 'explore' | 'download'
  scheduleStartAt: string | null
  queueId?: string | null
  onCompleteAction?: string
  onCompleteProgram?: string | null
}): Promise<GrabberProjectRow> {
  // Round 75: camelCase invoke keys (Tauri v2 default) — the
  // snake_case keys sent before made EVERY call fail with
  // "missing required key startUrl", so no Site Grabber project
  // could ever be created from the UI.
  return invoke<GrabberProjectRow>('create_grabber_project', {
    name: args.name,
    startUrl: args.startUrl,
    types: args.types,
    depth: args.depth,
    maxPages: args.maxPages,
    mode: args.mode,
    scheduleStartAt: args.scheduleStartAt,
    queueId: args.queueId ?? null,
    onCompleteAction: args.onCompleteAction ?? 'none',
    onCompleteProgram: args.onCompleteProgram ?? null,
  })
}

export async function runGrabber(id: string): Promise<void> {
  return invoke('run_grabber', { id })
}

export async function stopGrabber(id: string): Promise<void> {
  return invoke('stop_grabber', { id })
}

export async function getGrabberProject(id: string): Promise<GrabberProjectRow> {
  return invoke<GrabberProjectRow>('get_grabber_project', { id })
}

export async function listGrabberProjects(): Promise<GrabberProjectRow[]> {
  return invoke<GrabberProjectRow[]>('list_grabber_projects')
}

export async function addBatch(args: {
  urls: string[]
  folder?: string | null
  category?: string | null
  queue?: string | null
  startNow: boolean
}): Promise<BatchResult> {
  return invoke<BatchResult>('add_batch', {
    urls: args.urls,
    folder: args.folder ?? null,
    category: args.category ?? null,
    queue: args.queue ?? null,
    startNow: args.startNow,
  })
}

export async function downloadAllLinks(args: {
  pageUrl: string
  links: string[]
  folder?: string | null
  category?: string | null
  queue?: string | null
  startNow: boolean
}): Promise<BatchResult> {  return invoke<BatchResult>('download_all_links', {
    pageUrl: args.pageUrl,
    links: args.links,
    folder: args.folder ?? null,
    category: args.category ?? null,
    queue: args.queue ?? null,
    startNow: args.startNow,
  })
}

export async function updateCategory(args: {
  id: string
  folder?: string | null
  extensions?: string[] | null
}): Promise<CategoryRow> {
  return invoke<CategoryRow>('update_category', {
    id: args.id,
    folder: args.folder ?? null,
    extensions: args.extensions ?? null,
  })
}

/** Settings (S09). The whole row is sent on save; the engine
 *  validates it and applies it live. */
export async function getSettings(): Promise<Settings> {
  return invoke<Settings>('get_settings')
}

export async function updateSettings(settings: Settings): Promise<Settings> {
  return invoke<Settings>('update_settings', { settings })
}

/** Change only the global speed cap (S12). `null` means unlimited. */
export async function setSpeedLimit(kbps: number | null): Promise<Settings> {
  return invoke<Settings>('set_speed_limit', { kbps })
}

export async function getSpeedLimit(): Promise<number | null> {
  return invoke<number | null>('get_speed_limit')
}

/** Download quota (S12): the limit (X MB every Y hours, enabled),
 *  the live usage/window state, and the manual reset. Reset starts
 *  a fresh window at once and resumes quota-paused downloads. */
export async function setDownloadLimit(mb: number, hours: number): Promise<Settings> {
  return invoke<Settings>('set_download_limit', { mb, hours })
}

export async function getQuotaStatus(): Promise<QuotaStatus> {
  return invoke<QuotaStatus>('get_quota_status')
}

export async function resetQuota(): Promise<QuotaStatus> {
  return invoke<QuotaStatus>('reset_quota')
}

/** Per-download speed cap (S12). `null` (or 0) means no cap — the
 *  global cap still applies. Live-changeable on a running download. */
export async function setDownloadSpeedLimit(id: string, kbps: number | null): Promise<void> {
  return invoke('set_download_speed_limit', { id, kbps })
}

/** Proxy (S09 Proxy/Socks tab). The row never contains the password. */
export async function getProxyConfig(): Promise<ProxyConfigRow | null> {
  return invoke<ProxyConfigRow | null>('get_proxy_config')
}

export async function saveProxyConfig(input: ProxyConfigInput): Promise<ProxyConfigRow> {
  return invoke<ProxyConfigRow>('save_proxy_config', { input })
}

/** Site logins (S09 Site logins tab). Rows never contain a
 *  password — only `has_password`. `password: null` on save keeps
 *  the stored password; a value sets it. */
export async function listSiteLogins(): Promise<SiteLoginRow[]> {
  return invoke<SiteLoginRow[]>('list_site_logins')
}

export async function saveSiteLogin(input: SiteLoginInput): Promise<SiteLoginRow> {
  return invoke<SiteLoginRow>('save_site_login', { input })
}

export async function deleteSiteLogin(id: string): Promise<void> {
  return invoke('delete_site_login', { id })
}

/** Sounds (S09 Sounds tab). One setting per event; the engine
 *  emits the event and plays the WAV on Windows only. */
export async function getSoundSettings(): Promise<SoundSetting[]> {
  return invoke<SoundSetting[]>('get_sound_settings')
}

export async function saveSoundSettings(inputs: SoundSettingInput[]): Promise<SoundSetting[]> {
  return invoke<SoundSetting[]>('save_sound_settings', { inputs })
}

/** Play a WAV file (the Sounds tab's Test button). Validates the
 *  file first; on this Linux VM playback is Windows-only, so the
 *  honest result here is the validation, not audio. */
export async function testSoundFile(path: string): Promise<string> {
  return invoke<string>('test_sound_file', { path })
}

/** Probe a URL through a proxy that has not been saved yet (the Test
 *  button). Returns the HTTP status. */
export async function testProxy(input: ProxyConfigInput, testUrl: string): Promise<number> {
  return invoke<number>('test_proxy', { input, testUrl })
}

/** Update check (S09): fetch the version manifest from the
 *  configured update server. Never installs anything — the
 *  result is for the update dialog to show, and the user
 *  decides. Always runs, even when the weekly setting is off. */
export async function checkForUpdates(): Promise<UpdateCheckResult> {
  return invoke<UpdateCheckResult>('check_for_updates')
}

/** Antivirus hook (S14): run the configured scanner against a
 *  finished download and record the result. A threat, a failure
 *  and a timeout are all results the dialog shows, not errors. */
export async function runAntivirusScan(id: string): Promise<ScanResult> {
  return invoke<ScanResult>('run_antivirus_scan', { id })
}

/** Antivirus hook (S14): the stored scan result for a download,
 *  or null when it was never scanned. */
export async function getScanResult(id: string): Promise<ScanResult | null> {
  return invoke<ScanResult | null>('get_scan_result', { id })
}

/** Antivirus hook (S14): the Settings > Downloads "Test" button
 *  — run the command typed in the dialog (saved or not) against
 *  a small throwaway file and report the result. */
export async function testAntivirusScanner(
  command: string,
  timeoutSecs: number,
): Promise<ScanResult> {
  return invoke<ScanResult>('test_antivirus_scanner', { command, timeoutSecs })
}

/** Antivirus hook (S14): move a file the scanner flagged as a
 *  threat into a `Quarantine` folder. Explicit user action only
 *  — nothing ever calls this on its own. Returns the new path. */
export async function quarantineDownload(id: string): Promise<string> {
  return invoke<string>('quarantine_download', { id })
}

/** Round 66 — "Download complete" dialog: open (or focus) the
 *  separate dialog window for a finished download. The main
 *  window calls this when a download flips to 'complete'; the
 *  command is idempotent — an already-open dialog is focused
 *  instead of a second copy. */
export async function openCompleteDialog(id: string): Promise<void> {
  return invoke('open_complete_dialog', { downloadId: id })
}

/** Round 68 — open the "Download failed" dialog for a failed
 *  download (the main window calls this when a download flips
 *  to 'error'; the progress window stays open behind it). */
export async function openErrorDialog(id: string): Promise<void> {
  return invoke('open_error_dialog', { downloadId: id })
}

/** Round 66 — play the completion sound when the "Download
 *  complete" dialog shows: the user's configured
 *  `download_complete` sound when one is set (the engine
 *  already played it at completion, so this is a no-op), else
 *  the built-in chime, else the Windows default sound. Returns
 *  a short note saying which one played. Best-effort — callers
 *  swallow failures, a silent dialog is better than an error. */
export async function playCompletionSound(): Promise<string> {
  return invoke<string>('play_completion_sound')
}

/** Round 68 — play the error alert when the "Download failed"
 *  dialog shows: the user's configured `download_failed` sound
 *  when one is set (the engine already played it), else the
 *  built-in descending alert, else the Windows default sound.
 *  Silent when sounds are turned off in Settings. */
export async function playErrorSound(): Promise<string> {
  return invoke<string>('play_error_sound')
}

/** Round 68 — taskbar progress: the overall download percent
 *  on the main window's taskbar button, plus the file / speed /
 *  time-left summary in the window title (the taskbar tooltip).
 *  `progress` is 0–100 or null to clear; `status` is 'normal' |
 *  'paused' | 'error'. Best-effort — the caller swallows
 *  failures. */
export async function setTaskbarProgress(
  progress: number | null,
  status: 'normal' | 'paused' | 'error' | null,
  title: string | null,
): Promise<void> {
  return invoke('set_taskbar_progress', { progress, status, title })
}

/** Antivirus hook (S14): open a finished download in its default
 *  application. Refused when the scanner flagged the file. */
export async function openFile(id: string): Promise<void> {
  return invoke('open_file', { id })
}

/** Round 65 — "Open with..." (right-click menu): show the
 *  Windows "Open with" dialog for a download's file. */
export async function openFileWith(id: string): Promise<void> {
  return invoke('open_file_with', { id })
}

/** Subscribe to `queue-action-pending` events: a queue drained and its
 *  on-completion action is waiting for the UI countdown. */
export function onQueueAction(cb: (action: PendingAction) => void): Promise<() => void> {
  return listen<PendingAction>('queue-action-pending', (e) => cb(e.payload))
}

/** Subscribe to `update-available` events: the weekly check
 *  found a newer version. The UI shows the update dialog for
 *  it; nothing is installed without the user. */
export function onUpdateAvailable(
  cb: (result: UpdateCheckResult) => void,
): Promise<() => void> {
  return listen<UpdateCheckResult>('update-available', (e) => cb(e.payload))
}

/** Round 52 — clipboard monitoring: subscribe to
 *  `clipboard-link-found` events. The shell polls the clipboard
 *  once a second; the engine only emits when the copied text is
 *  a new downloadable link (a fresh copy of the same link is a
 *  new clipboard sequence, so it emits again). Nothing is
 *  downloaded without the user — the UI opens the Add URL
 *  dialog for it directly (Round 75). */
export function onClipboardLink(cb: (offer: ClipboardOffer) => void): Promise<() => void> {
  return listen<ClipboardOffer>('clipboard-link-found', (e) => cb(e.payload))
}

/** Round 75 — the app itself just wrote to the clipboard (the
 *  update dialog copies the installer URL): record the current
 *  clipboard sequence as seen, so the monitor does not offer our
 *  own link back to the user. */
export async function noteOwnClipboardWrite(): Promise<void> {
  return invoke('note_own_clipboard_write')
}
