import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { X } from 'lucide-react'
import {
  addDownload,
  addToQueue,
  checkForUpdates,
  deleteCategory,
  deleteDownload,
  dismissClipboardOffer,
  executeQueueAction,
  getCategories,
  getPendingAction,
  getQueueFiles,
  getQuotaStatus,
  getSettings,
  listDownloads,
  listGrabberProjects,
  listQueues,
  moveToCategory,
  onClipboardLink,
  onQueueAction,
  onUpdateAvailable,
  openFolder,
  openProgressWindow,
  pauseDownload,
  resumeDownload,
  runGrabber,
  startQueue,
  stopQueue,
  syncQueueNow,
} from './lib/api'
import { formatSpeed } from './lib/format'
import type { CategoryRow, ClipboardOffer, DownloadRow, GrabberProjectRow, PendingAction, QueueRow, QuotaStatus, Settings, UpdateCheckResult } from './lib/types'
import { AddCategoryDialog } from './components/AddCategoryDialog'
import { AddDownloadDialog } from './components/AddDownloadDialog'
import { AddQueueDialog } from './components/AddQueueDialog'
import { BatchDialog } from './components/BatchDialog'
import { Button } from './components/Button'
import { CategorySidebar, type SidebarFilter } from './components/CategorySidebar'
import { ClipboardOfferDialog } from './components/ClipboardOfferDialog'
import { DownloadAllDialog } from './components/DownloadAllDialog'
import { DownloadTable } from './components/DownloadTable'
import { QueueActionDialog } from './components/QueueActionDialog'
import { RefreshAddressDialog } from './components/RefreshAddressDialog'
import { SchedulerDialog } from './components/SchedulerDialog'
import { SettingsDialog } from './components/SettingsDialog'
import { SiteGrabberDialog } from './components/SiteGrabberDialog'
import { Toolbar } from './components/Toolbar'
import { UpdateDialog } from './components/UpdateDialog'
import { VideoGrabberDialog } from './components/VideoGrabberDialog'

/** S01 — main window (Round 45): the user's asset pack — big-icon
 *  toolbar, categories tree on the left, file table. No menu bar:
 *  the reference has none (title bar only, which the native
 *  Windows title bar provides); its actions live on the toolbar,
 *  in the Grabber dropdown, on the table's right-click menu and in
 *  Settings. Dark only — the light theme was deleted at the user's
 *  instruction.
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
   *  category's), the unfinished / finished filter, or one
   *  grabber project's downloads. */
  const [selectedFilter, setSelectedFilter] = useState<SidebarFilter>('all')
  const [selectedGrabberId, setSelectedGrabberId] = useState<string | null>(null)
  const [showAdd, setShowAdd] = useState(false)
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
  const [settings, setSettings] = useState<Settings | null>(null)
  /** Download quota (S12): live usage, shown in the status bar
   *  when a quota is enabled; refreshed with the settings. */
  const [quotaStatus, setQuotaStatus] = useState<QuotaStatus | null>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState<DownloadRow | null>(null)
  /** Round 51 — refresh download address: the row whose expired
   *  link is being replaced; null when the dialog is closed. */
  const [refreshTarget, setRefreshTarget] = useState<DownloadRow | null>(null)
  /** Round 52 — clipboard monitoring: the copied link currently
   *  offered to the user; null when no offer is showing. */
  const [clipboardOffer, setClipboardOffer] = useState<ClipboardOffer | null>(null)
  /** Update check (S09): Help > Check for updates, and the weekly
   *  check's `update-available` event, both land here. */
  const [updateResult, setUpdateResult] = useState<UpdateCheckResult | null>(null)
  const [updateCheckError, setUpdateCheckError] = useState<string | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [showUpdateDialog, setShowUpdateDialog] = useState(false)

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
    try {
      setQueues(await listQueues())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const refreshGrabberProjects = useCallback(async () => {
    try {
      setGrabberProjects(await listGrabberProjects())
    } catch {
      // The tree's grabber section stays empty outside the
      // desktop app; the rest of the window keeps working.
    }
  }, [])

  const refreshCategories = useCallback(async () => {
    try {
      setCategories(await getCategories())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const refresh = useCallback(async () => {
    try {
      setDownloads(await listDownloads())
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const refreshAll = useCallback(async () => {
    await refresh()
    await refreshQueues()
    await refreshGrabberProjects()
    if (selectedQueue !== null) {
      try {
        setQueueFiles(await getQueueFiles(selectedQueue))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    }
  }, [refresh, refreshQueues, refreshGrabberProjects, selectedQueue])

  useEffect(() => {
    void refresh()
    void refreshCategories()
    void refreshQueues()
    void refreshGrabberProjects()
    void refreshSettings()
    const timer = window.setInterval(() => void refreshAll(), 1000)
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
          .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
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
  }, [refresh, refreshAll, refreshCategories, refreshQueues, refreshGrabberProjects, refreshSettings])

  /** Dark only (Round 45): the theme is a constant now, but the
   *  attribute stays set so the Tailwind dark-mode selector keeps
   *  working if a `dark:` variant is ever added. */
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
  }, [])

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

  /** Remove a download. When the "ask before removing" setting is
   *  on, the confirm dialog below runs first. */
  function requestRemove(download: DownloadRow) {
    if (settings?.confirm_delete) {
      setConfirmRemove(download)
    } else {
      void run(() => deleteDownload(download.id, false).then(() => setSelectedId(null)))
    }
  }

  const selected = useMemo(
    () => downloads.find((d) => d.id === selectedId) ?? null,
    [downloads, selectedId],
  )
  const visible = useMemo(() => {
    if (selectedQueue !== null) return queueFiles
    if (selectedFilter === 'unfinished') return downloads.filter((d) => d.status !== 'complete')
    if (selectedFilter === 'finished') return downloads.filter((d) => d.status === 'complete')
    if (selectedFilter === 'grabber') return downloads.filter((d) => d.grabber_project_id === selectedGrabberId)
    if (selectedCategory === null) return downloads
    return downloads.filter((d) => d.category_id === selectedCategory)
  }, [downloads, selectedCategory, selectedQueue, selectedFilter, selectedGrabberId, queueFiles])
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

  async function run(action: () => Promise<void>) {
    try {
      await action()
      await refreshAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
      setError(e instanceof Error ? e.message : String(e))
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
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
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
      if (!quiet) setError(e instanceof Error ? e.message : String(e))
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

  const toolbarActions = {
    add: () => setShowAdd(true),
    resume: () => selected && void run(() => resumeDownload(selected.id)),
    stop: () => selected && void run(() => pauseDownload(selected.id)),
    stopAll: () => void handleStopAll(),
    deleteSelected: () => selected && requestRemove(selected),
    deleteCompleted: () => void handleDeleteCompleted(),
    settings: () => setShowSettings(true),
    scheduler: () => openScheduler(),
    startQueue: () => activeQueueRow && void run(() => startQueue(activeQueueRow.id)),
    stopQueue: () => activeQueueRow && void run(() => stopQueue(activeQueueRow.id)),
    videoGrabber: () => setShowVideoGrabber(true),
    siteGrabber: () => setShowSiteGrabber(true),
    downloadAll: () => setShowDownloadAll(true),
    batch: () => setShowBatch(true),
  }

  return (
    <div className="flex h-full flex-col text-text">
      <Toolbar
        actions={toolbarActions}
        selected={selected}
        queue={activeQueueRow}
        hasDownloading={runningCount > 0}
        hasComplete={hasComplete}
      />

      {error ? (
        <div role="alert" className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-danger-soft px-4 py-2 text-sm text-danger">
          <span>{error}</span>
          <button type="button" onClick={() => setError(null)} aria-label="Dismiss error" className="rounded-md p-1 hover:bg-surface-2">
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
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
          onOpenScheduler={(queueId) => openScheduler(queueId)}
          onRunGrabber={(projectId) => void run(() => runGrabber(projectId))}
          onNewGrabber={() => setShowSiteGrabber(true)}
          onSyncQueue={(queueId) => void run(() => syncQueueNow(queueId).then(() => undefined))}
          onDropDownload={(downloadId, target) => void handleDropDownload(downloadId, target)}
        />
        {/* Round 47: the main area is a rounded panel with a thin
            glowing border, as in the reference — inset from the
            toolbar/footer by the row's gap/padding, like the
            sidebar panel. The translucent surface lets the neon
            waves show through behind the table while the rows stay
            readable (the thead is opaque). */}
        <main className="min-w-0 flex-1 overflow-auto rounded-xl border border-border bg-surface/40 shadow-panel">
          <DownloadTable
            downloads={visible}
            queues={queues}
            categories={categories}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onOpenProgress={(id) => void openProgressWindowFor(id, false)}
            onAdd={() => setShowAdd(true)}
            onOpenFolder={(id) => void run(() => openFolder(id))}
            onMoveToCategory={(id, categoryId) => void run(() => moveToCategory(id, categoryId))}
            onRefreshAddress={(download) => setRefreshTarget(download)}
          />
        </main>
      </div>

      <footer className="flex h-7 shrink-0 items-center justify-between border-t border-border bg-surface px-4 text-xs text-text-muted">
        <span>
          {downloads.length} download{downloads.length === 1 ? '' : 's'} ·{' '}
          {downloads.filter((d) => d.status === 'complete').length} finished
        </span>
        <span className="flex items-center gap-3">
          <span>{runningCount > 0 ? `Total speed ${formatSpeed(totalSpeed)}` : 'Ready'}</span>
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
          <button
            type="button"
            onClick={() => setShowSettings(true)}
            title="Open settings"
            className="rounded-md px-1 py-0.5 hover:bg-surface-2 hover:text-text"
          >
            {settings?.speed_limit_kbps ? `Limit ${settings.speed_limit_kbps} KB/s` : 'No speed limit'}
          </button>
        </span>
      </footer>

      <AddDownloadDialog
        open={showAdd}
        categories={categories}
        defaultAction={settings?.default_action ?? 'start_now'}
        initialUrl={pickedVideoUrl}
        onClose={() => {
          setShowAdd(false)
          setPickedVideoUrl('')
        }}
        onAdded={() => void refreshAll()}
      />
      <RefreshAddressDialog
        open={refreshTarget !== null}
        download={refreshTarget}
        onClose={() => setRefreshTarget(null)}
        onRefreshed={() => {
          setRefreshTarget(null)
          void refreshAll()
        }}
      />
      {clipboardOffer ? (
        <ClipboardOfferDialog
          offer={clipboardOffer}
          onDownload={() => {
            const url = clipboardOffer.url
            void addDownload({ url })
              .then(() => void refreshAll())
              .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
            setClipboardOffer(null)
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
          onClick={(e) => {
            if (e.target === e.currentTarget) setConfirmRemove(null)
          }}
        >
          <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <h2 id="confirm-remove-title" className="text-lg font-semibold">
              Remove this download?
            </h2>
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
                  void run(() => deleteDownload(row.id, false).then(() => setSelectedId(null)))
                }}
              >
                Remove
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
