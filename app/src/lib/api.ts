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
  DownloadRow,
  GrabberProjectRow,
  HistoryRow,
  PageLink,
  PendingAction,
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

export async function pauseDownload(id: string): Promise<void> {
  return invoke('pause_download', { id })
}

export async function resumeDownload(id: string): Promise<void> {
  return invoke('resume_download', { id })
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

/** Resize a progress window between its full view (speed graph,
 *  parts, scan details) and its compact view (S03). */
export async function setProgressWindowCompact(id: string, compact: boolean): Promise<void> {
  return invoke('set_progress_window_compact', { downloadId: id, compact })
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

export async function updateQueue(id: string, update: QueueUpdate): Promise<QueueRow> {
  return invoke<QueueRow>('update_queue', {
    id,
    schedule_type: update.schedule_type,
    start_at: update.start_at,
    stop_at: update.stop_at,
    start_on_startup: update.start_on_startup,
    on_complete_action: update.on_complete_action,
    on_complete_program: update.on_complete_program,
    sync_interval_minutes: update.sync_interval_minutes,
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
  return invoke<GrabberProjectRow>('create_grabber_project', {
    name: args.name,
    start_url: args.startUrl,
    types: args.types,
    depth: args.depth,
    max_pages: args.maxPages,
    mode: args.mode,
    schedule_start_at: args.scheduleStartAt,
    queue_id: args.queueId ?? null,
    on_complete_action: args.onCompleteAction ?? 'none',
    on_complete_program: args.onCompleteProgram ?? null,
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

/** Antivirus hook (S14): open a finished download in its default
 *  application. Refused when the scanner flagged the file. */
export async function openFile(id: string): Promise<void> {
  return invoke('open_file', { id })
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
 *  a new downloadable link. Nothing is downloaded without the
 *  user — the UI shows the offer dialog for it. */
export function onClipboardLink(cb: (offer: ClipboardOffer) => void): Promise<() => void> {
  return listen<ClipboardOffer>('clipboard-link-found', (e) => cb(e.payload))
}

/** Round 52 — clipboard monitoring: the user dismissed the
 *  offer for this URL — the engine will not offer it again. */
export async function dismissClipboardOffer(url: string): Promise<void> {
  return invoke('dismiss_clipboard_offer', { url })
}
