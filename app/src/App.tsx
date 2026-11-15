import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { CalendarClock, Clapperboard, FolderDown, Globe, Link2, ListPlus, Moon, Pause, Play, Settings as SettingsIcon, Square, Sun, Trash2, X } from 'lucide-react'
import {
  addToQueue,
  cancelDownload,
  checkForUpdates,
  deleteCategory,
  deleteDownload,
  executeQueueAction,
  getCategories,
  getPendingAction,
  getQueueFiles,
  getQuotaStatus,
  getSettings,
  listDownloads,
  listQueues,
  moveToCategory,
  onQueueAction,
  onUpdateAvailable,
  openProgressWindow,
  pauseDownload,
  resumeDownload,
  startQueue,
  stopQueue,
  updateSettings,
} from './lib/api'
import { formatSpeed } from './lib/format'
import type { CategoryRow, DownloadRow, PendingAction, QueueRow, QuotaStatus, Settings, UpdateCheckResult } from './lib/types'
import { AddCategoryDialog } from './components/AddCategoryDialog'
import { AddDownloadDialog } from './components/AddDownloadDialog'
import { AddQueueDialog } from './components/AddQueueDialog'
import { BatchDialog } from './components/BatchDialog'
import { Button } from './components/Button'
import { CategorySidebar } from './components/CategorySidebar'
import { DownloadAllDialog } from './components/DownloadAllDialog'
import { DownloadTable } from './components/DownloadTable'
import { QueueActionDialog } from './components/QueueActionDialog'
import { SchedulerDialog } from './components/SchedulerDialog'
import { SettingsDialog } from './components/SettingsDialog'
import { SiteGrabberDialog } from './components/SiteGrabberDialog'
import { UpdateDialog } from './components/UpdateDialog'
import { VideoGrabberDialog } from './components/VideoGrabberDialog'
import logoUrl from './assets/logo.jpg'

/** S01 — main window. Header, toolbar, categories + queues sidebar,
 *  download table, status bar. The list is refreshed once a second
 *  and that refresh is the live path for the table — the
 *  `download-progress` listener that used to patch rows between
 *  polls is gone (BUG-044: it never fired in the built app). Queue
 *  completion actions are subscribed on `queue-action-pending`:
 *  shutdown / hibernate wait for the countdown dialog, the others
 *  run at once — but that listener shares the same BUG-044 denial,
 *  so a waiting action is also surfaced by the mount-time
 *  `getPendingAction` sweep below. */
export default function App() {
  const [downloads, setDownloads] = useState<DownloadRow[]>([])
  const [categories, setCategories] = useState<CategoryRow[]>([])
  const [queues, setQueues] = useState<QueueRow[]>([])
  const [queueFiles, setQueueFiles] = useState<DownloadRow[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null)
  const [selectedQueue, setSelectedQueue] = useState<string | null>(null)
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
  const [theme, setTheme] = useState<'light' | 'dark'>('dark')
  const [error, setError] = useState<string | null>(null)
  const [settings, setSettings] = useState<Settings | null>(null)
  /** Download quota (S12): live usage, shown in the status bar
   *  when a quota is enabled; refreshed with the settings. */
  const [quotaStatus, setQuotaStatus] = useState<QuotaStatus | null>(null)
  const [showSettings, setShowSettings] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState<DownloadRow | null>(null)
  /** Update check (S09): Help > Check for updates, and the weekly
   *  check's `update-available` event, both land here. */
  const [showHelpMenu, setShowHelpMenu] = useState(false)
  const [updateResult, setUpdateResult] = useState<UpdateCheckResult | null>(null)
  const [updateCheckError, setUpdateCheckError] = useState<string | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [showUpdateDialog, setShowUpdateDialog] = useState(false)

  const refreshSettings = useCallback(async () => {
    try {
      const s = await getSettings()
      setSettings(s)
      setTheme(s.theme)
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
    if (selectedQueue !== null) {
      try {
        setQueueFiles(await getQueueFiles(selectedQueue))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    }
  }, [refresh, refreshQueues, selectedQueue])

  useEffect(() => {
    void refresh()
    void refreshCategories()
    void refreshQueues()
    void refreshSettings()
    const timer = window.setInterval(() => void refreshAll(), 1000)
    let unlistenAction: (() => void) | undefined
    let unlistenUpdate: (() => void) | undefined
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
    }
  }, [refresh, refreshAll, refreshCategories, refreshQueues, refreshSettings])

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme)
  }, [theme])

  /** Manual update check (Help > Check for updates). Runs even
   *  when the weekly setting is off; a failure shows the error
   *  in the dialog instead of a silent nothing. */
  async function runManualUpdateCheck() {
    setShowHelpMenu(false)
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

  /** Theme toggle: applies at once and persists it in settings
   *  when the bridge is available. */
  function toggleTheme() {
    const next = theme === 'light' ? 'dark' : 'light'
    setTheme(next)
    if (settings) {
      void updateSettings({ ...settings, theme: next }).catch(() => undefined)
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
    if (selectedCategory === null) return downloads
    return downloads.filter((d) => d.category_id === selectedCategory)
  }, [downloads, selectedCategory, selectedQueue, queueFiles])
  const selectedQueueRow = useMemo(
    () => queues.find((q) => q.id === selectedQueue) ?? null,
    [queues, selectedQueue],
  )
  const totalSpeed = downloads
    .filter((d) => d.status === 'downloading')
    .reduce((sum, d) => sum + d.speed_bps, 0)
  const runningCount = downloads.filter((d) => d.status === 'downloading').length

  async function run(action: () => Promise<void>) {
    try {
      await action()
      await refreshAll()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
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

  return (
    <div className="flex h-full flex-col text-text">
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-border bg-surface px-4">
        <div className="flex items-center gap-2">
          <img src={logoUrl} alt="Sonic Manager logo" className="h-8 w-8 rounded-md object-cover" />
          <h1 className="text-lg font-bold">Sonic Manager</h1>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-sm text-text-muted">
            {runningCount > 0 ? `${runningCount} downloading · ${formatSpeed(totalSpeed)}` : 'Idle'}
          </span>
          <Button
            variant="ghost"
            size="sm"
            aria-label={theme === 'light' ? 'Switch to dark theme' : 'Switch to light theme'}
            onClick={toggleTheme}
          >
            {theme === 'light' ? <Moon className="h-4 w-4" aria-hidden="true" /> : <Sun className="h-4 w-4" aria-hidden="true" />}
          </Button>
          <div className="relative">
            <Button
              variant="ghost"
              size="sm"
              aria-label="Help menu"
              aria-expanded={showHelpMenu}
              onClick={() => setShowHelpMenu((v) => !v)}
            >
              Help
            </Button>
            {showHelpMenu && (
              <div
                role="menu"
                aria-label="Help"
                className="absolute right-0 z-20 mt-1 w-56 rounded-md border border-border bg-surface-raised p-1 shadow-pop"
              >
                <button
                  role="menuitem"
                  type="button"
                  className="w-full rounded px-3 py-2 text-left text-sm hover:bg-surface"
                  onClick={() => void runManualUpdateCheck()}
                >
                  Check for updates
                </button>
                <button
                  role="menuitem"
                  type="button"
                  className="w-full rounded px-3 py-2 text-left text-sm hover:bg-surface"
                  onClick={() => {
                    setShowHelpMenu(false)
                    setShowSettings(true)
                  }}
                >
                  Settings
                </button>
              </div>
            )}
          </div>
        </div>
      </header>

      <div role="toolbar" aria-label="Download actions" className="flex h-11 shrink-0 items-center gap-2 border-b border-border bg-surface px-4">
        <Button variant="primary" size="sm" onClick={() => setShowAdd(true)}>
          <Link2 className="h-4 w-4" aria-hidden="true" /> Add a link
        </Button>
        <Button variant="secondary" size="sm" onClick={() => setShowDownloadAll(true)}>
          <Globe className="h-4 w-4" aria-hidden="true" /> Download all
        </Button>
        <Button variant="secondary" size="sm" onClick={() => setShowBatch(true)}>
          <ListPlus className="h-4 w-4" aria-hidden="true" /> Batch
        </Button>
        <Button variant="secondary" size="sm" onClick={() => setShowVideoGrabber(true)}>
          <Clapperboard className="h-4 w-4" aria-hidden="true" /> Video grabber
        </Button>
        <Button variant="secondary" size="sm" onClick={() => setShowSiteGrabber(true)}>
          <FolderDown className="h-4 w-4" aria-hidden="true" /> Site grabber
        </Button>
        <Button
          variant="secondary"
          size="sm"
          disabled={!selected || selected.status !== 'downloading'}
          onClick={() => selected && void run(() => pauseDownload(selected.id))}
        >
          <Pause className="h-4 w-4" aria-hidden="true" /> Pause
        </Button>
        <Button
          variant="secondary"
          size="sm"
          disabled={!selected || (selected.status !== 'paused' && selected.status !== 'error')}
          onClick={() => selected && void run(() => resumeDownload(selected.id))}
        >
          <Play className="h-4 w-4" aria-hidden="true" /> Resume
        </Button>
        <Button
          variant="danger"
          size="sm"
          disabled={!selected || (selected.status !== 'downloading' && selected.status !== 'paused')}
          onClick={() => selected && void run(() => cancelDownload(selected.id))}
        >
          <X className="h-4 w-4" aria-hidden="true" /> Cancel
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={!selected}
          onClick={() => selected && requestRemove(selected)}
        >
          <Trash2 className="h-4 w-4" aria-hidden="true" /> Remove
        </Button>
        <label className="sr-only" htmlFor="move-to-category">
          Move selected download to a category
        </label>
        <select
          id="move-to-category"
          value=""
          disabled={!selected}
          onChange={(e) => {
            const categoryId = e.target.value
            if (selected && categoryId) void run(() => moveToCategory(selected.id, categoryId))
          }}
          className="h-7 rounded-md border border-border-input bg-bg px-2 text-sm text-text-muted focus:border-accent disabled:opacity-45"
        >
          <option value="">Move to category…</option>
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        <span className="mx-1 h-6 w-px shrink-0 bg-border" aria-hidden="true" />
        <Button variant="secondary" size="sm" onClick={() => openScheduler()}>
          <CalendarClock className="h-4 w-4" aria-hidden="true" /> Queues &amp; scheduler
        </Button>
        <Button variant="secondary" size="sm" onClick={() => setShowSettings(true)}>
          <SettingsIcon className="h-4 w-4" aria-hidden="true" /> Settings
        </Button>
        {selectedQueueRow ? (
          selectedQueueRow.running ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void run(() => stopQueue(selectedQueueRow.id))}
            >
              <Square className="h-4 w-4" aria-hidden="true" /> Stop queue
            </Button>
          ) : (
            <Button
              variant="primary"
              size="sm"
              disabled={selectedQueueRow.file_count === 0}
              onClick={() => void run(() => startQueue(selectedQueueRow.id))}
            >
              <Play className="h-4 w-4" aria-hidden="true" /> Start queue
            </Button>
          )
        ) : null}
      </div>

      {error ? (
        <div role="alert" className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-danger-soft px-4 py-2 text-sm text-danger">
          <span>{error}</span>
          <button type="button" onClick={() => setError(null)} aria-label="Dismiss error" className="rounded-md p-1 hover:bg-surface-2">
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
      ) : null}

      <div className="flex min-h-0 flex-1">
        <CategorySidebar
          categories={categories}
          downloads={downloads}
          queues={queues}
          selectedCategory={selectedCategory}
          selectedQueue={selectedQueue}
          onSelectCategory={setSelectedCategory}
          onSelectQueue={setSelectedQueue}
          onAddCategory={() => setShowAddCategory(true)}
          onAddQueue={() => setShowAddQueue(true)}
          onDeleteCategory={(category) =>
            void run(() => deleteCategory(category.id).then(() => void refreshCategories()))
          }
          onOpenScheduler={(queueId) => openScheduler(queueId)}
          onDropDownload={(downloadId, target) => void handleDropDownload(downloadId, target)}
        />
        <main className="min-w-0 flex-1 overflow-auto">
          <DownloadTable
            downloads={visible}
            categories={categories}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onOpenProgress={(id) => void openProgressWindowFor(id, false)}
            onAdd={() => setShowAdd(true)}
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
          setSelectedQueue(id)
          setSelectedCategory(null)
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
