import { useEffect, useState } from 'react'
import { ChevronDown, ChevronUp, FileWarning, FolderOpen, Pause, Play, RotateCw, ShieldCheck, ShieldX, X } from 'lucide-react'
import { formatBytes, formatEta, formatSpeed, progressPercent, statusLabel } from '../lib/format'
import {
  cancelDownload,
  deleteDownload,
  getProgressSnapshot,
  getSettings,
  listDownloads,
  openFile,
  openFolder,
  pauseDownload,
  quarantineDownload,
  resumeDownload,
  retryNow,
  runAntivirusScan,
  setProgressWindowCompact,
} from '../lib/api'
import { recordSpeedSample, speedGraphPoints, speedGraphPolyline } from '../lib/speedGraph'
import type { SpeedSample } from '../lib/speedGraph'
import type { DownloadRow, ProgressEvent, ScanResult } from '../lib/types'
import { Button } from './Button'
import { ProgressBar } from './ProgressBar'
import { StatusBadge } from './StatusBadge'

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
 *  500 ms. Polling is deliberate — the frontend
 *  `download-progress` listener never fires in the built app
 *  (BUG-044: no Tauri capability grants `plugin:event|listen`),
 *  so the snapshot is the window's live feed. The 500 ms poll is
 *  also what feeds the speed graph, so the graph actually builds
 *  here (in the old overlay it never got a sample in production).
 *  The window title is updated from Rust, in the progress-event
 *  forwarder — the frontend cannot set its own title for the
 *  same capability reason. */
export function ProgressWindow({ downloadId }: { downloadId: string }) {
  const [download, setDownload] = useState<DownloadRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [live, setLive] = useState<ProgressEvent | null>(null)
  const [samples, setSamples] = useState<SpeedSample[]>([])
  const [compact, setCompact] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [retryBusy, setRetryBusy] = useState(false)
  const [folderBusy, setFolderBusy] = useState(false)
  const [scanBusy, setScanBusy] = useState<string | null>(null)

  // The window runs the same bundle as the main window, so it
  // applies the stored theme itself on mount.
  useEffect(() => {
    void getSettings()
      .then((s) => document.documentElement.setAttribute('data-theme', s.theme))
      .catch(() => undefined)
  }, [])

  useEffect(() => {
    let stopped = false
    async function poll() {
      try {
        const rows = await listDownloads()
        if (stopped) return
        const row = rows.find((d) => d.id === downloadId) ?? null
        setDownload(row)
        setLoaded(true)
        if (row) {
          const snap = await getProgressSnapshot(downloadId)
          if (stopped) return
          if (snap && snap.id === downloadId) setLive(snap)
        }
      } catch {
        // Outside the desktop app the bridge throws; the window
        // simply shows its empty state.
        setLoaded(true)
      }
    }
    void poll()
    const timer = window.setInterval(() => void poll(), 500)
    return () => {
      stopped = true
      window.clearInterval(timer)
    }
  }, [downloadId])

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

  if (!loaded) {
    return (
      <div className="flex h-full flex-col bg-bg p-6 text-text">
        <p className="text-sm text-text-muted">Loading download…</p>
      </div>
    )
  }

  if (!download) {
    return (
      <div className="flex h-full flex-col bg-bg p-6 text-text">
        <h1 className="text-lg font-semibold">Download removed</h1>
        <p className="mt-2 text-sm text-text-muted">
          This download is no longer in the list — it was removed from the main window.
        </p>
        <div className="mt-6 flex justify-end">
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
    <div className="flex h-full flex-col bg-bg p-6 text-text">
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
        <button
          type="button"
          onClick={() => window.close()}
          aria-label="Close"
          className="rounded-md p-1 text-text-muted hover:bg-surface-2"
        >
          <X className="h-5 w-5" aria-hidden="true" />
        </button>
      </div>

      <div className="mb-4 flex items-center gap-3">
        <StatusBadge status={download.status} />
        <span className="font-mono text-sm text-text-muted">{download.url}</span>
      </div>

      <ProgressBar
        percent={percent}
        size="tall"
        status={download.status === 'complete' ? 'complete' : download.status === 'error' ? 'error' : download.status === 'paused' ? 'paused' : 'downloading'}
        label={`Progress of ${download.filename}`}
      />

      <dl className="mt-4 grid grid-cols-2 gap-x-4 gap-y-2 text-sm">
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

      {!compact ? (
        <>
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
              {scan?.status === 'clean' ? (
                <p role="status" className="mt-2 flex items-start gap-2 text-sm">
                  <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-success" aria-hidden="true" />
                  <span>Antivirus scan: clean — no threat found.</span>
                </p>
              ) : scan?.status === 'infected' ? (
                <div role="alert" className="mt-2 flex items-start gap-2 text-sm text-danger">
                  <ShieldX className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                  <span>
                    Antivirus scan: threat found
                    {scan.detail ? ` — ${scan.detail}` : '.'} The file was not opened. Delete it or
                    quarantine it below.
                  </span>
                </div>
              ) : scan?.status === 'scan_failed' ? (
                <p role="status" className="mt-2 flex items-start gap-2 text-sm text-warning">
                  <FileWarning className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                  <span>Antivirus scan failed{scan.detail ? ` — ${scan.detail}` : '.'}</span>
                </p>
              ) : scan?.status === 'timed_out' ? (
                <p role="status" className="mt-2 flex items-start gap-2 text-sm text-warning">
                  <FileWarning className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                  <span>Antivirus scan timed out{scan.detail ? ` — ${scan.detail}` : '.'}</span>
                </p>
              ) : scan?.status === 'scanning' ? (
                <p role="status" className="mt-2 text-sm text-text-muted">
                  Scanning with your antivirus scanner…
                </p>
              ) : scan?.status === 'not_configured' ? (
                <p className="mt-2 text-sm text-text-muted">
                  No antivirus scanner is configured — the file was not scanned. Set one up in
                  Settings &gt; Downloads.
                </p>
              ) : (
                <p className="mt-2 text-sm text-text-muted">Scan result not available yet.</p>
              )}
            </div>
          ) : null}
        </>
      ) : null}

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
            {scan && scan.status !== 'scanning' ? (
              <Button variant="secondary" onClick={handleRescan} disabled={scanBusy !== null}>
                <ShieldCheck className="h-4 w-4" aria-hidden="true" /> Scan again
              </Button>
            ) : null}
            {threatFound ? (
              <>
                <Button variant="secondary" onClick={handleQuarantine} disabled={scanBusy !== null}>
                  Quarantine
                </Button>
                <Button variant="danger" onClick={handleDeleteFile} disabled={scanBusy !== null}>
                  Delete file
                </Button>
              </>
            ) : null}
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
