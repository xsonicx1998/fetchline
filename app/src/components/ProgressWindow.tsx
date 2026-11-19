import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { File, FileDown, FileX, FolderOpen, Pause, Play, Power, RotateCw } from 'lucide-react'
import { formatBytes, formatEta, formatSpeed, progressPercent, statusLabel } from '../lib/format'
import {
  cancelDownload,
  exitApp,
  getDownloadCompletionOptions,
  getProgressSnapshot,
  getSettings,
  listDownloads,
  openFile,
  openFolder,
  pauseDownload,
  reloadWebviewWindow,
  resumeDownload,
  retryNow,
  setDownloadCompletionOptions,
  setDownloadSpeedLimit,
  shutdownComputer,
  withTimeout,
} from '../lib/api'
import type { DownloadCompletionOptions, DownloadRow, ProgressEvent, SegmentProgress, Settings } from '../lib/types'
import { Button } from './Button'
import { ConfirmDialog } from './ConfirmDialog'
import { ProgressBar } from './ProgressBar'
import { StatusBadge } from './StatusBadge'
import { TitleBar } from './TitleBar'

type ProgressTab = 'status' | 'options'

/** Round 62 — one part's IDM status line (N. | Downloaded |
 *  Info). The engine only reports two segment states — `done`
 *  and `active` (manager.rs) — so the five IDM words are
 *  DERIVED from what the snapshot carries, and the mapping is
 *  the honest one:
 *  - done → "Done" (cyan)
 *  - the download is not running (paused / failed / finished)
 *    and the part is not done → "No" (red, bold): it is not
 *    downloading right now
 *  - the download is in its retry backoff and the part is not
 *    done → "Disconnect" (pink): the connection dropped
 *  - running, part not done, zero bytes yet → "Send GET..."
 *    (amber): still connecting, nothing received
 *  - running, part not done, bytes flowing → "Receiving
 *    data..." (purple)
 *  The engine cannot tell "connecting" from "waiting for a
 *  slot" for a zero-byte part, so "Send GET..." is the
 *  best-supported reading, not a measured connection state. */
function partInfo(
  segment: SegmentProgress,
  downloadStatus: string,
  retrying: boolean,
): { label: string; className: string } {
  if (segment.status === 'done') return { label: 'Done', className: 'text-info' }
  if (downloadStatus !== 'downloading') return { label: 'No', className: 'font-bold text-[#ff3b30]' }
  if (retrying) return { label: 'Disconnect', className: 'text-danger' }
  if (segment.downloaded_bytes <= 0) return { label: 'Send GET...', className: 'text-warning' }
  return { label: 'Receiving data...', className: 'text-accent' }
}

/** S03 — the download progress WINDOW, redesigned in Round 62
 *  from the user's AI mockup. A separate OS window per download
 *  (opened by the `open_progress_window` Tauri command), titled
 *  "<percent>% <filename>", frameless (its own dark title bar,
 *  same as the main window), square like IDM — landscape, wider
 *  than tall, ~660x480. Two tabs: "Download status" and one
 *  merged "Options" tab (speed limiter + close program /
 *  shut down computer after download); the tabs sit at the top,
 *  directly under the title bar (Round 63), like the user's
 *  mockup. The 60 s speed graph and the "Hide details" toggle
 *  are gone (speed is text only), the per-part bars are an
 *  IDM-style table, and the antivirus scan choice is gone — on
 *  completion the footer offers Open file / Open folder
 *  (Round 63). Closing the window NEVER cancels the download:
 *  the engine keeps running and the window is only a view of
 *  it. The footer has Pause + Cancel while downloading/paused
 *  (Cancel restored in Round 63b per the user's decision, after
 *  its Round 63 removal; it cancels the download), and on
 *  completion it offers Open file / Open folder (Round 63)
 *  instead. The title-bar X still closes the window — the
 *  in-page footer Close button was removed in Round 64 per
 *  the user's decision.
 *
 *  Live feed: the window polls `list_downloads` (the row) and
 *  `get_progress_snapshot` (the latest progress event) every
 *  500 ms. Polling is deliberate — the `download-progress`
 *  listener was removed in Round 39 and polling is this window's
 *  verified live feed. The window title is updated from Rust,
 *  in the progress-event forwarder — the frontend cannot set
 *  its own title for the same capability reason. The custom
 *  title bar shows the same live "<percent>% - <filename>"
 *  from the poll. */
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
  /** Round 74 (11) — Cancel asks "Are you sure?" first (the
   *  ConfirmDialog below); this only runs once confirmed. */
  const [confirmCancel, setConfirmCancel] = useState(false)
  const [resumeBusy, setResumeBusy] = useState(false)
  const [live, setLive] = useState<ProgressEvent | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [retryBusy, setRetryBusy] = useState(false)
  const [tab, setTab] = useState<ProgressTab>('status')
  /** Settings (for the global speed cap shown on the Options
   *  tab). */
  const [settings, setSettings] = useState<Settings | null>(null)
  /** Options tab: the speed-cap input, seeded from the row when
   *  the tab opens; empty = no per-download cap. */
  const [capInput, setCapInput] = useState('')
  const [capBusy, setCapBusy] = useState(false)
  /** Options tab: the per-download completion options. Loaded
   *  from Rust on mount, persisted on every change — a
   *  reopened window shows the checked state. */
  const [closeAfter, setCloseAfter] = useState(false)
  const [shutdownAfter, setShutdownAfter] = useState(false)
  const closeAfterRef = useRef(false)
  const shutdownAfterRef = useRef(false)
  /** The 30 s shutdown countdown, in seconds left; null when
   *  no shutdown is pending. */
  const [shutdownCountdown, setShutdownCountdown] = useState<number | null>(null)
  const [shutdownBusy, setShutdownBusy] = useState(false)
  /** Completion bookkeeping: the previous polled status (the
   *  completion actions fire only on a transition into
   *  'complete' observed while this window is open — a
   *  download that was already complete when the window opened
   *  does not fire them), and whether this completion already
   *  acted (so the 500 ms poll cannot fire it twice). */
  const prevStatusRef = useRef<string | null>(null)
  const completionActedRef = useRef(false)
  /** Round 66 — the settings, mirrored so the completion
   *  effect below reads the freshest value without a stale
   *  closure (same pattern as the completion-options refs). */
  const settingsRef = useRef<Settings | null>(null)
  /** Round 64 — hold the last known speed / ETA while
   *  downloading: the engine reports an instantaneous
   *  per-tick speed (250 ms ticks), so a tick with no new
   *  bytes carries speed 0 and eta null, and the stats row
   *  flickered between a value and '—'. These refs keep the
   *  last positive speed / ETA and when it was seen; the
   *  render holds them for up to 5 s, then the download is
   *  genuinely idle and the fields show 0 B/s / '—'. */
  const lastSpeedRef = useRef<number | null>(null)
  const lastEtaRef = useRef<number | null>(null)
  const lastSpeedAtRef = useRef(0)

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
    void getDownloadCompletionOptions(downloadId)
      .then((opts) => {
        setCloseAfter(opts.close_after)
        setShutdownAfter(opts.shutdown_after)
      })
      .catch(() => undefined)
  }, [downloadId])

  useEffect(() => {
    closeAfterRef.current = closeAfter
  }, [closeAfter])
  useEffect(() => {
    shutdownAfterRef.current = shutdownAfter
  }, [shutdownAfter])
  useEffect(() => {
    settingsRef.current = settings
  }, [settings])

  // Seed the limiter input when its tab opens, from the freshest
  // row — typing in it must not be clobbered by the 500 ms poll.
  function openTab(next: ProgressTab) {
    setTab(next)
    setActionError(null)
    if (next === 'options' && download) {
      setCapInput(download.speed_limit_kbps ? String(download.speed_limit_kbps) : '')
    }
  }

  /** Persist a completion-options change (checkbox toggle) to
   *  Rust and to local state. A failure surfaces as the
   *  window's action error; the checkbox already moved, so the
   *  user sees the error instead of a silent revert. */
  async function persistOptions(next: DownloadCompletionOptions) {
    setCloseAfter(next.close_after)
    setShutdownAfter(next.shutdown_after)
    try {
      await setDownloadCompletionOptions(downloadId, next)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
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

  // Round 64 — raw (unheld) speed / ETA from the latest
  // snapshot or row. The effect keeps the hold refs fresh: a
  // positive speed (re)starts the 5 s hold window, and a
  // non-null ETA is remembered with it. A status other than
  // 'downloading' clears the hold, so a later run never shows
  // a stale value from the previous one.
  const rawSpeed = live?.speed_bps ?? download?.speed_bps
  const rawEta = live?.eta_seconds ?? download?.eta_seconds
  useEffect(() => {
    if (download?.status !== 'downloading') {
      lastSpeedRef.current = null
      lastEtaRef.current = null
      return
    }
    if (typeof rawSpeed === 'number' && rawSpeed > 0) {
      lastSpeedRef.current = rawSpeed
      lastSpeedAtRef.current = Date.now()
    }
    if (rawEta !== null && rawEta !== undefined) {
      lastEtaRef.current = rawEta
    }
  }, [download?.status, rawSpeed, rawEta])

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

  /** Completion actions (Round 62): when the polled status
   *  flips into 'complete' while this window is open, act on
   *  the checked Options — shut down wins over close (closing
   *  the program would stop the shutdown). A download that was
   *  already complete when the window opened does not fire
   *  them: the options were chosen for a run that already
   *  ended. Round 66: with neither option checked, this window
   *  closes itself — the main window opens the "Download
   *  complete" dialog at the same moment, and the dialog
   *  replaces this window. Suppressed (the dialog turned off
   *  in Settings, or settings not loaded yet) keeps today's
   *  behaviour: the window stays open with Open file /
   *  Open folder in the footer. */
  useEffect(() => {
    const status = download?.status ?? null
    const prev = prevStatusRef.current
    prevStatusRef.current = status
    if (status !== 'complete') {
      completionActedRef.current = false
      return
    }
    if ((prev === 'downloading' || prev === 'paused') && !completionActedRef.current) {
      completionActedRef.current = true
      if (shutdownAfterRef.current) {
        setShutdownCountdown(30)
      } else if (closeAfterRef.current) {
        void exitApp().catch((e: unknown) =>
          setActionError(e instanceof Error ? e.message : String(e)),
        )
      } else if (settingsRef.current?.show_download_complete_dialog) {
        void getCurrentWindow().close()
      }
    }
  }, [download?.status])

  /** The 30 s shutdown countdown: one second at a time, then
   *  the Windows shutdown command. Cancelling (below) clears
   *  the countdown and unchecks the option, so it cannot fire
   *  again for this download. Round 74 (14): Escape cancels it
   *  too, like the Cancel button. */
  useEffect(() => {
    if (shutdownCountdown === null) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void cancelShutdownCountdown()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // `cancelShutdownCountdown` is declared below; it is a
    // hoisted function declaration, so this effect may use it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shutdownCountdown])
  useEffect(() => {
    if (shutdownCountdown === null) return
    if (shutdownCountdown <= 0) {
      setShutdownCountdown(null)
      setShutdownBusy(true)
      void shutdownComputer()
        .catch((e: unknown) => setActionError(e instanceof Error ? e.message : String(e)))
        .finally(() => setShutdownBusy(false))
      return
    }
    const timer = window.setTimeout(
      () => setShutdownCountdown((s) => (s === null ? null : s - 1)),
      1000,
    )
    return () => window.clearTimeout(timer)
  }, [shutdownCountdown])

  async function cancelShutdownCountdown() {
    setShutdownCountdown(null)
    await persistOptions({ close_after: closeAfterRef.current, shutdown_after: false })
  }

  async function runShutdownNow() {
    setShutdownCountdown(null)
    setShutdownBusy(true)
    try {
      await shutdownComputer()
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setShutdownBusy(false)
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

  /** Round 74 (B-UI) — Resume with visible busy feedback: the
   *  Round 73 retest read the progress-window Resume as "did
   *  nothing" while the re-probe ran with no UI change. The
   *  button now shows "Resuming…" until the invoke settles, and
   *  a failure lands in the window's action error instead of
   *  silence. The call itself is the same `resumeDownload` the
   *  toolbar uses, with the same download id — no frontend
   *  defect was found in the wiring (id, state gating, closure);
   *  a resume that still never starts is engine-side (see the
   *  Round 74 report). */
  async function handleResume() {
    setActionError(null)
    setResumeBusy(true)
    try {
      await resumeDownload(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setResumeBusy(false)
    }
  }

  /** Round 63 — on completion the footer offers Open file /
   *  Open folder (the engine commands already exist; the main
   *  window uses them too). A failure surfaces as the window's
   *  action error instead of pretending the file opened.
   *  Round 74 (4) — when the file is gone from disk there is
   *  nothing to open: warn instead of invoking the command. */
  async function handleOpenFile() {
    setActionError(null)
    if (download?.file_missing) {
      setActionError('This file is no longer on disk — Redownload it from the main window to get it again.')
      return
    }
    try {
      await openFile(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenFolder() {
    setActionError(null)
    if (download?.file_missing) {
      setActionError('This file is no longer on disk — Redownload it from the main window to get it again.')
      return
    }
    try {
      await openFolder(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleCancel() {
    setConfirmCancel(false)
    setActionError(null)
    try {
      await cancelDownload(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  /** Speed Limiter (Options tab) — apply the per-download cap,
   *  live: the engine's token bucket picks it up at once (S12).
   *  Empty or 0 removes the cap; the global cap still applies,
   *  so the effective speed is the lower of the two. `value`
   *  overrides the input (the Remove cap button passes '' —
   *  state updates are async, so reading the input there would
   *  use the old value). */
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

  if (!loaded) {
    return (
      <div className="flex h-full flex-col text-text">
        <TitleBar title="Sonic Manager" />
        <p className="p-6 text-sm text-text-muted">Loading download…</p>
      </div>
    )
  }

  if (!download) {
    // Round 74 (3) — the "Download removed" window was a big,
    // mostly-empty window (a heading and one line floating in a
    // 660x480 frame). It is now a compact centred card like the
    // other dialogs: icon + heading + one line + Close.
    return (
      <div className="flex h-full flex-col text-text">
        <TitleBar title="Sonic Manager" />
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <div className="flex h-12 w-12 items-center justify-center rounded-full border-[1.5px] border-border bg-surface-2 text-text-muted">
            <FileX className="h-6 w-6" aria-hidden="true" />
          </div>
          <h1 className="text-lg font-semibold">
            {pollError ? 'Could not load this download' : 'Download removed'}
          </h1>
          <p className="max-w-md text-sm text-text-muted">
            {pollError
              ? `The window could not reach the app: ${pollError} The download itself is not affected.`
              : 'This download is no longer in the list — it was removed from the main window.'}
          </p>
          <div className="mt-3 flex justify-center gap-2">
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
            <Button variant="primary" onClick={() => getCurrentWindow().close()}>
              Close
            </Button>
          </div>
        </div>
      </div>
    )
  }

  const downloaded = live?.downloaded_bytes ?? download.downloaded_bytes
  const size = live?.size_bytes ?? download.size_bytes
  // Round 64 — displayed speed / ETA: while downloading, a
  // zero/null tick holds the last positive value (up to 5 s)
  // instead of blanking to '—'; only a sustained idle shows
  // 0 (rendered as '0 B/s') for speed and null (rendered as
  // '—') for ETA, which needs a speed to exist. Outside
  // 'downloading' the raw values are shown as-is — a paused
  // or finished row shows '—' by design.
  const freshSpeed = live?.speed_bps ?? download.speed_bps
  const freshEta = live?.eta_seconds ?? download.eta_seconds
  const holding =
    download.status === 'downloading' && Date.now() - lastSpeedAtRef.current <= 5_000
  const speed =
    download.status === 'downloading'
      ? freshSpeed > 0
        ? freshSpeed
        : holding && lastSpeedRef.current !== null
          ? lastSpeedRef.current
          : 0
      : freshSpeed
  const eta =
    download.status === 'downloading'
      ? freshEta ?? (holding ? lastEtaRef.current : null)
      : freshEta
  const percent = progressPercent(downloaded, size)
  const segments = live?.segments ?? []
  // The countdown is the engine's: `retry` arrives on the live
  // snapshot (every 500 ms) with seconds left until the next
  // attempt, counted down from the engine's backoff deadline.
  const retry = download.status === 'downloading' ? (live?.retry ?? null) : null
  const resumeCapability =
    download.accept_ranges === true ? 'Yes' : download.accept_ranges === false ? 'No' : 'Unknown'
  const title = percent !== null ? `${percent}% - ${download.filename}` : download.filename

  return (
    <div className="flex h-full flex-col text-text">
      <TitleBar title={title} />
      {/* Round 63 — the tabs sit at the TOP, directly under the
          title bar (like the user's mockup), pinned above the
          scrollable content instead of below it. */}
      <div role="tablist" aria-label="Download details" className="flex shrink-0 gap-1 border-b border-border px-4">
        {(
          [
            ['status', 'Download status'],
            ['options', 'Options'],
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
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
        {backendDead ? (
          <div role="alert" className="flex shrink-0 items-center justify-between gap-2 rounded-md border border-border bg-danger-soft px-3 py-2 text-sm text-danger">
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

        <div className="flex items-start gap-3">
          <div
            aria-hidden="true"
            className="flex h-12 w-12 shrink-0 items-center justify-center rounded-xl bg-[linear-gradient(135deg,#8b20ff,#f02dcd)] shadow-[0_0_18px_rgba(139,32,255,0.55)]"
          >
            <FileDown className="h-6 w-6 text-white" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <h1 className="min-w-0 truncate text-base font-semibold" title={download.filename}>
                {download.filename}
              </h1>
              <StatusBadge status={download.status} fileMissing={download.file_missing} />
            </div>
            <p className="mt-0.5 truncate font-mono text-xs text-text-muted" title={download.url}>
              {download.url}
            </p>
            <p className="mt-0.5 text-xs text-text-muted">
              {download.status === 'downloading'
                ? 'Receiving data…'
                : download.status === 'complete'
                  ? 'Download complete'
                  : statusLabel(download.status)}
            </p>
          </div>
          <div className="shrink-0 text-right" aria-label={`Progress ${percent === null ? 'unknown' : `${percent} percent`}`}>
            <span className="bg-[linear-gradient(90deg,#8b20ff,#00c8ff)] bg-clip-text text-4xl font-bold tabular-nums text-transparent">
              {percent === null ? '—' : `${percent}%`}
            </span>
          </div>
        </div>

        <ProgressBar
          percent={percent}
          gradient
          size="tall"
          status={download.status === 'complete' ? 'complete' : download.status === 'error' ? 'error' : download.status === 'paused' ? 'paused' : 'downloading'}
          label={`Progress of ${download.filename}`}
        />

        <dl className="grid grid-cols-4 gap-2 text-sm">
          <div>
            <dt className="text-text-muted">Downloaded</dt>
            <dd className="mt-0.5 font-mono">
              {size ? `${formatBytes(downloaded)} / ${formatBytes(size)}` : formatBytes(downloaded)}
            </dd>
          </div>
          <div>
            <dt className="text-text-muted">Speed</dt>
            <dd className="mt-0.5 font-mono">
              {download.status === 'downloading' ? formatSpeed(speed, '0 B/s') : formatSpeed(speed)}
            </dd>
          </div>
          <div>
            <dt className="text-text-muted">Time left</dt>
            <dd className="mt-0.5 font-mono">{formatEta(eta)}</dd>
          </div>
          <div>
            <dt className="text-text-muted">Resume</dt>
            <dd className="mt-0.5">{resumeCapability}</dd>
          </div>
        </dl>

        {tab === 'status' ? (
          <>
            {segments.length > 0 ? (
              <div>
                <div className="mb-1 flex items-baseline justify-between gap-2">
                  {/* Round 70 — "Parts (N parts)", not "connections":
                      the engine splits the largest remaining segment
                      whenever a connection frees up (dynamic split),
                      so the part count grows past the connections
                      setting during a download (e.g. 8 connections
                      can show 11 parts) while the concurrent
                      connections never exceed the setting. Calling
                      the count "connections" made the setting look
                      ignored in the Round 69 retest (OVH showed
                      11 with the setting at 8). Round 72 (D) — the
                      live connection count now rides on the event
                      (`active_connections`), so the header shows
                      both: "2 connections · 3/6 completed". */}
                  <p className="text-sm font-semibold">
                    Parts ({segments.length} part{segments.length === 1 ? '' : 's'})
                  </p>
                  <p className="text-xs text-accent">
                    {live?.active_connections ?? 0} connection
                    {(live?.active_connections ?? 0) === 1 ? '' : 's'} ·{' '}
                    {segments.filter((s) => s.status === 'done').length}/{segments.length} completed
                  </p>
                </div>
                <div className="max-h-[140px] overflow-y-auto rounded-md border border-border">
                  <table className="w-full border-collapse text-sm" aria-label="Download parts">
                    <thead className="sticky top-0 bg-surface">
                      <tr className="border-b border-border text-left text-xs text-text-muted">
                        <th scope="col" className="px-3 py-1.5">N.</th>
                        <th scope="col" className="px-3 py-1.5">Downloaded</th>
                        <th scope="col" className="px-3 py-1.5">Info</th>
                      </tr>
                    </thead>
                    <tbody>
                      {segments.map((s) => {
                        const info = partInfo(s, download.status, retry !== null)
                        return (
                          <tr key={s.segment_index} className="border-b border-border last:border-0">
                            <td className="px-3 py-1.5 font-mono text-xs text-text-muted">
                              {s.segment_index + 1}
                            </td>
                            <td className="px-3 py-1.5 font-mono text-xs">{formatBytes(s.downloaded_bytes)}</td>
                            <td className={`px-3 py-1.5 text-xs ${info.className}`}>{info.label}</td>
                          </tr>
                        )
                      })}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}

            {download.quota_paused ? (
              <p className="text-sm text-danger">
                Paused — the download quota is reached. It resumes on its own when the quota window
                rolls over, or sooner with “Reset quota &amp; resume now” in Settings.
              </p>
            ) : null}

            {retry ? (
              <div role="status" className="rounded-md border border-border bg-warning-soft p-3">
                <p className="text-sm font-semibold">
                  {/* Round 72 (C) — a patient tail wait has
                      `max_attempts` 0 (no limit): say so instead
                      of "attempt N of 0". */}
                  Connection lost — retrying in {retry.retry_in_secs}s
                  {retry.max_attempts > 0
                    ? ` (attempt ${retry.attempt + 1} of ${retry.max_attempts})`
                    : ' — retrying until it succeeds'}
                </p>
                <p className="mt-1 text-sm text-text-muted">{retry.error}</p>
                <div className="mt-3 flex gap-2">
                  <Button variant="secondary" onClick={handleRetryNow} disabled={retryBusy}>
                    <RotateCw className="h-4 w-4" aria-hidden="true" /> Retry now
                  </Button>
                  <Button variant="ghost" onClick={() => setConfirmCancel(true)}>
                    Cancel download
                  </Button>
                </div>
              </div>
            ) : null}

            {download.status === 'error' && download.error_message ? (
              <p role="alert" className="text-sm text-danger">
                {download.error_message}
              </p>
            ) : null}
          </>
        ) : null}

        {tab === 'options' ? (
          <div className="text-sm">
            <p className="font-semibold">Speed Limiter</p>
            <p className="mt-1 text-text-muted">
              {download.speed_limit_kbps
                ? `This download is capped at ${download.speed_limit_kbps} KB/s.`
                : 'This download has no per-download cap.'}
              {settings?.speed_limit_kbps
                ? ` The global limit is ${settings.speed_limit_kbps} KB/s — the effective speed is the lower of the two.`
                : ' There is no global speed limit set.'}
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
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

            <p className="mt-5 font-semibold">When this download finishes</p>
            <label className="mt-2 flex cursor-pointer items-center gap-2">
              <input
                type="checkbox"
                checked={closeAfter}
                onChange={(e) => void persistOptions({ close_after: e.target.checked, shutdown_after: shutdownAfter })}
                className="h-4 w-4 accent-[#8b20ff]"
              />
              Close program after download
            </label>
            <label className="mt-2 flex cursor-pointer items-center gap-2">
              <input
                type="checkbox"
                checked={shutdownAfter}
                onChange={(e) => void persistOptions({ close_after: closeAfter, shutdown_after: e.target.checked })}
                className="h-4 w-4 accent-[#8b20ff]"
              />
              Shut down computer after download
            </label>
            <p className="mt-2 text-text-muted">
              Shutting down waits 30 seconds with a Cancel button first. If both boxes are checked,
              shut down wins — closing the program would stop the shutdown.
            </p>
          </div>
        ) : null}
      </div>

      {actionError ? (
        <p role="alert" className="shrink-0 px-4 pb-2 text-sm text-danger">
          {actionError}
        </p>
      ) : null}

      <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-border p-3">
        <p className="mr-auto min-w-0 truncate font-mono text-xs text-text-muted" title={download.save_path}>
          {download.save_path}
        </p>
        {download.status === 'downloading' ? (
          <Button variant="secondary" onClick={handlePause}>
            <Pause className="h-4 w-4" aria-hidden="true" /> Pause
          </Button>
        ) : null}
        {download.status === 'paused' || download.status === 'error' ? (
          <Button variant="primary" onClick={handleResume} loading={resumeBusy}>
            <Play className="h-4 w-4" aria-hidden="true" /> Resume
          </Button>
        ) : null}
        {download.status === 'downloading' || download.status === 'paused' || download.status === 'error' ? (
          <Button variant="danger" onClick={() => setConfirmCancel(true)}>
            Cancel
          </Button>
        ) : null}
        {download.status === 'complete' ? (
          <>
            <Button
              variant="primary"
              disabled={download.file_missing}
              title={
                download.file_missing
                  ? 'The file is no longer on disk — Redownload it from the main window to get it again'
                  : undefined
              }
              onClick={() => void handleOpenFile()}
            >
              <File className="h-4 w-4" aria-hidden="true" /> Open file
            </Button>
            <Button
              variant="secondary"
              disabled={download.file_missing}
              title={
                download.file_missing
                  ? 'The file is no longer on disk — Redownload it from the main window to get it again'
                  : undefined
              }
              onClick={() => void handleOpenFolder()}
            >
              <FolderOpen className="h-4 w-4" aria-hidden="true" /> Open folder
            </Button>
          </>
        ) : null}
      </div>

      {download.status === 'complete' && download.file_missing ? (
        <p role="status" className="shrink-0 px-4 pb-2 text-sm text-danger">
          This file is no longer on disk — Redownload it from the main window to get it again.
        </p>
      ) : null}

      {/* Round 74 (11) — Cancel asks first. Fixed-size, no drag,
          closes on Cancel / X / Escape only. */}
      <ConfirmDialog
        open={confirmCancel}
        title="Cancel this download?"
        message={`“${download.filename}” will be cancelled and removed from the list. The part already downloaded is deleted.`}
        confirmLabel="Cancel download"
        cancelLabel="Keep downloading"
        onConfirm={() => void handleCancel()}
        onCancel={() => setConfirmCancel(false)}
      />

      {shutdownCountdown !== null ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="shutdown-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
        >
          <div className="w-full max-w-sm rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <div className="mb-2 flex items-center gap-3">
              <Power className="h-6 w-6 text-danger" aria-hidden="true" />
              <h2 id="shutdown-title" className="text-lg font-semibold">
                Shut down the computer?
              </h2>
            </div>
            <p className="text-base text-text-muted">
              “{download.filename}” finished downloading.
            </p>
            <p className="mt-4 text-center text-3xl font-bold tabular-nums" aria-live="polite">
              {shutdownCountdown}s
            </p>
            <p className="text-center text-sm text-text-muted">
              Shutting down when the countdown ends.
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button variant="ghost" loading={shutdownBusy} onClick={() => void cancelShutdownCountdown()}>
                Cancel
              </Button>
              <Button variant="danger" loading={shutdownBusy} onClick={() => void runShutdownNow()}>
                Shut down now
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
