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
  executeQueueAction,
  exitApp,
  getCategories,
  getDownloadCompletionOptions,
  getFreeDiskSpace,
  getPendingAction,
  getQueueFiles,
  getQuotaStatus,
  getSettings,
  enqueueClipboardCapture,
  listDownloads,
  listGrabberProjects,
  listPendingLinks,
  listQueues,
  moveToCategory,
  onClipboardLink,
  onQueueAction,
  onUpdateAvailable,
  openAddUrlWindow,
  openManualAddUrlWindow,
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
  renameGrabberProject,
  deleteGrabberProject,
  renameQueue,
  resumeDownload,
  runGrabber,
  setSettingField,
  setTaskbarProgress,
  startQueue,
  stopQueue,
  syncQueueNow,
  withTimeout,
} from './lib/api'
import { formatBytes, formatEta, formatSpeed } from './lib/format'
import { BannerDismissals, FailedCaptures, type BannerError } from './lib/bannerErrors'
import { selectCompletedDownloads, selectVisibleRows, useSidebarSelection } from './lib/useSidebarSelection'
import type { CategoryRow, ClipboardOffer, DownloadRow, GrabberProjectRow, PendingAction, PendingLinkRow, QueueRow, QuotaStatus, Settings, UpdateCheckResult } from './lib/types'
import { AddCategoryDialog } from './components/AddCategoryDialog'
import { AddDownloadDialog } from './components/AddDownloadDialog'
import { AddQueueDialog } from './components/AddQueueDialog'
import { BatchDialog } from './components/BatchDialog'
import { Button } from './components/Button'
import { CategorySidebar } from './components/CategorySidebar'
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
  /** Round 75 — sidebar selection lives in useSidebarSelection
   *  (one list at a time: category / queue / filter / grabber),
   *  shared with the sidebar test so the test drives the real
   *  selection functions. */
  const {
    selectedCategory,
    selectedQueue,
    /** Which list the table shows: every download (or one
     *  category's), the unfinished / finished filter, the download
     *  history (Round 53 — the main area renders HistoryView
     *  instead of the table), or one grabber project's downloads. */
    selectedFilter,
    selectedGrabberId,
    selectCategory,
    selectQueue,
    selectFilter,
  } = useSidebarSelection()
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
  /** Round 75 — the capture the Add URL dialog is open for (mirrors
   *  `clipboardCapture` for the clipboard-link listener, whose
   *  closure cannot read the current state): while one is open,
   *  new offers wait — one at a time, and never a second window
   *  for the same URL. */
  const clipboardCaptureRef = useRef<ClipboardOffer | null>(null)
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
  const [error, setError] = useState<BannerError | null>(null)
  /** Round 55 (defect 4): action errors are STICKY — the 1 s poll
   *  clears `error` on every successful refresh, so a Resume /
   *  Pause failure used to flash for under a second and vanish,
   *  looking like a silent no-op. `bannerError` is only cleared by
   *  the dismiss button or by a later successful action, never by
   *  the poll. The banner shows it first, else the transient poll
   *  error. Both carry their source (Round 75), so dismissals are
   *  keyed per (source, message). */
  const [bannerError, setBannerError] = useState<BannerError | null>(null)
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
  /** Round 71 (D2), reworked Round 75 — the banner's X must
   *  STICK. The 1 s poll re-ran `list_downloads` (and the
   *  secondary polls) and every failure re-populated `error`
   *  from the same old error rows, so the banner came back a
   *  second after the user dismissed it. `dismissedBannerRef`
   *  remembers the exact (source, message) the user dismissed;
   *  `setPollError` — used by every poll-failure path — refuses
   *  to re-show it. A NEW error (a different message, or the
   *  same message from another source) still shows, and a
   *  successful poll clears ONLY its own source's memory, so a
   *  non-poll error (e.g. a capture auto-open failure) is never
   *  un-dismissed by an unrelated successful poll. See
   *  `lib/bannerErrors.ts`. */
  const dismissedBannerRef = useRef(new BannerDismissals())
  /** Round 75 — a capture whose auto-open failed. Its id stays
   *  here until the user clicks Retry or a NEW pending id
   *  arrives; the effect never retries it on its own (the old
   *  code forgot the id, so the next poll re-queued it and the
   *  red banner looped every second). */
  const failedCaptureIds = useRef(new FailedCaptures())
  /** Round 75 — the capture waiting on a retry, for the
   *  banner's Retry button ("Capture failed — Open"). Null when
   *  no capture is waiting. */
  const [failedCapture, setFailedCapture] = useState<PendingLinkRow | null>(null)
  const setPollError = useCallback((source: string, msg: string) => {
    if (!dismissedBannerRef.current.shouldShow(source, msg)) return
    setError({ source, message: msg })
  }, [])
  /** A sticky (non-poll) error: shown unless the user dismissed
   *  exactly this (source, message). */
  const showBannerError = useCallback((source: string, msg: string) => {
    if (!dismissedBannerRef.current.shouldShow(source, msg)) return
    setBannerError({ source, message: msg })
  }, [])
  /** A successful poll of `source` forgets that source's
   *  dismissal and clears a poll error from that source — other
   *  sources are untouched. */
  const clearPollErrorForSource = useCallback((source: string) => {
    dismissedBannerRef.current.clearForSource(source)
    setError((prev) => (prev && prev.source === source ? null : prev))
  }, [])
  /** Round 69 (d) — an Add URL submit failure, shown in the
   *  standalone error dialog (IDM-style, with Details) as an
   *  overlay above the Add dialog. */
  const [addError, setAddError] = useState<{ title: string; message: string } | null>(null)
  function actionError(e: unknown) {
    showBannerError('action', e instanceof Error ? e.message : String(e))
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
  /** Round 87 (13b) — the remove confirm holds a LIST now: one
   *  row for a single Remove, every selected row for a
   *  multi-select Delete. The dialog says how many it removes. */
  const [confirmRemove, setConfirmRemove] = useState<DownloadRow[] | null>(null)
  /** Round 87 (13b) — the table's multi-selection, mirrored
   *  here via `onSelectionChange` so the toolbar / menu Delete
   *  removes ALL selected rows, not just the anchor. */
  const [selectedIds, setSelectedIds] = useState<string[]>([])
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
      clearPollErrorForSource('poll:queues')
    } catch (e) {
      setPollError('poll:queues', e instanceof Error ? e.message : String(e))
    } finally {
      secondaryRefreshInFlight.current = false
    }
  }, [clearPollErrorForSource])

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
      clearPollErrorForSource('poll:queue-files')
    } catch (e) {
      setPollError('poll:queue-files', e instanceof Error ? e.message : String(e))
    } finally {
      secondaryRefreshInFlight.current = false
    }
  }, [selectedQueue, clearPollErrorForSource])

  const refreshCategories = useCallback(async () => {
    try {
      setCategories(await getCategories())
      clearPollErrorForSource('poll:categories')
    } catch (e) {
      setPollError('poll:categories', e instanceof Error ? e.message : String(e))
    }
  }, [clearPollErrorForSource])

  const refresh = useCallback(async () => {
    // In-flight guard (§7.3): skip the tick while a refresh is
    // still running — a hung invoke must not pile up a new one
    // every second.
    if (refreshInFlight.current) return
    refreshInFlight.current = true
    try {
      setDownloads(await withTimeout(listDownloads(), 5000, 'Backend did not respond within 5 seconds'))
      // Round 75 — a successful poll clears ONLY its own
      // source's dismissal: the old failure is gone, so a later
      // failure with the same text is a new failure and must
      // show again — but a dismissed non-poll error (e.g. a
      // capture failure) is never un-dismissed by this poll.
      clearPollErrorForSource('poll:downloads')
      lastPollOkAt.current = Date.now()
      if (backendDeadRef.current) {
        backendDeadRef.current = false
        setBackendDead(false)
      }
    } catch (e) {
      setPollError('poll:downloads', e instanceof Error ? e.message : String(e))
    } finally {
      refreshInFlight.current = false
    }
  }, [clearPollErrorForSource])

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
          .catch((e: unknown) => setPollError('queue-action', e instanceof Error ? e.message : String(e)))
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
    // Round 52, reworked Round 75 and Round 81 — clipboard
    // monitoring: a download link was copied. Round 81: the
    // offer is enqueued as a pending capture, so it opens in
    // its own standalone Add URL window, popped in front over
    // whatever the user has open — the main window's 1 s
    // pending-captures poll picks the new row up. Until now the
    // offer opened the Add URL dialog directly inside the MAIN
    // window, and when that window was hidden to the tray the
    // user saw nothing on top and had to "go into the program"
    // to find it. A fresh copy re-offers (new clipboard
    // sequence), so nothing is lost by dismissing; the
    // standalone window resolves the capture itself on close.
    // Only when the enqueue itself fails (a real refusal) does
    // the offer fall back to the in-window dialog, so it is
    // never lost.
    void onClipboardLink((offer) => {
      void enqueueClipboardCapture(offer.url).catch(() => {
        if (clipboardCaptureRef.current !== null) return
        clipboardCaptureRef.current = offer
        setClipboardCapture(offer)
        setShowAdd(true)
      })
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

  /** Round 74 (13), reworked Round 75 — a queued capture opens
   *  its own standalone Add URL window (`open_add_url_window`),
   *  popped in front over whatever window the user has open —
   *  never the in-window overlay: since Round 87.2 manual Add
   *  URL opens the same standalone window, and the overlay
   *  stays only for the clipboard-capture fallback and the
   *  video-grabber pick. One capture at a time: the effect takes the next one
   *  off the queue, and that window resolves it (accepted on
   *  submit, rejected on close). A window that FAILS to open is
   *  remembered in `failedCaptureIds` and is NOT retried — the
   *  old code forgot the id, so the next poll re-queued it and
   *  the red banner looped every second. It is offered again
   *  only when the user clicks Retry on the banner, or when a
   *  NEW pending id arrives for the same link. A failure never
   *  blocks the other queued captures. */
  useEffect(() => {
    if (captureQueue.length === 0) return
    const [next, ...rest] = captureQueue
    setCaptureQueue(rest)
    if (failedCaptureIds.current.has(next.id)) return
    void openAddUrlWindow(next.id).then(() => {
      // The window opened: a previous sticky capture error for
      // this retry is spent.
      setBannerError((prev) => (prev && prev.source === 'capture' ? null : prev))
    }).catch((e: unknown) => {
      failedCaptureIds.current.markFailed(next.id)
      setFailedCapture(next)
      showBannerError('capture', e instanceof Error ? e.message : String(e))
    })
  }, [captureQueue, showBannerError])

  /** Dark only (Round 45): the theme is a constant now, but the
   *  attribute stays set so the Tailwind dark-mode selector keeps
   *  working if a `dark:` variant is ever added. */
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
  }, [])

  /** Round 62 — status-bar free space: the drive of the default
   *  download folder, polled every 10 s (Round 76 (5): 30 s was
   *  slow enough for the user to notice the figure lagging after
   *  a cancel) and once when the settings load. Off Windows the
   *  command rejects and the figure stays hidden. Round 74 (2) —
   *  the same refresh runs right after a delete / remove
   *  completes and after a download finishes, so the figure
   *  never sits stale until the next poll. */
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
    const timer = window.setInterval(() => void refreshFreeSpace(), 10_000)
    return () => window.clearInterval(timer)
  }, [refreshFreeSpace])

  /** Round 76 (5) — the main window has no direct cancel path
   *  (cancelling happens in the progress window, which deletes
   *  the partial file), so refresh the free-space figure when
   *  the download list itself changes shape — a row vanishing
   *  or changing status (cancel / finish / remove / delete
   *  completed). The signature ignores byte progress, so the
   *  1 s poll does not spam the drive with a refresh per tick. */
  const downloadSignatureRef = useRef<string | null>(null)
  useEffect(() => {
    const signature = downloads.map((d) => `${d.id}:${d.status}`).join(',')
    if (downloadSignatureRef.current === null) {
      downloadSignatureRef.current = signature
      return
    }
    if (downloadSignatureRef.current !== signature) {
      downloadSignatureRef.current = signature
      void refreshFreeSpace()
    }
  }, [downloads, refreshFreeSpace])

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
   *  indicator follows the real setting.
   *
   *  Round 87.3 — re-read the settings fresh first: this
   *  window's `settings` is fetched once at mount, so spreading
   *  it would write every other field back as it was at mount
   *  — including a `show_download_complete_dialog` the user
   *  just turned ON in Settings (which saves at once now and
   *  does not refresh this window until its next save). The
   *  fresh read is authoritative, and the flip reads from it
   *  too.
   *
   *  Round 87.5 — the write goes through `setSettingField`
   *  (api.ts): the one settings write model — fresh re-read +
   *  single-field update — shared with every other writer. */
  async function toggleClipboardMonitoring() {
    if (!settings) return
    try {
      const fresh = await getSettings()
      const updated = await setSettingField(
        'clipboard_monitoring',
        !fresh.clipboard_monitoring,
      )
      setSettings(updated)
    } catch (e) {
      actionError(e)
    }
  }

  /** Remove a download. When the "ask before removing" setting is
   *  on, the confirm dialog below runs first. */
  function requestRemove(download: DownloadRow) {
    requestRemoveMany([download])
  }

  /** Round 87 (13b) — remove every selected row at once. The
   *  confirm (when on) lists the count; the files on disk are
   *  kept, as for a single remove. */
  function requestRemoveMany(rows: DownloadRow[]) {
    const targets = rows.filter((row) => downloads.some((d) => d.id === row.id))
    if (targets.length === 0) return
    if (settings?.confirm_delete) {
      setConfirmRemove(targets)
    } else {
      // Round 74 (2) — refresh the free-space figure once the
      // remove lands, not only on the 30 s poll.
      void run(async () => {
        for (const row of targets) {
          await deleteDownload(row.id, false)
        }
        setSelectedId(null)
        setSelectedIds([])
      }).then(() => void refreshFreeSpace())
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
  const visible = useMemo(
    () =>
      selectVisibleRows({
        downloads,
        queueFiles,
        selectedCategory,
        selectedQueue,
        selectedFilter,
        selectedGrabberId,
        searchQuery,
      }),
    [downloads, selectedCategory, selectedQueue, selectedFilter, selectedGrabberId, queueFiles, searchQuery],
  )
  /** The queue Start/Stop/Sync act on: the selected queue.
   *  Round 87 (13c) — NO main-queue fallback: the main queue is
   *  hidden from the sidebar, so with no queue selected there is
   *  no queue to act on (the TOOLS strip opens Add Queue). */
  const activeQueueRow = useMemo(
    () => queues.find((q) => q.id === selectedQueue) ?? null,
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

  /** Sidebar selection: one list at a time — the handlers live in
   *  useSidebarSelection (Round 75) and clear the other selections
   *  themselves, so the sidebar calls exactly one per click. */
  function openScheduler(queueId?: string) {
    // Round 87 (13c) — no 'queue-main' default: the main queue
    // is hidden, so the scheduler opens on the selected queue,
    // else the first visible queue, else none (the dialog then
    // says "No queues yet — create one to begin").
    setSchedulerQueueId(
      queueId ?? selectedQueue ?? queues.find((q) => !q.is_main)?.id ?? null,
    )
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
   *  exactly like the single Remove. Round 76 (6) — the filter
   *  is `selectCompletedDownloads` (status 'complete' only, the
   *  whole list, not just the visible view): no confirmation,
   *  it executes directly, per the user's decision. */
  async function handleDeleteCompleted() {
    try {
      for (const d of selectCompletedDownloads(downloads)) {
        await deleteDownload(d.id, false)
      }
      setSelectedId(null)
      setSelectedIds([])
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
      void (async () => {
        try {
          // Round 87 (9) — read the setting FRESH here, not from
          // this window's `settings` state: that state is fetched
          // once at mount, so a "Don't show this dialog again"
          // checked in a dialog window (a different OS window
          // writing the same Settings store) never reached it
          // and the next completion still popped a dialog. The
          // backend also gates in `open_complete_dialog` — this
          // read keeps the frontend from even asking.
          const fresh = await getSettings()
          if (!fresh.show_download_complete_dialog) return
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
      showBannerError('action', `“${row.filename}” is no longer on disk — Redownload it to get it again.`)
      return
    }
    void run(() => open(id))
  }

  /** Round 87 (13b) — toolbar / menu Delete removes ALL
   *  selected rows; with no multi-selection it removes the
   *  anchor row, as before. */
  function deleteSelected() {
    const rows = downloads.filter((d) => selectedIds.includes(d.id))
    if (rows.length > 0) requestRemoveMany(rows)
    else if (selected) requestRemove(selected)
  }

  /** Round 87.1 — the table mirrors its multi-selection
   *  here; when it empties (Ctrl+click toggled the last row
   *  OFF) the anchor goes with it, so toolbar / menu Delete
   *  has nothing to act on instead of deleting the stale
   *  anchor row. */
  function handleSelectionChange(ids: string[]) {
    setSelectedIds(ids)
    if (ids.length === 0) setSelectedId(null)
  }

  /** Round 87.2 — manual Add URL opens the standalone window
   *  (the in-window overlay's header has no TitleBar / drag
   *  region, so it could never be dragged — the R87/R87.1
   *  failure). On failure the banner explains instead of
   *  silently doing nothing. The in-window dialog stays for
   *  the clipboard-capture fallback and the video-grabber
   *  pick, which arrive as offers inside the main window. */
  function openManualAddUrl() {
    void openManualAddUrlWindow().catch((e: unknown) =>
      showBannerError('action', e instanceof Error ? e.message : String(e)),
    )
  }

  const toolbarActions = {
    add: () => openManualAddUrl(),
    resume: () => selected && void run(() => resumeDownload(selected.id), { immediate: true }),
    stop: () => selected && void run(() => pauseDownload(selected.id)),
    deleteSelected,
  }

  const menuActions: MenuBarActions = {
    add: () => openManualAddUrl(),
    downloadAll: () => setShowDownloadAll(true),
    batch: () => setShowBatch(true),
    resume: () => selected && void run(() => resumeDownload(selected.id), { immediate: true }),
    stop: () => selected && void run(() => pauseDownload(selected.id)),
    stopAll: () => void handleStopAll(),
    deleteSelected,
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
            <span>{banner.message}</span>
            <span className="flex shrink-0 items-center gap-1">
              {banner.source === 'capture' && failedCapture ? (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    // Round 75 — "Capture failed — Open": retry
                    // this capture's Add URL window. The dismissal
                    // for 'capture' is forgotten first, so a
                    // second failure can show again.
                    const retry = failedCapture
                    failedCaptureIds.current.clear(retry.id)
                    dismissedBannerRef.current.clearForSource('capture')
                    setFailedCapture(null)
                    setBannerError(null)
                    setCaptureQueue((current) => [retry, ...current])
                  }}
                >
                  Retry
                </Button>
              ) : null}
              <button
                type="button"
                onClick={() => {
                  // Round 71 (D2), reworked Round 75 — remember
                  // the dismissed (source, message) so the next
                  // poll cannot re-populate the banner with it;
                  // see `dismissedBannerRef` above. A capture
                  // dismissal also drops its Retry: the link can
                  // still be re-sent from the browser (a NEW
                  // pending id opens normally).
                  dismissedBannerRef.current.dismiss(banner.source, banner.message)
                  setBannerError(null)
                  setError(null)
                  setFailedCapture(null)
                }}
                aria-label="Dismiss error"
                className="rounded-md p-1 hover:bg-surface-2"
                style={{ WebkitAppRegion: 'no-drag' } as CSSProperties}
              >
                <X className="h-4 w-4" aria-hidden="true" />
              </button>
            </span>
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
                  showBannerError('action', e instanceof Error ? e.message : String(e)),
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
          onRenameGrabber={(project, name) =>
            void run(() => renameGrabberProject(project.id, name).then(() => void refreshGrabberProjects()))
          }
          onDeleteGrabber={(project) =>
            void run(() =>
              deleteGrabberProject(project.id).then(() => {
                if (selectedGrabberId === project.id) selectFilter('all')
                return void refreshGrabberProjects()
              }),
            )
          }
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
              onError={(message) => showBannerError('action', message)}
            />
          ) : (
            <DownloadTable
              downloads={visible}
              categories={categories}
              queues={queues}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onSelectionChange={handleSelectionChange}
              onRemoveMany={requestRemoveMany}
              onOpenProgress={(id) => void openProgressWindowFor(id, false)}
              onAdd={() => openManualAddUrl()}
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
        lastUsedFolder={settings?.last_used_folder ?? ''}
        autoFileTypes={settings?.file_types_auto ?? []}
        initialUrl={clipboardCapture?.url ?? pickedVideoUrl}
        captureMode={clipboardCapture !== null}
        captureTitle={clipboardCapture?.filename ?? null}
        alreadyInList={clipboardCapture?.already_in_list ?? false}
        onClose={() => {
          // Round 75 — a clipboard capture closed without a submit
          // is simply forgotten: a fresh copy re-offers it (new
          // clipboard sequence), so no engine-side dismiss is
          // needed any more. (Extension captures live in their own
          // standalone window now and resolve themselves.)
          if (clipboardCapture) {
            setClipboardCapture(null)
            clipboardCaptureRef.current = null
          }
          setShowAdd(false)
          setPickedVideoUrl('')
        }}
        onAdded={(id) => {
          // Round 75 — the submitted capture is consumed; the
          // dialog's close afterwards clears the ref (onClose).
          // Round 55 (defect 3a): the new row must be visible — a
          // download auto-assigned to another category used to be
          // hidden by the sidebar filter and look like a silent
          // failure. Go to "All downloads" and select the row, so
          // it is in view and the toolbar acts on it.
          selectFilter('all')
          setSelectedId(id)
          // Round 76 (2) — the remembered folder is stored by the
          // submit (updateCategory), but the dialog's autofill
          // reads `categories` state: refresh it now, or the next
          // dialog still shows the old folder for the category.
          // Round 87.5 (E5) — the submit may also have stored
          // the picked folder as the GLOBAL default (no category
          // set), so refresh the settings too, or the next dialog
          // still shows the old default folder.
          void refreshCategories()
          void refreshSettings()
          void refreshAll()
        }}
        onLateError={(message) => showBannerError('action', message)}
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
        // Round 87 (13c) — the main queue is hidden from the
        // queue lists, including the scheduler's dropdown.
        queues={queues.filter((q) => !q.is_main)}
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
                {confirmRemove.length === 1
                  ? 'Remove this download?'
                  : `Remove ${confirmRemove.length} downloads?`}
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
              {confirmRemove.length === 1
                ? `“${confirmRemove[0].filename}” will be removed from the list. The file on disk is kept.`
                : `${confirmRemove.length} downloads will be removed from the list. The files on disk are kept.`}
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setConfirmRemove(null)}>
                Keep it
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  const rows = confirmRemove
                  setConfirmRemove(null)
                  void run(async () => {
                    for (const row of rows) {
                      await deleteDownload(row.id, false)
                    }
                    setSelectedId(null)
                    setSelectedIds([])
                  }).then(() => void refreshFreeSpace())
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
