import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { X } from 'lucide-react'
import {
  addToQueue,
  cancelDownload,
  checkForUpdates,
  deleteCategory,
  deleteDownload,
  deleteQueue,
  dismissClipboardOffer,
  executeQueueAction,
  exitApp,
  getCategories,
  getDownloadCompletionOptions,
  getFreeDiskSpace,
  getPendingAction,
  getQueueFiles,
  getQuotaStatus,
  getSettings,
  listDownloads,
  listGrabberProjects,
  listPendingLinks,
  listQueues,
  moveToCategory,
  onClipboardLink,
  onQueueAction,
  onUpdateAvailable,
  openAddUrlWindow,
  openFile,
  openFileWith,
  openFolder,
  openCompleteDialog,
  openErrorDialog,
  openProgressWindow,
  pauseDownload,
  redownloadDownload,
  reloadWebviewWindow,
  removeFromQueue,
  renameQueue,
  resumeDownload,
  runGrabber,
  setTaskbarProgress,
  startQueue,
  stopQueue,
  syncQueueNow,
  updateSettings,
  withTimeout,
} from './lib/api'
import { formatBytes, formatEta, formatSpeed } from './lib/format'
import type { CategoryRow, ClipboardOffer, DownloadRow, GrabberProjectRow, PendingAction, PendingLinkRow, QueueRow, QuotaStatus, Settings, UpdateCheckResult } from './lib/types'
import { AddCategoryDialog } from './components/AddCategoryDialog'
import { AddDownloadDialog } from './components/AddDownloadDialog'
import { AddQueueDialog } from './components/AddQueueDialog'
import { BatchDialog } from './components/BatchDialog'
import { Button } from './components/Button'
import { CategorySidebar, type SidebarFilter } from './components/CategorySidebar'
import { ClipboardOfferDialog } from './components/ClipboardOfferDialog'
import { DownloadAllDialog } from './components/DownloadAllDialog'
import { DownloadErrorDialog } from './components/DownloadErrorDialog'
import { DownloadPropertiesDialog } from './components/DownloadPropertiesDialog'
import { DownloadTable } from './components/DownloadTable'
import { HistoryView } from './components/HistoryView'
import { MenuBar, type MenuBarActions } from './components/MenuBar'
import { QueueActionDialog } from './components/QueueActionDialog'
import { RefreshAddressDialog } from './components/RefreshAddressDialog'
import { SchedulerDialog } from './components/SchedulerDialog'
import { SettingsDialog } from './components/SettingsDialog'
import { SiteGrabberDialog } from './components/SiteGrabberDialog'
import { TitleBar } from './components/TitleBar'
import { Toolbar } from './components/Toolbar'
import { ToolsStrip } from './components/ToolsStrip'
import { UpdateDialog } from './components/UpdateDialog'
import { VideoGrabberDialog } from './components/VideoGrabberDialog'

/** S01 — main window (Round 62): the approved clean design. The
 *  window is frameless (`decorations: false` in tauri.conf.json)
 *  with its own dark title bar (new logo + "Sonic Manager"),
 *  then the menu bar (File, Tasks, View, Downloads, Options,
 *  Help), the slim toolbar (gradient Add URL, Resume, Pause,
 *  Stop, Delete, search), the TOOLS strip (Queue, Scheduler,
 *  Video Grabber, Site Grabber, Clipboard), the categories
 *  tree on the left and the file table. Dark only — the light
 *  theme was deleted at the user's instruction. No settings
 *  gear anywhere: Settings is reached from the Options menu.
 *  The list is refreshed once a second and that refresh is the
 *  live path for the table — the `download-progress` listener that
 *  used to patch rows between polls is gone (BUG-044: it never
 *  fired in the built app). Queue completion actions are
 *  subscribed on `queue-action-pending`: shutdown / hibernate wait
 *  for the countdown dialog, the others run at once — but that
 *  listener shares the same BUG-044 denial, so a waiting action
 *  is also surfaced by the mount-time `getPendingAction` sweep
 *  below. */
export default function App() {
  const [downloads, setDownloads] = useState<DownloadRow[]>([])
  const [categories, setCategories] = useState<CategoryRow[]>([])
  const [queues, setQueues] = useState<QueueRow[]>([])
  const [grabberProjects, setGrabberProjects] = useState<GrabberProjectRow[]>([])
  const [queueFiles, setQueueFiles] = useState<DownloadRow[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null)
  const [selectedQueue, setSelectedQueue] = useState<string | null>(null)
  /** Which list the table shows: every download (or one
   *  category's), the unfinished / finished filter, the download
   *  history (Round 53 — the main area renders HistoryView
   *  instead of the table), or one grabber project's downloads. */
  const [selectedFilter, setSelectedFilter] = useState<SidebarFilter>('all')
  const [selectedGrabberId, setSelectedGrabberId] = useState<string | null>(null)
  const [showAdd, setShowAdd] = useState(false)
  /** Round 67 — browser-extension captures waiting for the Add
   *  URL dialog (like IDM: the download only starts when the
   *  user presses Start Download). The queue is polled with the
   *  1 s refresh; one capture at a time opens the dialog, and a
   *  capture that arrives while the dialog is open waits its
   *  turn — dialogs never stack. `seenCaptureIds` is the set of
   *  ids already queued (or showing), so a poll never queues
   *  the same capture twice; an id leaves the set when its
   *  capture is decided. */
  const [captureQueue, setCaptureQueue] = useState<PendingLinkRow[]>([])
  const seenCaptureIds = useRef<Set<string>>(new Set())
  /** Round 74 (12) — a clipboard offer the user accepted: the
   *  Add URL dialog opens in capture mode for it (file name as
   *  the title, URL locked) instead of adding the download
   *  straight away. */
  const [clipboardCapture, setClipboardCapture] = useState<ClipboardOffer | null>(null)
  const clipboardAddedRef = useRef(false)
  const refreshCapturesInFlight = useRef(false)
  const [showDownloadAll, setShowDownloadAll] = useState(false)
  const [showBatch, setShowBatch] = useState(false)
  const [showVideoGrabber, setShowVideoGrabber] = useState(false)
  const [showSiteGrabber, setShowSiteGrabber] = useState(false)
  const [pickedVideoUrl, setPickedVideoUrl] = useState('')
  const [showAddCategory, setShowAddCategory] = useState(false)
  const [showAddQueue, setShowAddQueue] = useState(false)
  const [showScheduler, setShowScheduler] = useState(false)
  const [schedulerQueueId, setSchedulerQueueId] = useState<string | null>(null)
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null)
  const [error, setError] = useState<string | null>(null)
  /** Round 55 (defect 4): action errors are STICKY — the 1 s poll
   *  clears `error` on every successful refresh, so a Resume /
   *  Pause failure used to flash for under a second and vanish,
   *  looking like a silent no-op. `bannerError` is only cleared by
   *  the dismiss button or by a later successful action, never by
   *  the poll. The banner shows it first, else the transient poll
   *  error. */
  const [bannerError, setBannerError] = useState<string | null>(null)
  /** Round 72 (C) — row-action busy feedback: Resume /
   *  Redownload re-probe first, and against a flaky server the
   *  probe takes up to 16 s with no UI change — the Round 71
   *  retest read that as "clicks do nothing". While any `run()`
   *  action is in flight for more than 400 ms the status bar
   *  shows "Working…" so a slow probe is visibly doing
   *  something. The delay keeps fast actions from flickering
   *  the bar. */
  const [actionBusy, setActionBusy] = useState(false)
  const actionBusyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  /** Round 71 (D2) — the banner's X must STICK. The 1 s poll
   *  re-ran `list_downloads` (and the secondary polls) and every
   *  failure re-populated `error` from the same old error rows,
   *  so the banner came back a second after the user dismissed
   *  it. `dismissedBannerRef` remembers the exact message the
   *  user dismissed; `setPollError` — used by every poll-failure
   *  path — refuses to re-show that message. A NEW error (a
   *  different message) still shows, and a successful poll clears
   *  the memory, so a genuinely new failure with the same text
   *  is not muted forever. */
  const dismissedBannerRef = useRef<string | null>(null)
  const setPollError = useCallback((msg: string) => {
    if (dismissedBannerRef.current === msg) return
    setError(msg)
  }, [])
  /** Round 69 (d) — an Add URL submit failure, shown in the
   *  standalone error dialog (IDM-style, with Details) as an
   *  overlay above the Add dialog. */
  const [addError, setAddError] = useState<{ title: string; message: string } | null>(null)
  function actionError(e: unknown) {
    setBannerError(e instanceof Error ? e.message : String(e))
  }
  /** Round 56 (§7.3): backend-not-responding guard. The 1 s poll
   *  used to fire a new `list_downloads` invoke every second even
   *  when the previous one never returned — a wedged backend
   *  piled up invokes forever while the table sat frozen at 0%.
   *  `refreshInFlight` skips a tick while a refresh is still
   *  running; the poll invoke itself times out after 5 s
   *  (`withTimeout`), so a hung invoke REJECTS instead of
   *  pending forever; and when no poll has succeeded for ~10 s
   *  the window declares the backend dead, stops issuing polls,
   *  and offers Retry / Reload. A successful poll clears the dead
   *  state. The refs mirror the state so the interval callback
   *  never reads a stale closure. */
  const [backendDead, setBackendDead] = useState(false)
  const backendDeadRef = useRef(false)
  const refreshInFlight = useRef(false)
  /** Round 57 (§9.2d): the secondary polls (`refreshQueues`,
   *  `refreshGrabberProjects`, `getQueueFiles`) had neither the
   *  5 s timeout nor the in-flight guard the main poll got in
   *  Round 56 — a wedged backend piled up one hung invoke per
   *  second for each of them too, and `refreshAll` awaited them
   *  in sequence, so one hung call stalled every later refresh.
   *  One shared guard for all three (they always run together
   *  from `refreshAll`); a skipped tick is just a skipped tick. */
  const secondaryRefreshInFlight = useRef(false)
  const pollStoppedRef = useRef(false)
  const lastPollOkAt = useRef(Date.now())
  const [settings, setSettings] = useState<Settings | null>(null)
  /** Download quota (S12): live usage, shown in the status bar
   *  when a quota is enabled; refreshed with the settings. */
  const [quotaStatus, setQuotaStatus] = useState<QuotaStatus | null>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState<DownloadRow | null>(null)
  /** Round 73 — the Redownload warning dialog's row, if open. */
  const [confirmRedownload, setConfirmRedownload] = useState<DownloadRow | null>(null)

  // Round 74 (14) — Escape closes the remove / redownload
  // confirms (they have Keep buttons but no X and no key close).
  useEffect(() => {
    if (confirmRemove === null && confirmRedownload === null) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setConfirmRemove(null)
        setConfirmRedownload(null)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [confirmRemove, confirmRedownload])
  /** Round 51 — refresh download address: the row whose expired
   *  link is being replaced; null when the dialog is closed. */
  const [refreshTarget, setRefreshTarget] = useState<DownloadRow | null>(null)
  /** Round 65 — Properties (right-click menu): the row whose
   *  details are showing; null when the dialog is closed. */
  const [propertiesTarget, setPropertiesTarget] = useState<DownloadRow | null>(null)
  /** Round 52 — clipboard monitoring: the copied link currently
   *  offered to the user; null when no offer is showing. */
  const [clipboardOffer, setClipboardOffer] = useState<ClipboardOffer | null>(null)
  /** Update check (S09): Help > Check for updates, and the weekly
   *  check's `update-available` event, both land here. */
  const [updateResult, setUpdateResult] = useState<UpdateCheckResult | null>(null)
  const [updateCheckError, setUpdateCheckError] = useState<string | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [showUpdateDialog, setShowUpdateDialog] = useState(false)
  /** Round 62 — the toolbar search box filters the table by
   *  filename (case-insensitive substring). */
  const [searchQuery, setSearchQuery] = useState('')
  /** Round 62 — free bytes on the download drive, for the
   *  status bar ("C: 21.2 GB free"). Null outside the desktop
   *  app / off Windows, where the command rejects. */
  const [freeSpace, setFreeSpace] = useState<{ drive: string; bytes: number } | null>(null)

  const refreshSettings = useCallback(async () => {
    try {
      const s = await getSettings()
      setSettings(s)
      try {
        setQuotaStatus(await getQuotaStatus())
      } catch {
        // Quota state is a status-bar nicety, not settings: a
        // failure here must not hide the settings themselves.
      }
    } catch {
      // Outside the desktop app (vite preview) the bridge throws;
      // the app keeps working with local defaults.
    }
  }, [])

  const refreshQueues = useCallback(async () => {
    // In-flight guard + 5 s timeout (§9.2d, Round 57): the same
    // protection the main poll got in Round 56 — a hung invoke
    // must reject instead of pending forever and piling up.
    if (secondaryRefreshInFlight.current) return
    secondaryRefreshInFlight.current = true
    try {
      setQueues(await withTimeout(listQueues(), 5000, 'Backend did not respond within 5 seconds'))
    } catch (e) {
      setPollError(e instanceof Error ? e.message : String(e))
    } finally {
      secondaryRefreshInFlight.current = false
    }
  }, [])

  const refreshGrabberProjects = useCallback(async () => {
    // In-flight guard + 5 s timeout (§9.2d, Round 57) — see
    // `refreshQueues` above.
    if (secondaryRefreshInFlight.current) return
    secondaryRefreshInFlight.current = true
    try {
      setGrabberProjects(await withTimeout(listGrabberProjects(), 5000, 'Backend did not respond within 5 seconds'))
    } catch {
      // The tree's grabber section stays empty outside the
      // desktop app; the rest of the window keeps working.
    } finally {
      secondaryRefreshInFlight.current = false
    }
  }, [])

  const refreshQueueFiles = useCallback(async () => {
    if (selectedQueue === null) return
    // In-flight guard + 5 s timeout (§9.2d, Round 57) — see
    // `refreshQueues` above.
    if (secondaryRefreshInFlight.current) return
    secondaryRefreshInFlight.current = true
    try {
      setQueueFiles(await withTimeout(getQueueFiles(selectedQueue), 5000, 'Backend did not respond within 5 seconds'))
    } catch (e) {
      setPollError(e instanceof Error ? e.message : String(e))
    } finally {
      secondaryRefreshInFlight.current = false
    }
  }, [selectedQueue])

  const refreshCategories = useCallback(async () => {
    try {
      setCategories(await getCategories())
    } catch (e) {
      setPollError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const refresh = useCallback(async () => {
    // In-flight guard (§7.3): skip the tick while a refresh is
    // still running — a hung invoke must not pile up a new one
    // every second.
    if (refreshInFlight.current) return
    refreshInFlight.current = true
    try {
      setDownloads(await withTimeout(listDownloads(), 5000, 'Backend did not respond within 5 seconds'))
      setError(null)
      // Round 71 (D2) — a successful poll clears the dismissed
      // memory: the old failure is gone, so a later failure with
      // the same text is a new failure and must show again.
      dismissedBannerRef.current = null
      lastPollOkAt.current = Date.now()
      if (backendDeadRef.current) {
        backendDeadRef.current = false
        setBackendDead(false)
      }
    } catch (e) {
      setPollError(e instanceof Error ? e.message : String(e))
    } finally {
      refreshInFlight.current = false
    }
  }, [])

  /** Resume polling after the backend-dead banner (§7.3): clear
   *  the stop flag, reset the staleness clock, and refresh at
   *  once. The interval is still alive (its ticks no-op while
   *  stopped), so no timer needs recreating. If the backend is
   *  still dead the banner comes back after ~10 s. */
  const retryPolling = useCallback(() => {
    pollStoppedRef.current = false
    backendDeadRef.current = false
    setBackendDead(false)
    lastPollOkAt.current = Date.now()
    void refresh()
  }, [refresh])

  const refreshAll = useCallback(async () => {
    await refresh()
    await refreshQueues()
    await refreshGrabberProjects()
    await refreshQueueFiles()
  }, [refresh, refreshQueues, refreshGrabberProjects, refreshQueueFiles])

  /** Round 67 — poll the pending browser captures and queue any
   *  unseen browser/video rows for the Add URL dialog. The
   *  engine's drain leaves those rows pending (only
   *  'download_all' rows auto-add), so this is the only path
   *  that surfaces them. A failed list is silent — the poll
   *  retries next second, and a failure must not clear the
   *  queue or surface a banner. */
  const refreshPendingCaptures = useCallback(async () => {
    if (refreshCapturesInFlight.current) return
    refreshCapturesInFlight.current = true
    try {
      const rows = await listPendingLinks()
      const fresh = rows.filter(
        (r) => (r.source === 'browser' || r.source === 'video') && !seenCaptureIds.current.has(r.id),
      )
      if (fresh.length > 0) {
        for (const r of fresh) seenCaptureIds.current.add(r.id)
        setCaptureQueue((current) => [...current, ...fresh])
      }
    } catch {
      // The poll retries next second.
    } finally {
      refreshCapturesInFlight.current = false
    }
  }, [])

  useEffect(() => {
    if (!pollStoppedRef.current) void refresh()
    void refreshCategories()
    void refreshQueues()
    void refreshGrabberProjects()
    void refreshSettings()
    const timer = window.setInterval(() => {
      if (pollStoppedRef.current) return
      void refreshAll()
      // Round 67 — surface browser captures in the Add URL dialog.
      void refreshPendingCaptures()
      // Backend-dead detection (§7.3): no successful poll for
      // ~10 s — stop issuing invokes and show the banner. A poll
      // still in flight is not a dead backend (its 5 s timeout
      // has not fired yet), so the check waits for it. The
      // interval itself keeps ticking (its ticks no-op) so Retry
      // needs no timer recreation.
      if (
        !refreshInFlight.current &&
        Date.now() - lastPollOkAt.current > 10_000 &&
        !backendDeadRef.current
      ) {
        backendDeadRef.current = true
        pollStoppedRef.current = true
        setBackendDead(true)
      }
    }, 1000)
    let unlistenAction: (() => void) | undefined
    let unlistenUpdate: (() => void) | undefined
    let unlistenClipboard: (() => void) | undefined
    // NOTE (BUG-044, fixed in Round 40): there used to be a
    // `download-progress` listener here that patched rows live
    // between polls. It never fired in the built app — this app
    // shipped no Tauri capability files, so `plugin:event|listen`
    // was denied by the ACL and the subscription promise rejected
    // silently. `app/src-tauri/capabilities/default.json` now
    // grants `core:event:allow-listen` to the main window, so the
    // two subscriptions below work as written. The 1 s poll is
    // still the live path for the table (the rows carry speed,
    // ETA and scan results), and the progress window polls the
    // engine's `get_progress_snapshot` instead — the dead
    // `download-progress` listener stays removed.
    void onQueueAction((action) => {
      // Shutdown / hibernate wait for the countdown dialog; the
      // other actions have nothing to cancel, so they run at once.
      if (action.action === 'shutdown' || action.action === 'hibernate') {
        setPendingAction(action)
      } else {
        void executeQueueAction(action.queue_id)
          .then(() => void refreshAll())
          .catch((e: unknown) => setPollError(e instanceof Error ? e.message : String(e)))
      }
    }).then((fn) => {
      unlistenAction = fn
    })
    // The weekly update check found a newer version: ask the
    // user — nothing is installed without them.
    void onUpdateAvailable((result) => {
      setUpdateResult(result)
      setUpdateCheckError(null)
      setShowUpdateDialog(true)
    }).then((fn) => {
      unlistenUpdate = fn
    })
    // Round 52 — clipboard monitoring: a download link was copied.
    // While an offer is already showing, new events are ignored —
    // offers never stack on top of each other.
    void onClipboardLink((offer) => {
      setClipboardOffer((current) => current ?? offer)
    }).then((fn) => {
      unlistenClipboard = fn
    })
    // A completion action can be left waiting from a run that ended
    // before the window opened: surface its countdown now.
    void listQueues()
      .then(async (rows) => {
        for (const q of rows) {
          const pending = await getPendingAction(q.id)
          if (pending && (pending.action === 'shutdown' || pending.action === 'hibernate')) {
            setPendingAction(pending)
            break
          }
        }
      })
      .catch(() => undefined)
    return () => {
      window.clearInterval(timer)
      unlistenAction?.()
      unlistenUpdate?.()
      unlistenClipboard?.()
    }
  }, [refresh, refreshAll, refreshCategories, refreshQueues, refreshGrabberProjects, refreshSettings, refreshPendingCaptures])

  /** Round 74 (13) — a queued capture opens its own standalone
   *  Add URL window (`open_add_url_window`), popped in front
   *  over whatever window the user has open — never the
   *  in-window overlay, which stays for manual Add URL only.
   *  One capture at a time: the effect takes the next one off
   *  the queue, and that window resolves it (accepted on submit,
   *  rejected on close). A window that fails to open forgets
   *  the id, so the next poll offers the capture again. */
  useEffect(() => {
    if (captureQueue.length === 0) return
    const [next, ...rest] = captureQueue
    setCaptureQueue(rest)
    void openAddUrlWindow(next.id).catch((e: unknown) => {
      setBannerError(e instanceof Error ? e.message : String(e))
      seenCaptureIds.current.delete(next.id)
    })
  }, [captureQueue])

  /** Dark only (Round 45): the theme is a constant now, but the
   *  attribute stays set so the Tailwind dark-mode selector keeps
   *  working if a `dark:` variant is ever added. */
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
  }, [])

  /** Round 62 — status-bar free space: the drive of the default
   *  download folder, polled every 30 s (and once when the
   *  settings load). Off Windows the command rejects and the
   *  figure stays hidden. Round 74 (2) — the same refresh runs
   *  right after a delete / remove completes and after a
   *  download finishes, so the figure never sits stale until
   *  the next 30 s poll. */
  const refreshFreeSpace = useCallback(async () => {
    const folder = settings?.default_download_folder || 'C:\\'
    const drive = /^([A-Za-z]:)/.exec(folder)?.[1] ?? 'C:'
    try {
      const bytes = await getFreeDiskSpace(folder)
      setFreeSpace({ drive, bytes })
    } catch {
      setFreeSpace(null)
    }
  }, [settings?.default_download_folder])

  useEffect(() => {
    void refreshFreeSpace()
    const timer = window.setInterval(() => void refreshFreeSpace(), 30_000)
    return () => window.clearInterval(timer)
  }, [refreshFreeSpace])

  /** Manual update check (Settings > Check for updates). Runs even
   *  when the weekly setting is off; a failure shows the error
   *  in the dialog instead of a silent nothing. */
  async function runManualUpdateCheck() {
    setCheckingUpdate(true)
    setUpdateResult(null)
    setUpdateCheckError(null)
    setShowUpdateDialog(true)
    try {
      setUpdateResult(await checkForUpdates())
    } catch (e) {
      setUpdateCheckError(e instanceof Error ? e.message : String(e))
    } finally {
      setCheckingUpdate(false)
    }
  }

  /** Round 62 — the TOOLS strip's Clipboard toggle: flip the
   *  monitoring setting and refresh, so the strip's ON/OFF
   *  indicator follows the real setting. */
  async function toggleClipboardMonitoring() {
    if (!settings) return
    try {
      const updated = await updateSettings({
        ...settings,
        clipboard_monitoring: !settings.clipboard_monitoring,
      })
      setSettings(updated)
    } catch (e) {
      actionError(e)
    }
  }

  /** Remove a download. When the "ask before removing" setting is
   *  on, the confirm dialog below runs first. */
  function requestRemove(download: DownloadRow) {
    if (settings?.confirm_delete) {
      setConfirmRemove(download)
    } else {
      // Round 74 (2) — refresh the free-space figure once the
      // remove lands, not only on the 30 s poll.
      void run(() => deleteDownload(download.id, false).then(() => setSelectedId(null))).then(() =>
        void refreshFreeSpace(),
      )
    }
  }

  /** Round 65 — Redownload (right-click menu): start the same
   *  URL over from scratch IN PLACE — the engine resets the
   *  row's segments and starts it, and the row survives a failed
   *  probe as Failed with the new error. The old delete-first
   *  flow (delete the row, re-add the link, start it) lost the
   *  row entirely when the re-add probe failed on a flaky server
   *  (Round 71 retest). */
  async function handleRedownload(download: DownloadRow) {
    if (download.status === 'downloading') {
      await cancelDownload(download.id).catch(() => undefined)
    }
    await redownloadDownload(download.id)
    setSelectedId(download.id)
  }

  /** Round 73 — Redownload on a Failed/partial row discards the
   *  bytes already downloaded and starts over; Resume keeps
   *  them. Warn first so the user does not lose progress by
   *  picking the wrong item. A Finished row needs no warning —
   *  its file is on disk and nothing is lost. */
  function redownloadNeedsWarning(download: DownloadRow) {
    return (
      download.status !== 'complete' &&
      (download.status === 'error' || download.downloaded_bytes > 0)
    )
  }

  const selected = useMemo(
    () => downloads.find((d) => d.id === selectedId) ?? null,
    [downloads, selectedId],
  )
  const visible = useMemo(() => {
    let rows: DownloadRow[]
    if (selectedQueue !== null) rows = queueFiles
    else if (selectedFilter === 'unfinished') rows = downloads.filter((d) => d.status !== 'complete')
    else if (selectedFilter === 'finished') rows = downloads.filter((d) => d.status === 'complete')
    else if (selectedFilter === 'grabber') rows = downloads.filter((d) => d.grabber_project_id === selectedGrabberId)
    else if (selectedCategory === null) rows = downloads
    else rows = downloads.filter((d) => d.category_id === selectedCategory)
    const query = searchQuery.trim().toLowerCase()
    if (query) rows = rows.filter((d) => d.filename.toLowerCase().includes(query))
    return rows
  }, [downloads, selectedCategory, selectedQueue, selectedFilter, selectedGrabberId, queueFiles, searchQuery])
  /** The queue Start/Stop/Sync act on: the selected queue, else
   *  the main queue. */
  const activeQueueRow = useMemo(
    () =>
      queues.find((q) => q.id === selectedQueue) ??
      queues.find((q) => q.is_main) ??
      null,
    [queues, selectedQueue],
  )
  const totalSpeed = downloads
    .filter((d) => d.status === 'downloading')
    .reduce((sum, d) => sum + d.speed_bps, 0)
  const runningCount = downloads.filter((d) => d.status === 'downloading').length
  const hasComplete = downloads.some((d) => d.status === 'complete')

  async function run(action: () => Promise<void>, opts?: { immediate?: boolean }) {
    // Round 72 (C) — busy feedback, delayed so fast actions do
    // not flicker the status bar (see `actionBusy` above).
    // Round 73 — `immediate` shows "Working…" at once (no 400 ms
    // delay) for Redownload and Resume: on a Failed row the
    // Round 72 retest showed no feedback at all while the probe
    // retried, and the user read the silence as "nothing
    // happened".
    if (opts?.immediate) {
      setActionBusy(true)
    } else {
      actionBusyTimerRef.current = setTimeout(() => setActionBusy(true), 400)
    }
    try {
      await action()
      // A successful action clears a previous sticky error (the
      // thing it complained about worked now); the 1 s poll never
      // touches it (see `bannerError` above).
      setBannerError(null)
      await refreshAll()
    } catch (e) {
      actionError(e)
    } finally {
      if (actionBusyTimerRef.current !== null) {
        clearTimeout(actionBusyTimerRef.current)
        actionBusyTimerRef.current = null
      }
      setActionBusy(false)
    }
  }

  /** Sidebar selection: one list at a time — picking a category,
   *  a queue or a filter clears the other two. */
  function selectCategory(id: string | null) {
    setSelectedCategory(id)
    setSelectedQueue(null)
    setSelectedFilter('all')
    setSelectedGrabberId(null)
  }

  function selectQueue(id: string | null) {
    setSelectedQueue(id)
    setSelectedCategory(null)
    setSelectedFilter('all')
    setSelectedGrabberId(null)
  }

  function selectFilter(filter: SidebarFilter, grabberId: string | null = null) {
    setSelectedFilter(filter)
    setSelectedGrabberId(grabberId)
    setSelectedCategory(null)
    setSelectedQueue(null)
  }

  function openScheduler(queueId?: string) {
    setSchedulerQueueId(queueId ?? selectedQueue ?? 'queue-main')
    setShowScheduler(true)
  }

  async function handleDropDownload(
    downloadId: string,
    target: { kind: 'category' | 'queue'; id: string },
  ) {
    try {
      if (target.kind === 'category') {
        await moveToCategory(downloadId, target.id)
      } else {
        await addToQueue(downloadId, target.id)
      }
      await refreshAll()
    } catch (e) {
      actionError(e)
    }
  }

  /** Stop All (toolbar / Tasks menu): pause every download that
   *  is running right now. There is no pause-all engine command —
   *  pausing each one in turn is the same set of calls the Pause
   *  button makes, and the 1 s poll shows them all paused. */
  async function handleStopAll() {
    try {
      for (const d of downloads.filter((x) => x.status === 'downloading')) {
        await pauseDownload(d.id)
      }
      await refreshAll()
    } catch (e) {
      actionError(e)
    }
  }

  /** Delete Completed (toolbar / Tasks menu): remove every
   *  finished download from the list. The files on disk are kept,
   *  exactly like the single Remove. */
  async function handleDeleteCompleted() {
    try {
      for (const d of downloads.filter((x) => x.status === 'complete')) {
        await deleteDownload(d.id, false)
      }
      setSelectedId(null)
      await refreshAll()
      // Round 74 (2) — the free-space figure follows the removals.
      await refreshFreeSpace()
    } catch (e) {
      actionError(e)
    }
  }

  /** Open (or focus) a download's separate progress window (S03).
   *  Explicit (double-click / Enter): a failure shows in the
   *  error banner. The auto-open below calls it quiet instead, so
   *  a window that cannot open never spams the banner while a
   *  download runs. */
  async function openProgressWindowFor(id: string, quiet: boolean) {
    try {
      await openProgressWindow(id)
    } catch (e) {
      if (!quiet) actionError(e)
    }
  }

  /** S03 progress window: open a window for each download that
   *  starts, while the setting is on. The first snapshot only
   *  seeds the set — downloads already running at startup do not
   *  pop windows. A download that leaves `downloading` leaves the
   *  set, so resuming it opens its window again. Settings must
   *  have loaded first: while they are null the switch is
   *  unknown, and a window opened in that gap would ignore a
   *  user who turned it off. */
  const seenDownloading = useRef<Set<string> | null>(null)
  useEffect(() => {
    if (seenDownloading.current === null) {
      seenDownloading.current = new Set(
        downloads.filter((d) => d.status === 'downloading').map((d) => d.id),
      )
      return
    }
    if (settings === null || !settings.show_progress_window) return
    for (const d of downloads) {
      if (d.status === 'downloading' && !seenDownloading.current.has(d.id)) {
        seenDownloading.current.add(d.id)
        void openProgressWindowFor(d.id, true)
      }
    }
    for (const id of [...seenDownloading.current]) {
      if (!downloads.some((d) => d.id === id && d.status === 'downloading')) {
        seenDownloading.current.delete(id)
      }
    }
  }, [downloads, settings])

  /** Round 66 — "Download complete" dialog: when a download
   *  flips into 'complete', open its dialog (the progress
   *  window closes itself at the same moment). The first
   *  snapshot only seeds the map — downloads already complete
   *  at startup do not pop dialogs, and neither does a download
   *  first seen already complete unless it finished within the
   *  last 30 s (a tiny file can complete between two 1 s polls).
   *  Skipped when the dialog is turned off in Settings, and
   *  when the download's Options say close the program / shut
   *  the computer down — those take precedence and the progress
   *  window acts on them instead. A failure to open the dialog
   *  is silent: the dialog is a nicety, not the download, and
   *  must never spam the error banner. */
  const prevCompleteStatus = useRef<Map<string, string> | null>(null)
  const completeDialogHandled = useRef<Set<string>>(new Set())
  useEffect(() => {
    if (prevCompleteStatus.current === null) {
      prevCompleteStatus.current = new Map(downloads.map((d) => [d.id, d.status]))
      // Round 71 (D3) — seed the handled set with downloads
      // already complete at startup, so a poll refresh never
      // re-triggers the dialog for an earlier finished download
      // (the ISO completion dialog re-appeared after the fwlink
      // Start in the Round 70 retest).
      for (const d of downloads) {
        if (d.status === 'complete') completeDialogHandled.current.add(d.id)
      }
      return
    }
    if (settings === null) return
    const now = Date.now()
    for (const d of downloads) {
      const prev = prevCompleteStatus.current.get(d.id)
      prevCompleteStatus.current.set(d.id, d.status)
      if (d.status !== 'complete') continue
      if (completeDialogHandled.current.has(d.id)) continue
      const fresh =
        prev === 'downloading' ||
        prev === 'paused' ||
        (prev === undefined &&
          d.completed_at !== null &&
          now - Date.parse(d.completed_at) < 30_000)
      if (!fresh) continue
      completeDialogHandled.current.add(d.id)
      // Round 74 (2) — a finished download frees its temp space
      // and lands its file: refresh the free-space figure now.
      void refreshFreeSpace()
      if (!settings.show_download_complete_dialog) continue
      void (async () => {
        try {
          const opts = await getDownloadCompletionOptions(d.id)
          if (opts.close_after || opts.shutdown_after) return
          await openCompleteDialog(d.id)
        } catch {
          // Silent — see the comment above.
        }
      })()
    }
    for (const id of [...prevCompleteStatus.current.keys()]) {
      if (!downloads.some((d) => d.id === id)) prevCompleteStatus.current.delete(id)
    }
  }, [downloads, settings, refreshFreeSpace])

  /** Round 68 — "Download failed" dialog: when a download
   *  flips into 'error', open its dialog over the progress
   *  window (the progress window stays open behind it — the
   *  dialog's OK returns there, and Resume continues from where
   *  the download stopped). Same seeding rule as the complete
   *  dialog, stricter: only a flip from a previously-seen
   *  downloading / paused / queued status fires — a download
   *  first seen already in error (an old failure at startup)
   *  never pops a dialog. Deduped per id; a failure to open the
   *  dialog is silent, like the complete dialog. */
  const prevErrorStatus = useRef<Map<string, string> | null>(null)
  const errorDialogHandled = useRef<Set<string>>(new Set())
  useEffect(() => {
    if (prevErrorStatus.current === null) {
      prevErrorStatus.current = new Map(downloads.map((d) => [d.id, d.status]))
      return
    }
    for (const d of downloads) {
      const prev = prevErrorStatus.current.get(d.id)
      prevErrorStatus.current.set(d.id, d.status)
      if (d.status !== 'error') continue
      if (errorDialogHandled.current.has(d.id)) continue
      const fresh = prev === 'downloading' || prev === 'paused' || prev === 'queued'
      if (!fresh) continue
      errorDialogHandled.current.add(d.id)
      void (async () => {
        try {
          await openErrorDialog(d.id)
        } catch {
          // Silent — see the complete-dialog comment above.
        }
      })()
    }
    for (const id of [...prevErrorStatus.current.keys()]) {
      if (!downloads.some((d) => d.id === id)) prevErrorStatus.current.delete(id)
    }
  }, [downloads])

  /** Round 68 — taskbar progress: the overall percent of the
   *  active downloads on the main window's taskbar button, with
   *  the file / speed / time-left summary in the window title
   *  (the taskbar tooltip). Aggregate = bytes downloaded over
   *  bytes total across the downloading rows with a known size;
   *  one active download is its own percent. All-paused shows
   *  the paused state at the same aggregate; nothing active
   *  clears the bar and restores the plain title. The command
   *  is only invoked when the rounded percent or the summary
   *  text actually changes — never more than once per refresh
   *  poll. */
  const lastTaskbar = useRef<string | null>(null)
  useEffect(() => {
    const active = downloads.filter((d) => d.status === 'downloading')
    const paused = downloads.filter((d) => d.status === 'paused')
    const known = active.filter((d) => d.size_bytes !== null && d.size_bytes > 0)
    if (active.length === 0 && paused.length === 0) {
      if (lastTaskbar.current !== 'clear') {
        lastTaskbar.current = 'clear'
        void setTaskbarProgress(null, null, 'Sonic Manager').catch(() => undefined)
      }
      return
    }
    const rows = active.length > 0 ? known : paused.filter((d) => d.size_bytes !== null && d.size_bytes > 0)
    const total = rows.reduce((sum, d) => sum + (d.size_bytes ?? 0), 0)
    const done = rows.reduce((sum, d) => sum + Math.min(d.downloaded_bytes, d.size_bytes ?? 0), 0)
    const percent = total > 0 ? Math.round((done / total) * 100) : 0
    const status = active.length > 0 ? 'normal' : 'paused'
    const speed = active.reduce((sum, d) => sum + d.speed_bps, 0)
    const eta = active
      .map((d) => d.eta_seconds)
      .filter((e): e is number => e !== null)
    const summary =
      active.length === 1
        ? `${percent}% · ${active[0].filename} · ${formatSpeed(speed)}${eta.length > 0 ? ` · ${formatEta(Math.min(...eta))}` : ''}`
        : `${percent}% · ${active.length > 0 ? active.length : paused.length} downloads · ${formatSpeed(speed)}`
    const title = `Sonic Manager — ${summary}`
    const key = `${status}:${percent}:${title}`
    if (lastTaskbar.current !== key) {
      lastTaskbar.current = key
      void setTaskbarProgress(percent, status, title).catch(() => undefined)
    }
  }, [downloads])

  /** Round 74 (4) — check the `file_missing` flag before any
   *  open: a Finished row whose file was deleted from Windows
   *  warns instead of invoking open-file / open-with /
   *  open-folder on a deleted location. */
  function openGuarded(id: string, open: (id: string) => Promise<void>) {
    const row = downloads.find((d) => d.id === id)
    if (row?.status === 'complete' && row.file_missing) {
      setBannerError(`“${row.filename}” is no longer on disk — Redownload it to get it again.`)
      return
    }
    void run(() => open(id))
  }

  const toolbarActions = {
    add: () => setShowAdd(true),
    resume: () => selected && void run(() => resumeDownload(selected.id), { immediate: true }),
    stop: () => selected && void run(() => pauseDownload(selected.id)),
    deleteSelected: () => selected && requestRemove(selected),
  }

  const menuActions: MenuBarActions = {
    add: () => setShowAdd(true),
    downloadAll: () => setShowDownloadAll(true),
    batch: () => setShowBatch(true),
    resume: () => selected && void run(() => resumeDownload(selected.id), { immediate: true }),
    stop: () => selected && void run(() => pauseDownload(selected.id)),
    stopAll: () => void handleStopAll(),
    deleteSelected: () => selected && requestRemove(selected),
    deleteCompleted: () => void handleDeleteCompleted(),
    showHistory: () => selectFilter('history'),
    refresh: () => void refreshAll(),
    startQueue: () => activeQueueRow && void run(() => startQueue(activeQueueRow.id)),
    stopQueue: () => activeQueueRow && void run(() => stopQueue(activeQueueRow.id)),
    scheduler: () => openScheduler(),
    videoGrabber: () => setShowVideoGrabber(true),
    siteGrabber: () => setShowSiteGrabber(true),
    settings: () => setShowSettings(true),
    checkUpdates: () => void runManualUpdateCheck(),
    // Round 74 (10) — File > Exit quits the whole app via the
    // `exit_app` command (the tray Exit already did); it used to
    // just close the main window, which hides to the tray.
    exit: () => void exitApp().catch((e: unknown) => actionError(e)),
  }

  return (
    <div className="flex h-full flex-col text-text">
      <TitleBar title="Sonic Manager" hideOnClose />
      <MenuBar
        actions={menuActions}
        selected={selected}
        queue={activeQueueRow}
        hasDownloading={runningCount > 0}
        hasComplete={hasComplete}
      />
      <Toolbar
        actions={toolbarActions}
        selected={selected}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
      />
      <ToolsStrip
        onQueue={() => {
          if (activeQueueRow) selectQueue(activeQueueRow.id)
          else setShowAddQueue(true)
        }}
        onScheduler={() => openScheduler()}
        onVideoGrabber={() => setShowVideoGrabber(true)}
        onSiteGrabber={() => setShowSiteGrabber(true)}
        clipboardOn={settings?.clipboard_monitoring ?? false}
        onToggleClipboard={() => void toggleClipboardMonitoring()}
      />

      {(() => {
        // Round 55: the sticky action error wins over the transient
        // poll error; dismissing clears both.
        const banner = bannerError ?? error
        return banner ? (
          <div role="alert" className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-danger-soft px-4 py-2 text-sm text-danger">
            <span>{banner}</span>
            <button
              type="button"
              onClick={() => {
                // Round 71 (D2) — remember the dismissed message
                // so the next poll cannot re-populate the banner
                // with it; see `dismissedBannerRef` above.
                dismissedBannerRef.current = banner
                setBannerError(null)
                setError(null)
              }}
              aria-label="Dismiss error"
              className="rounded-md p-1 hover:bg-surface-2"
              style={{ WebkitAppRegion: 'no-drag' } as CSSProperties}
            >
              <X className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
        ) : null
      })()}

      {backendDead ? (
        <div role="alert" className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-danger-soft px-4 py-2 text-sm text-danger">
          <span>Backend not responding — restart the app. Your downloads are safe on disk.</span>
          <span className="flex shrink-0 items-center gap-2">
            <Button variant="secondary" size="sm" onClick={retryPolling}>
              Retry connection
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                void reloadWebviewWindow('main').catch((e: unknown) =>
                  setBannerError(e instanceof Error ? e.message : String(e)),
                )
              }}
            >
              Reload window
            </Button>
          </span>
        </div>
      ) : null}

      <div className="flex min-h-0 flex-1 gap-2 p-2">
        <CategorySidebar
          categories={categories}
          downloads={downloads}
          queues={queues}
          grabberProjects={grabberProjects}
          selectedCategory={selectedCategory}
          selectedQueue={selectedQueue}
          selectedFilter={selectedFilter}
          selectedGrabberId={selectedGrabberId}
          onSelectCategory={selectCategory}
          onSelectQueue={selectQueue}
          onSelectFilter={selectFilter}
          onAddCategory={() => setShowAddCategory(true)}
          onAddQueue={() => setShowAddQueue(true)}
          onDeleteCategory={(category) =>
            void run(() => deleteCategory(category.id).then(() => void refreshCategories()))
          }
          onDeleteQueue={(queue) =>
            void run(() =>
              deleteQueue(queue.id).then(() => {
                if (selectedQueue === queue.id) selectFilter('all')
              }),
            )
          }
          onRenameQueue={(queue, name) =>
            void run(() => renameQueue(queue.id, name).then(() => undefined))
          }
          onOpenScheduler={(queueId) => openScheduler(queueId)}
          onRunGrabber={(projectId) => void run(() => runGrabber(projectId))}
          onNewGrabber={() => setShowSiteGrabber(true)}
          onSyncQueue={(queueId) => void run(() => syncQueueNow(queueId).then(() => undefined))}
          onDropDownload={(downloadId, target) => void handleDropDownload(downloadId, target)}
        />
        {/* Round 47: the main area is a rounded panel with a thin
            glowing border, as in the reference — inset from the
            toolbar/footer by the row's gap/padding, like the
            sidebar panel. Round 63: the neon-waves backdrop is
            gone (flat dark #0B0F24 now), so the translucent
            surface just reads as a slightly lighter dark; the
            rows stay readable (the thead is opaque). */}
        <main className="min-w-0 flex-1 overflow-auto rounded-xl border border-border bg-surface/40 shadow-panel">
          {selectedFilter === 'history' ? (
            <HistoryView
              onRedownloaded={() => {
                // The new download lives in the downloads view —
                // go back there so it is visible at once.
                selectFilter('all')
                void refreshAll()
              }}
              onError={(message) => setBannerError(message)}
            />
          ) : (
            <DownloadTable
              downloads={visible}
              categories={categories}
              queues={queues}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onOpenProgress={(id) => void openProgressWindowFor(id, false)}
              onAdd={() => setShowAdd(true)}
              onOpenFile={(id) => openGuarded(id, openFile)}
              onOpenWith={(id) => openGuarded(id, openFileWith)}
              onOpenFolder={(id) => openGuarded(id, openFolder)}
              onRedownload={(download) => {
                if (redownloadNeedsWarning(download)) {
                  setConfirmRedownload(download)
                } else {
                  void run(() => handleRedownload(download), { immediate: true })
                }
              }}
              onResume={(id) => void run(() => resumeDownload(id), { immediate: true })}
              onPause={(id) => void run(() => pauseDownload(id))}
              onMoveToCategory={(id, categoryId) => void run(() => moveToCategory(id, categoryId))}
              onRefreshAddress={(download) => setRefreshTarget(download)}
              onRemove={(download) => requestRemove(download)}
              onMoveToQueue={(id, queueId) => void run(() => addToQueue(id, queueId))}
              onDeleteFromQueue={(id) => void run(() => removeFromQueue(id))}
              onShowProperties={(download) => setPropertiesTarget(download)}
            />
          )}
        </main>
      </div>

      <footer className="flex h-7 shrink-0 items-center justify-between border-t border-border bg-surface px-4 text-xs text-text-muted">
        <span>
          {actionBusy ? (
            'Working…'
          ) : (
            <>
              {runningCount} downloading · {runningCount > 0 ? formatSpeed(totalSpeed) : '—'} ·{' '}
              {downloads.filter((d) => d.status === 'complete').length} finished
            </>
          )}
        </span>
        <span className="flex items-center gap-3">
          {quotaStatus?.enabled && quotaStatus.limit_mb != null ? (
            <button
              type="button"
              onClick={() => setShowSettings(true)}
              title="Open settings — download quota"
              className="rounded-md px-1 py-0.5 hover:bg-surface-2 hover:text-text"
            >
              {quotaStatus.reached
                ? `Quota reached — resumes ${quotaStatus.window_end_unix ? new Date(quotaStatus.window_end_unix * 1000).toLocaleTimeString() : 'when the window ends'}`
                : `Quota ${(quotaStatus.used_bytes / 1_048_576).toFixed(1)}/${quotaStatus.limit_mb} MB`}
            </button>
          ) : null}
          {freeSpace ? (
            <span>
              {freeSpace.drive} {formatBytes(freeSpace.bytes)} free
            </span>
          ) : null}
        </span>
      </footer>

      <AddDownloadDialog
        open={showAdd}
        categories={categories}
        defaultAction={settings?.default_action ?? 'start_now'}
        defaultFolder={settings?.default_download_folder ?? ''}
        initialUrl={clipboardCapture?.url ?? pickedVideoUrl}
        captureMode={clipboardCapture !== null}
        captureTitle={clipboardCapture?.filename ?? null}
        onClose={() => {
          // Round 74 (12) — a clipboard capture closed without a
          // submit is dismissed, so the engine does not offer the
          // same URL again. (Extension captures live in their own
          // standalone window now and resolve themselves.)
          const capture = clipboardCapture
          if (capture) {
            setClipboardCapture(null)
            if (!clipboardAddedRef.current) {
              void dismissClipboardOffer(capture.url).catch(() => undefined)
            }
          }
          setShowAdd(false)
          setPickedVideoUrl('')
        }}
        onAdded={(id) => {
          // Round 74 (12) — a submitted clipboard capture must
          // not be dismissed by `onClose` afterwards.
          if (clipboardCapture) {
            clipboardAddedRef.current = true
          }
          // Round 55 (defect 3a): the new row must be visible — a
          // download auto-assigned to another category used to be
          // hidden by the sidebar filter and look like a silent
          // failure. Go to "All downloads" and select the row, so
          // it is in view and the toolbar acts on it.
          selectFilter('all')
          setSelectedId(id)
          void refreshAll()
        }}
        onLateError={(message) => setBannerError(message)}
        onSubmitError={(message, filename) =>
          setAddError({ title: filename || 'Download failed', message })
        }
        onAddCategory={() => setShowAddCategory(true)}
      />
      {addError ? (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Download failed"
          className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60 p-4"
        >
          <div className="h-[360px] w-[520px] max-w-full overflow-hidden rounded-lg border border-border bg-bg shadow-pop">
            <DownloadErrorDialog
              message={addError.message}
              title={addError.title}
              onClose={() => setAddError(null)}
            />
          </div>
        </div>
      ) : null}
      <RefreshAddressDialog
        open={refreshTarget !== null}
        download={refreshTarget}
        onClose={() => setRefreshTarget(null)}
        onRefreshed={() => {
          setRefreshTarget(null)
          void refreshAll()
        }}
      />
      <DownloadPropertiesDialog
        open={propertiesTarget !== null}
        download={propertiesTarget}
        categories={categories}
        queues={queues}
        onClose={() => setPropertiesTarget(null)}
      />
      {clipboardOffer ? (
        <ClipboardOfferDialog
          offer={clipboardOffer}
          onDownload={() => {
            // Round 74 (12) — accepting the offer opens the SAME
            // Add URL window in capture mode (file name as the
            // title, URL locked) so the user confirms folder /
            // name / category first. The old path added the row
            // and started it at once, with no chance to confirm
            // anything — and a start failure left a stuck
            // "Waiting" row (Round 68).
            clipboardAddedRef.current = false
            setClipboardCapture(clipboardOffer)
            setClipboardOffer(null)
            setShowAdd(true)
          }}
          onDismiss={() => {
            const url = clipboardOffer.url
            void dismissClipboardOffer(url).catch(() => undefined)
            setClipboardOffer(null)
          }}
        />
      ) : null}
      <VideoGrabberDialog
        open={showVideoGrabber}
        queues={queues}
        onClose={() => setShowVideoGrabber(false)}
        onPick={(url) => {
          setPickedVideoUrl(url)
          setShowVideoGrabber(false)
          setShowAdd(true)
        }}
        onAdded={() => void refreshAll()}
      />
      <SiteGrabberDialog
        open={showSiteGrabber}
        queues={queues}
        onClose={() => setShowSiteGrabber(false)}
        onAdded={() => void refreshAll()}
      />
      <DownloadAllDialog
        open={showDownloadAll}
        queues={queues}
        onClose={() => setShowDownloadAll(false)}
        onAdded={() => void refreshAll()}
      />
      <BatchDialog
        open={showBatch}
        queues={queues}
        onClose={() => setShowBatch(false)}
        onAdded={() => void refreshAll()}
      />
      <AddCategoryDialog
        open={showAddCategory}
        onClose={() => setShowAddCategory(false)}
        onAdded={() => void refreshCategories()}
      />
      <AddQueueDialog
        open={showAddQueue}
        onClose={() => setShowAddQueue(false)}
        onAdded={(id) => {
          void refreshQueues()
          selectQueue(id)
        }}
      />
      <SchedulerDialog
        open={showScheduler}
        queues={queues}
        initialQueueId={schedulerQueueId}
        onClose={() => setShowScheduler(false)}
        onChanged={() => void refreshAll()}
        onNewQueue={() => setShowAddQueue(true)}
      />
      <SettingsDialog
        open={showSettings}
        categories={categories}
        onClose={() => setShowSettings(false)}
        onCheckUpdates={() => void runManualUpdateCheck()}
        onSaved={() => {
          void refreshSettings()
          void refreshCategories()
        }}
      />
      {showUpdateDialog && (
        <UpdateDialog
          result={updateResult}
          checkError={updateCheckError}
          checking={checkingUpdate}
          onClose={() => setShowUpdateDialog(false)}
        />
      )}
      {confirmRemove ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="confirm-remove-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
        >
          <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <div className="flex items-start justify-between gap-4">
              <h2 id="confirm-remove-title" className="text-lg font-semibold">
                Remove this download?
              </h2>
              <button
                type="button"
                onClick={() => setConfirmRemove(null)}
                aria-label="Close"
                className="rounded-md p-1 text-text-muted transition-colors hover:bg-surface-2 hover:text-text"
              >
                <X className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
            <p className="mt-2 text-sm text-text-muted">
              “{confirmRemove.filename}” will be removed from the list. The file on disk is kept.
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setConfirmRemove(null)}>
                Keep it
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  const row = confirmRemove
                  setConfirmRemove(null)
                  void run(() => deleteDownload(row.id, false).then(() => setSelectedId(null))).then(
                    () => void refreshFreeSpace(),
                  )
                }}
              >
                Remove
              </Button>
            </div>
          </div>
        </div>
      ) : null}
      {confirmRedownload ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="confirm-redownload-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
        >
          <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <div className="flex items-start justify-between gap-4">
              <h2 id="confirm-redownload-title" className="text-lg font-semibold">
                Redownload from scratch?
              </h2>
              <button
                type="button"
                onClick={() => setConfirmRedownload(null)}
                aria-label="Close"
                className="rounded-md p-1 text-text-muted transition-colors hover:bg-surface-2 hover:text-text"
              >
                <X className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
            <p className="mt-2 text-sm text-text-muted">
              “{confirmRedownload.filename}” starts over; Resume keeps your progress.
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setConfirmRedownload(null)}>
                Keep it
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  const row = confirmRedownload
                  setConfirmRedownload(null)
                  void run(() => handleRedownload(row), { immediate: true })
                }}
              >
                Redownload
              </Button>
            </div>
          </div>
        </div>
      ) : null}
      <QueueActionDialog
        action={pendingAction}
        onDone={() => {
          setPendingAction(null)
          void refreshAll()
        }}
      />
    </div>
  )
}
