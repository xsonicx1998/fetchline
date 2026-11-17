import { useEffect, useRef, useState } from 'react'
import { ChevronDown, ChevronUp, FileWarning, FolderOpen, Pause, Play, RotateCw, ShieldCheck, ShieldX, X } from 'lucide-react'
import { completeActionLabel, formatBytes, formatEta, formatSpeed, progressPercent, statusLabel } from '../lib/format'
import {
  cancelDownload,
  deleteDownload,
  getProgressSnapshot,
  getQueue,
  getSettings,
  listDownloads,
  openFile,
  openFolder,
  pauseDownload,
  quarantineDownload,
  reloadWebviewWindow,
  resumeDownload,
  retryNow,
  runAntivirusScan,
  setDownloadSpeedLimit,
  setProgressWindowCompact,
  updateQueue,
  withTimeout,
} from '../lib/api'
import { recordSpeedSample, speedGraphPoints, speedGraphPolyline } from '../lib/speedGraph'
import type { SpeedSample } from '../lib/speedGraph'
import type { DownloadRow, ProgressEvent, QueueCompleteAction, QueueRow, QueueScheduleType, ScanResult, Settings } from '../lib/types'
import { Button } from './Button'
import { ProgressBar } from './ProgressBar'
import { ProgressRing } from './ProgressRing'
import { StatusBadge } from './StatusBadge'

type ProgressTab = 'status' | 'limiter' | 'completion'

/** S14 — the scan result line, shared by the status tab's finished
 *  block and the completion tab (which also carries the actions). */
function ScanStatus({ scan }: { scan: ScanResult | null }) {
  if (scan?.status === 'clean') {
    return (
      <p role="status" className="mt-2 flex items-start gap-2 text-sm">
        <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden="true" />
        <span>Antivirus scan: clean — no threat found.</span>
      </p>
    )
  }
  if (scan?.status === 'infected') {
    return (
      <div role="alert" className="mt-2 flex items-start gap-2 text-sm text-danger">
        <ShieldX className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>
          Antivirus scan: threat found
          {scan.detail ? ` — ${scan.detail}` : '.'} The file was not opened. Delete it or
          quarantine it from the Options on completion tab.
        </span>
      </div>
    )
  }
  if (scan?.status === 'scan_failed') {
    return (
      <p role="status" className="mt-2 flex items-start gap-2 text-sm text-warning">
        <FileWarning className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>Antivirus scan failed{scan.detail ? ` — ${scan.detail}` : '.'}</span>
      </p>
    )
  }
  if (scan?.status === 'timed_out') {
    return (
      <p role="status" className="mt-2 flex items-start gap-2 text-sm text-warning">
        <FileWarning className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>Antivirus scan timed out{scan.detail ? ` — ${scan.detail}` : '.'}</span>
      </p>
    )
  }
  if (scan?.status === 'scanning') {
    return (
      <p role="status" className="mt-2 text-sm text-text-muted">
        Scanning with your antivirus scanner…
      </p>
    )
  }
  if (scan?.status === 'not_configured') {
    return (
      <p className="mt-2 text-sm text-text-muted">
        No antivirus scanner is configured — the file was not scanned. Set one up in
        Settings &gt; Downloads.
      </p>
    )
  }
  return <p className="mt-2 text-sm text-text-muted">Scan result not available yet.</p>
}

/** S03 — the download progress WINDOW. A separate OS window per
 *  download (opened by the `open_progress_window` Tauri command),
 *  titled "<percent>% <filename>". Same content as the old in-app
 *  overlay had — live speed, ETA, per-part bars, 60 s speed graph,
 *  retry countdown, Open folder on completion — plus the IDM-style
 *  labelled rows (source, size, downloaded, transfer rate, time
 *  left, resume capability) and a "Hide details" toggle that
 *  collapses it to a compact view. Closing the window NEVER
 *  cancels the download: the engine keeps running and the window
 *  is only a view of it.
 *
 *  Live feed: the window polls `list_downloads` (the row) and
 *  `get_progress_snapshot` (the latest progress event) every
 *  500 ms. Polling is deliberate — the `download-progress`
 *  listener was removed in Round 39 and polling is this window's
 *  verified live feed (the main window's listeners work again
 *  since Round 40 added the capability file, but this window
 *  keeps polling by design). The 500 ms poll is also what feeds
 *  the speed graph, so the graph actually builds here (in the old
 *  overlay it never got a sample in production).
 *  The window title is updated from Rust, in the progress-event
 *  forwarder — the frontend cannot set its own title for the
 *  same capability reason. */
export function ProgressWindow({ downloadId }: { downloadId: string }) {
  const [download, setDownload] = useState<DownloadRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  /** Round 55 (defect 2, diagnostics): the last poll failure, if
   *  any — a bridge failure is not "Download removed", and the
   *  window should say so instead of showing the wrong empty
   *  state. Cleared by the next successful poll. */
  const [pollError, setPollError] = useState<string | null>(null)
  /** Round 56 (§7.3): backend-not-responding guard, same as the
   *  main window's — in-flight skip, 5 s poll timeout, and a
   *  backend-dead state after ~10 s with no poll response, with
   *  Retry / Reload. The refs mirror the state so the interval
   *  callback never reads a stale closure. */
  const [backendDead, setBackendDead] = useState(false)
  const backendDeadRef = useRef(false)
  const pollInFlight = useRef(false)
  const pollStoppedRef = useRef(false)
  const lastPollOkAt = useRef(Date.now())
  const [live, setLive] = useState<ProgressEvent | null>(null)
  const [samples, setSamples] = useState<SpeedSample[]>([])
  const [compact, setCompact] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [retryBusy, setRetryBusy] = useState(false)
  const [folderBusy, setFolderBusy] = useState(false)
  const [scanBusy, setScanBusy] = useState<string | null>(null)
  /** IDM's three tabs (Round 44): Download status, Speed Limiter,
   *  Options on completion. */
  const [tab, setTab] = useState<ProgressTab>('status')
  /** Settings (for the global speed cap shown on the limiter tab)
   *  and the download's queue (for the completion tab's editor). */
  const [settings, setSettings] = useState<Settings | null>(null)
  const [queue, setQueue] = useState<QueueRow | null>(null)
  /** Speed Limiter tab: the cap input, seeded from the row when
   *  the tab opens; empty = no per-download cap. */
  const [capInput, setCapInput] = useState('')
  const [capBusy, setCapBusy] = useState(false)
  /** Options on completion tab: the queue's completion action
   *  editor, seeded from the queue row when the tab opens. */
  const [completionAction, setCompletionAction] = useState<QueueCompleteAction>('none')
  const [completionProgram, setCompletionProgram] = useState('')
  const [completionBusy, setCompletionBusy] = useState(false)
  const [completionSaved, setCompletionSaved] = useState(false)

  // The window runs the same bundle as the main window. Dark only
  // (Round 45): the stored theme is ignored, the attribute stays
  // 'dark' so the Tailwind dark-mode selector keeps working.
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
    void getSettings()
      .then((s) => {
        setSettings(s)
      })
      .catch(() => undefined)
  }, [])

  // The queue this download belongs to (if any) — the completion
  // tab edits its on-completion action. Depend on the queue id
  // string, not the whole row: the 500 ms poll replaces the row
  // object every tick, and refetching on every tick would reseed
  // the completion editor while the user types in it.
  const queueId = download?.queue_id ?? null
  useEffect(() => {
    if (!queueId) {
      setQueue(null)
      return
    }
    let stopped = false
    void getQueue(queueId)
      .then((row) => {
        if (!stopped) setQueue(row)
      })
      .catch(() => undefined)
    return () => {
      stopped = true
    }
  }, [queueId])

  // Seed the completion editor from the queue row when the tab
  // opens or the row arrives (after the fetch, or after a save —
  // the saved values are the ones just entered, so reseeding is
  // harmless). The limiter input is seeded only on tab open (in
  // openTab): the poll replaces the row every 500 ms, so seeding
  // it from the row on every change would clobber typing.
  useEffect(() => {
    if (tab === 'completion' && queue) {
      setCompletionAction(queue.on_complete_action as QueueCompleteAction)
      setCompletionProgram(queue.on_complete_program ?? '')
    }
  }, [tab, queue])

  // Seed the tab editors when their tab opens, from the freshest
  // row — typing in them must not be clobbered by the 500 ms poll.
  function openTab(next: ProgressTab) {
    setTab(next)
    setActionError(null)
    setCompletionSaved(false)
    if (next === 'limiter' && download) {
      setCapInput(download.speed_limit_kbps ? String(download.speed_limit_kbps) : '')
    }
  }

  useEffect(() => {
    let stopped = false
    async function poll() {
      // In-flight guard (§7.3): skip the tick while a poll is
      // still running — a hung invoke must not pile up a new one
      // every 500 ms.
      if (pollInFlight.current) return
      pollInFlight.current = true
      try {
        const rows = await withTimeout(listDownloads(), 5000, 'Backend did not respond within 5 seconds')
        if (stopped) return
        const row = rows.find((d) => d.id === downloadId) ?? null
        setDownload(row)
        setLoaded(true)
        setPollError(null)
        lastPollOkAt.current = Date.now()
        if (backendDeadRef.current) {
          backendDeadRef.current = false
          setBackendDead(false)
        }
        if (row) {
          const snap = await withTimeout(getProgressSnapshot(downloadId), 5000, 'Backend did not respond within 5 seconds')
          if (stopped) return
          if (snap && snap.id === downloadId) setLive(snap)
        }
      } catch (e) {
        // Round 55: outside the desktop app the bridge throws, and
        // in the app a failing invoke means the window cannot reach
        // the engine — either way the empty state below must say
        // so instead of claiming the download was removed.
        setPollError(e instanceof Error ? e.message : String(e))
        setLoaded(true)
      } finally {
        pollInFlight.current = false
      }
    }
    void poll()
    const timer = window.setInterval(() => {
      if (pollStoppedRef.current) return
      void poll()
      // Backend-dead detection (§7.3): no successful poll for
      // ~10 s — stop issuing invokes and show the banner. A poll
      // still in flight is not a dead backend (its 5 s timeout
      // has not fired yet), so the check waits for it. The
      // interval itself keeps ticking (its ticks no-op) so Retry
      // needs no timer recreation.
      if (
        !pollInFlight.current &&
        Date.now() - lastPollOkAt.current > 10_000 &&
        !backendDeadRef.current
      ) {
        backendDeadRef.current = true
        pollStoppedRef.current = true
        setBackendDead(true)
      }
    }, 500)
    return () => {
      stopped = true
      window.clearInterval(timer)
    }
  }, [downloadId])

  /** Resume polling after the backend-dead banner (§7.3): clear
   *  the stop flag and reset the staleness clock. The interval
   *  is still alive (its ticks no-op while stopped), so the next
   *  tick — at most 500 ms away — polls on its own. If the
   *  backend is still dead the banner comes back after ~10 s. */
  function retryPolling() {
    pollStoppedRef.current = false
    backendDeadRef.current = false
    setBackendDead(false)
    lastPollOkAt.current = Date.now()
  }

  /** Reload this window (§7.4): closes the progress window and
   *  rebuilds it from scratch, which also replaces a dead
   *  renderer process with a fresh one. A failure surfaces as
   *  the window's action error instead of pretending the window
   *  recovered. */
  function reloadWindow() {
    void reloadWebviewWindow(`progress-${downloadId}`).catch((e: unknown) =>
      setActionError(e instanceof Error ? e.message : String(e)),
    )
  }

  // Fresh samples when the window shows another download (it
  // never does in practice — one window, one download — but the
  // reset costs nothing and matches the old dialog).
  useEffect(() => {
    setSamples([])
    setActionError(null)
    setRetryBusy(false)
    setFolderBusy(false)
    setScanBusy(null)
  }, [downloadId])

  useEffect(() => {
    if (!live || !download || live.id !== download.id) return
    if (live.status !== 'downloading') return
    setSamples((prev) => recordSpeedSample(prev, Date.now(), live.speed_bps))
  }, [live, download])

  async function toggleCompact() {
    const next = !compact
    setCompact(next)
    try {
      await setProgressWindowCompact(downloadId, next)
    } catch {
      // Outside the desktop app (or a closed window) the command
      // throws; the in-page collapse still applies.
    }
  }

  async function handleRetryNow() {
    setRetryBusy(true)
    setActionError(null)
    try {
      await retryNow(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setRetryBusy(false)
    }
  }

  async function handlePause() {
    setActionError(null)
    try {
      await pauseDownload(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleResume() {
    setActionError(null)
    try {
      await resumeDownload(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleCancel() {
    setActionError(null)
    try {
      await cancelDownload(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenFolder() {
    setFolderBusy(true)
    setActionError(null)
    try {
      await openFolder(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setFolderBusy(false)
    }
  }

  /** S14 — open the finished file. The engine refuses when the
   *  scanner flagged it, so a threat can never be opened from
   *  here (the button is disabled too). */
  async function handleOpenFile() {
    setScanBusy('open')
    setActionError(null)
    try {
      await openFile(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** S14 — scan the finished file again with the configured
   *  scanner. The result arrives on the live snapshot and on the
   *  download row, so the window updates without a refresh. */
  async function handleRescan() {
    setScanBusy('rescan')
    setActionError(null)
    try {
      await runAntivirusScan(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** S14 — quarantine is an explicit user choice: the engine
   *  moves the flagged file into a Quarantine folder. The window
   *  then closes — the file no longer lives at the old path. */
  async function handleQuarantine() {
    setScanBusy('quarantine')
    setActionError(null)
    try {
      await quarantineDownload(downloadId)
      window.close()
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** S14 — delete the flagged file. Also an explicit user choice:
   *  nothing is ever deleted automatically. */
  async function handleDeleteFile() {
    setScanBusy('delete')
    setActionError(null)
    try {
      await deleteDownload(downloadId, true)
      window.close()
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** Speed Limiter tab — apply the per-download cap, live: the
   *  engine's token bucket picks it up at once (S12). Empty or 0
   *  removes the cap; the global cap still applies, so the
   *  effective speed is the lower of the two. `value` overrides
   *  the input (the Remove cap button passes '' — state updates
   *  are async, so reading the input there would use the old
   *  value). */
  async function handleApplyCap(value?: string) {
    if (!download) return
    setCapBusy(true)
    setActionError(null)
    try {
      const trimmed = (value ?? capInput).trim()
      let kbps: number | null = null
      if (trimmed !== '') {
        const parsed = Number(trimmed)
        if (!Number.isInteger(parsed) || parsed < 0 || parsed > 1_000_000) {
          throw new Error('Enter a whole number of KB/s between 1 and 1,000,000, or leave it empty for no cap.')
        }
        kbps = parsed === 0 ? null : parsed
      }
      await setDownloadSpeedLimit(downloadId, kbps)
      const row = await listDownloads().then((rows) => rows.find((d) => d.id === downloadId) ?? null)
      if (row) setDownload(row)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setCapBusy(false)
    }
  }

  /** Options on completion tab — save the download's queue
   *  completion action. The action fires when the queue finishes:
   *  open the file, run a program, shut down or hibernate — the
   *  last two ask first, with the existing 60 s cancellable
   *  countdown in the main window. */
  async function handleSaveCompletion() {
    if (!queue) return
    setCompletionBusy(true)
    setActionError(null)
    setCompletionSaved(false)
    try {
      const updated = await updateQueue(queue.id, {
        schedule_type: queue.schedule_type as QueueScheduleType,
        start_at: queue.start_at,
        stop_at: queue.stop_at,
        start_on_startup: queue.start_on_startup,
        on_complete_action: completionAction,
        on_complete_program: completionAction === 'run_program' ? completionProgram.trim() || null : null,
        sync_interval_minutes: queue.sync_interval_minutes,
      })
      setQueue(updated)
      setCompletionSaved(true)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setCompletionBusy(false)
    }
  }

  if (!loaded) {
    return (
      <div className="flex h-full flex-col p-6 text-text">
        <p className="text-sm text-text-muted">Loading download…</p>
      </div>
    )
  }

  if (!download) {
    return (
      <div className="flex h-full flex-col p-6 text-text">
        <h1 className="text-lg font-semibold">
          {pollError ? 'Could not load this download' : 'Download removed'}
        </h1>
        <p className="mt-2 text-sm text-text-muted">
          {pollError
            ? `The window could not reach the app: ${pollError} The download itself is not affected.`
            : 'This download is no longer in the list — it was removed from the main window.'}
        </p>
        <div className="mt-6 flex justify-end gap-2">
          {backendDead ? (
            <>
              <Button variant="secondary" onClick={retryPolling}>
                Retry connection
              </Button>
              <Button variant="secondary" onClick={reloadWindow}>
                Reload window
              </Button>
            </>
          ) : null}
          <Button variant="primary" onClick={() => window.close()}>
            Close
          </Button>
        </div>
      </div>
    )
  }

  const downloaded = live?.downloaded_bytes ?? download.downloaded_bytes
  const size = live?.size_bytes ?? download.size_bytes
  const speed = live?.speed_bps ?? download.speed_bps
  const eta = live?.eta_seconds ?? download.eta_seconds
  const percent = progressPercent(downloaded, size)
  const segments = live?.segments ?? []
  // The countdown is the engine's: `retry` arrives on the live
  // snapshot (every 500 ms) with seconds left until the next
  // attempt, counted down from the engine's backoff deadline.
  const retry = download.status === 'downloading' ? (live?.retry ?? null) : null
  // S14 — the scan result: the live snapshot carries it once the
  // detached scan finishes; otherwise the download row (refreshed
  // by the poll) carries the stored result.
  const scan: ScanResult | null =
    live?.scan ??
    (download.scan_status
      ? {
          status: download.scan_status,
          detail: download.scan_detail,
          scanned_at: download.scan_scanned_at,
        }
      : null)
  const threatFound = scan?.status === 'infected'
  const points = speedGraphPoints(samples, 480, 80)
  const polyline = speedGraphPolyline(points)
  const area = points.length >= 2 ? `${polyline} 480,80 0,80` : null
  const resumeCapability =
    download.accept_ranges === true ? 'Yes' : download.accept_ranges === false ? 'No' : 'Unknown'

  return (
    <div className="flex h-full flex-col p-6 text-text">
      {backendDead ? (
        <div role="alert" className="mb-4 flex shrink-0 items-center justify-between gap-2 rounded-md border border-border bg-danger-soft px-3 py-2 text-sm text-danger">
          <span>Backend not responding — restart the app. Your downloads are safe on disk.</span>
          <span className="flex shrink-0 items-center gap-2">
            <Button variant="secondary" size="sm" onClick={retryPolling}>
              Retry connection
            </Button>
            <Button variant="secondary" size="sm" onClick={reloadWindow}>
              Reload window
            </Button>
          </span>
        </div>
      ) : null}
      <div className="mb-4 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="truncate text-lg font-semibold" title={download.filename}>
            {download.filename}
          </h1>
          <p className="mt-1 text-sm text-text-muted">
            {download.status === 'downloading'
              ? 'Receiving data…'
              : download.status === 'complete'
                ? 'Download complete'
                : statusLabel(download.status)}
          </p>
        </div>
        <div className="flex shrink-0 items-start gap-2">
          <ProgressRing
            percent={percent}
            status={download.status === 'complete' ? 'complete' : download.status === 'error' ? 'error' : download.status === 'paused' ? 'paused' : 'downloading'}
            size={56}
            label={`Progress of ${download.filename}`}
          />
          <button
            type="button"
            onClick={() => window.close()}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2"
          >
            <X className="h-5 w-5" aria-hidden="true" />
          </button>
        </div>
      </div>

      <div className="mb-4 flex items-center gap-3">
        <StatusBadge status={download.status} />
        <span className="font-mono text-sm text-text-muted">{download.url}</span>
      </div>

      {!compact ? (
        <>
          <div role="tablist" aria-label="Download details" className="mb-4 flex gap-1 border-b border-border">
            {(
              [
                ['status', 'Download status'],
                ['limiter', 'Speed Limiter'],
                ['completion', 'Options on completion'],
              ] as const
            ).map(([value, label]) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={tab === value}
                onClick={() => openTab(value)}
                className={`-mb-px border-b-2 px-3 py-2 text-sm font-semibold ${
                  tab === value ? 'border-accent text-accent' : 'border-transparent text-text-muted hover:text-text'
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          {tab === 'status' ? (
            <>
              <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm">
        <div>
          <dt className="text-text-muted">File size</dt>
          <dd>{formatBytes(size)}</dd>
        </div>
        <div>
          <dt className="text-text-muted">Downloaded</dt>
          <dd>
            {formatBytes(downloaded)}
            {percent !== null ? ` (${percent}%)` : ''}
          </dd>
        </div>
        <div>
          <dt className="text-text-muted">Transfer rate</dt>
          <dd>{formatSpeed(speed)}</dd>
        </div>
        <div>
          <dt className="text-text-muted">Time left</dt>
          <dd>{formatEta(eta)}</dd>
        </div>
        <div>
          <dt className="text-text-muted">Resume capability</dt>
          <dd>{resumeCapability}</dd>
        </div>
        {download.speed_limit_kbps ? (
          <div>
            <dt className="text-text-muted">Speed cap</dt>
            <dd>{download.speed_limit_kbps} KB/s</dd>
          </div>
        ) : null}
      </dl>

      {download.quota_paused ? (
        <p className="mt-4 text-sm text-danger">
          Paused — the download quota is reached. It resumes on its own when the quota window
          rolls over, or sooner with “Reset quota &amp; resume now” in Settings.
        </p>
      ) : null}

              {download.status === 'downloading' ? (
            <div className="mt-4">
              <p className="mb-2 text-sm font-semibold">Speed over the last minute</p>
              {points.length >= 2 ? (
                <svg
                  viewBox="0 0 480 80"
                  className="h-20 w-full rounded-md border border-border bg-surface"
                  role="img"
                  aria-label={`Speed graph for the last minute, current speed ${formatSpeed(speed)}`}
                >
                  {area ? <polygon points={area} fill="var(--color-accent-soft)" /> : null}
                  <polyline points={polyline} fill="none" stroke="var(--color-accent)" strokeWidth="2" strokeLinejoin="round" />
                </svg>
              ) : points.length === 1 ? (
                <svg
                  viewBox="0 0 480 80"
                  className="h-20 w-full rounded-md border border-border bg-surface"
                  role="img"
                  aria-label={`Speed graph for the last minute, current speed ${formatSpeed(speed)}`}
                >
                  <circle cx={points[0].x} cy={points[0].y} r="3" fill="var(--color-accent)" />
                </svg>
              ) : (
                <p className="rounded-md border border-border bg-surface p-3 text-sm text-text-muted">
                  The speed graph builds as the download runs.
                </p>
              )}
            </div>
          ) : null}

          {segments.length > 0 ? (
            <div className="mt-4">
              <p className="mb-2 text-sm font-semibold">Parts</p>
              <div className="flex flex-col gap-2" role="list" aria-label="Download parts">
                {segments.map((s) => {
                  const segTotal = s.end_byte === null ? null : s.end_byte - s.start_byte + 1
                  const segPercent = segTotal === null ? null : Math.min(100, Math.round((s.downloaded_bytes / segTotal) * 100))
                  return (
                    <div key={s.segment_index} role="listitem" className="flex items-center gap-2 text-sm">
                      <span className="w-16 shrink-0 font-mono text-xs text-text-muted">Part {s.segment_index + 1}</span>
                      <ProgressBar percent={segPercent} status={s.status === 'done' ? 'complete' : 'downloading'} label={`Part ${s.segment_index + 1}`} />
                    </div>
                  )
                })}
              </div>
            </div>
          ) : null}

          {retry ? (
            <div role="status" className="mt-4 rounded-md border border-border bg-warning-soft p-3">
              <p className="text-sm font-semibold">
                Connection lost — retrying in {retry.retry_in_secs}s (attempt {retry.attempt + 1} of {retry.max_attempts})
              </p>
              <p className="mt-1 text-sm text-text-muted">{retry.error}</p>
              <div className="mt-3 flex gap-2">
                <Button variant="secondary" onClick={handleRetryNow} disabled={retryBusy}>
                  <RotateCw className="h-4 w-4" aria-hidden="true" /> Retry now
                </Button>
                <Button variant="ghost" onClick={handleCancel}>
                  Cancel download
                </Button>
              </div>
            </div>
          ) : null}

          {download.status === 'error' && download.error_message ? (
            <p role="alert" className="mt-4 text-sm text-danger">
              {download.error_message}
            </p>
          ) : null}

          {download.status === 'complete' ? (
            <div className="mt-4 rounded-md border border-border bg-surface p-3">
              <p className="text-sm font-semibold">Finished file</p>
              <p className="mt-1 break-all font-mono text-sm text-text-muted" title={download.save_path}>
                {download.save_path}
              </p>
              <ScanStatus scan={scan} />
            </div>
          ) : null}
            </>
          ) : null}
          {tab === 'limiter' ? (
            <div className="mt-2 text-sm">
              <p className="text-text-muted">
                {download.speed_limit_kbps
                  ? `This download is capped at ${download.speed_limit_kbps} KB/s.`
                  : 'This download has no per-download cap.'}
                {settings?.speed_limit_kbps
                  ? ` The global limit is ${settings.speed_limit_kbps} KB/s — the effective speed is the lower of the two.`
                  : ' There is no global speed limit set.'}
              </p>
              <div className="mt-4 flex flex-wrap items-center gap-2">
                <label htmlFor="speed-cap" className="font-semibold">
                  Limit this download to
                </label>
                <input
                  id="speed-cap"
                  type="number"
                  min={0}
                  max={1_000_000}
                  step={1}
                  inputMode="numeric"
                  value={capInput}
                  onChange={(e) => setCapInput(e.target.value)}
                  placeholder="No cap"
                  aria-label="Per-download speed cap in KB/s"
                  className="h-8 w-28 rounded-md border border-border-input bg-bg px-2 font-mono text-sm text-text"
                />
                <span className="text-text-muted">KB/s</span>
                <Button variant="primary" size="sm" onClick={() => void handleApplyCap()} disabled={capBusy}>
                  Apply
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={capBusy || !download.speed_limit_kbps}
                  onClick={() => void handleApplyCap('')}
                >
                  Remove cap
                </Button>
              </div>
              <p className="mt-2 text-sm text-text-muted">
                Leave it empty for no cap. The cap applies at once — the download keeps running.
              </p>
            </div>
          ) : null}
          {tab === 'completion' ? (
            <div className="mt-2 text-sm">
              <p className="font-semibold">Finished file</p>
              {download.status === 'complete' ? (
                <>
                  <p className="mt-1 break-all font-mono text-sm text-text-muted" title={download.save_path}>
                    {download.save_path}
                  </p>
                  <ScanStatus scan={scan} />
                  <div className="mt-3 flex flex-wrap gap-2">
                    {scan && scan.status !== 'scanning' ? (
                      <Button variant="secondary" size="sm" onClick={handleRescan} disabled={scanBusy !== null}>
                        <ShieldCheck className="h-4 w-4" aria-hidden="true" /> Scan again
                      </Button>
                    ) : null}
                    {threatFound ? (
                      <>
                        <Button variant="secondary" size="sm" onClick={handleQuarantine} disabled={scanBusy !== null}>
                          Quarantine
                        </Button>
                        <Button variant="danger" size="sm" onClick={handleDeleteFile} disabled={scanBusy !== null}>
                          Delete file
                        </Button>
                      </>
                    ) : null}
                  </div>
                </>
              ) : (
                <p className="mt-1 text-text-muted">
                  The file actions and the scan result appear here once the download finishes.
                </p>
              )}
              <p className="mt-5 font-semibold">When the queue finishes</p>
              {queue ? (
                <>
                  <p className="mt-1 text-text-muted">
                    Queue &ldquo;{queue.name}&rdquo;: {completeActionLabel(queue.on_complete_action)}
                    {queue.on_complete_action === 'run_program' && queue.on_complete_program
                      ? ` — ${queue.on_complete_program}`
                      : ''}
                  </p>
                  <div className="mt-3 flex flex-wrap items-center gap-2">
                    <label htmlFor="completion-action" className="font-semibold">
                      When done
                    </label>
                    <select
                      id="completion-action"
                      value={completionAction}
                      onChange={(e) => setCompletionAction(e.target.value as QueueCompleteAction)}
                      className="h-8 rounded-md border border-border-input bg-bg px-2 text-sm text-text"
                    >
                      <option value="none">Do nothing when done</option>
                      <option value="open_file">Open the file when done</option>
                      <option value="run_program">Run a program when done</option>
                      <option value="shutdown">Shut down the computer when done</option>
                      <option value="hibernate">Hibernate the computer when done</option>
                    </select>
                    {completionAction === 'run_program' ? (
                      <input
                        type="text"
                        value={completionProgram}
                        onChange={(e) => setCompletionProgram(e.target.value)}
                        placeholder="Program path"
                        aria-label="Program to run when done"
                        className="h-8 w-64 rounded-md border border-border-input bg-bg px-2 font-mono text-sm text-text"
                      />
                    ) : null}
                    <Button variant="primary" size="sm" onClick={handleSaveCompletion} disabled={completionBusy}>
                      Save
                    </Button>
                  </div>
                  {completionSaved ? (
                    <p role="status" className="mt-2 text-success">
                      Saved.
                    </p>
                  ) : null}
                  <p className="mt-2 text-text-muted">
                    Shut down and hibernate ask first — a 60 second countdown appears in the main
                    window, and the action runs only when the queue actually downloaded something.
                  </p>
                </>
              ) : (
                <p className="mt-1 text-text-muted">
                  This download is not in a queue — completion actions are set per queue, in
                  Queues &amp; scheduler.
                </p>
              )}
            </div>
          ) : null}
        </>
      ) : null}

      <ProgressBar
        percent={percent}
        size="tall"
        status={download.status === 'complete' ? 'complete' : download.status === 'error' ? 'error' : download.status === 'paused' ? 'paused' : 'downloading'}
        label={`Progress of ${download.filename}`}
      />

      {actionError ? (
        <p role="alert" className="mt-4 text-sm text-danger">
          {actionError}
        </p>
      ) : null}

      <div className="mt-6 flex flex-wrap justify-end gap-2">
        <Button variant="ghost" onClick={toggleCompact}>
          {compact ? (
            <>
              <ChevronDown className="h-4 w-4" aria-hidden="true" /> Show details
            </>
          ) : (
            <>
              <ChevronUp className="h-4 w-4" aria-hidden="true" /> Hide details
            </>
          )}
        </Button>
        {download.status === 'downloading' ? (
          <Button variant="secondary" onClick={handlePause}>
            <Pause className="h-4 w-4" aria-hidden="true" /> Pause
          </Button>
        ) : null}
        {download.status === 'paused' || download.status === 'error' ? (
          <Button variant="primary" onClick={handleResume}>
            <Play className="h-4 w-4" aria-hidden="true" /> Resume
          </Button>
        ) : null}
        {download.status === 'complete' ? (
          <>
            <Button
              variant="primary"
              onClick={handleOpenFile}
              disabled={threatFound || scan?.status === 'scanning' || scanBusy !== null}
              title={
                threatFound
                  ? 'The antivirus scan found a threat in this file, so it cannot be opened'
                  : scan?.status === 'scanning'
                    ? 'Wait for the antivirus scan to finish first'
                    : undefined
              }
            >
              Open
            </Button>
            <Button variant="secondary" onClick={handleOpenFolder} disabled={folderBusy}>
              <FolderOpen className="h-4 w-4" aria-hidden="true" /> Open folder
            </Button>
          </>
        ) : null}
        {download.status === 'downloading' || download.status === 'paused' ? (
          <Button variant="danger" onClick={handleCancel}>
            Cancel
          </Button>
        ) : null}
        <Button variant="ghost" onClick={() => window.close()}>
          Close
        </Button>
      </div>
    </div>
  )
}
