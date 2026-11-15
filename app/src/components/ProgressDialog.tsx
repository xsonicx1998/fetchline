import { useEffect, useState } from 'react'
import { FileWarning, FolderOpen, Pause, Play, RotateCw, ShieldCheck, ShieldX, X } from 'lucide-react'
import { formatBytes, formatEta, formatSpeed, progressPercent } from '../lib/format'
import { deleteDownload, openFile, openFolder, quarantineDownload, retryNow, runAntivirusScan } from '../lib/api'
import { recordSpeedSample, speedGraphPoints, speedGraphPolyline } from '../lib/speedGraph'
import type { SpeedSample } from '../lib/speedGraph'
import type { DownloadRow, ProgressEvent, ScanResult } from '../lib/types'
import { Button } from './Button'
import { ProgressBar } from './ProgressBar'
import { StatusBadge } from './StatusBadge'

interface ProgressDialogProps {
  download: DownloadRow | null
  live: ProgressEvent | null
  onClose: () => void
  onPause: () => void
  onResume: () => void
  onCancel: () => void
}

/** S03 — download progress dialog (components.md: Dialog, ProgressBar,
 *  SegmentBar, StatusBadge). Live speed, ETA and one bar per segment,
 *  plus, since Milestone 5: a 60 s speed graph fed by the live
 *  progress events, a retry countdown driven by the engine's own
 *  retry state (never a UI-only timer), and "Open folder" on
 *  completion. States: downloading (incl. waiting for retry),
 *  paused, complete, error. */
export function ProgressDialog({ download, live, onClose, onPause, onResume, onCancel }: ProgressDialogProps) {
  // Hooks run before the early return, so the dialog can be closed
  // and reopened for another download without stale samples.
  const downloadId = download?.id ?? null
  const [samples, setSamples] = useState<SpeedSample[]>([])
  const [actionError, setActionError] = useState<string | null>(null)
  const [retryBusy, setRetryBusy] = useState(false)
  const [folderBusy, setFolderBusy] = useState(false)
  const [scanBusy, setScanBusy] = useState<string | null>(null)

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

  if (!download) return null

  // Captured after the early return: closures below cannot use the
  // narrowed prop directly (TS resets narrowing inside closures).
  const downloadIdResolved = download.id

  async function handleRetryNow() {
    setRetryBusy(true)
    setActionError(null)
    try {
      await retryNow(downloadIdResolved)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setRetryBusy(false)
    }
  }

  async function handleOpenFolder() {
    setFolderBusy(true)
    setActionError(null)
    try {
      await openFolder(downloadIdResolved)
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
      await openFile(downloadIdResolved)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** S14 — scan the finished file again with the configured
   *  scanner. The result arrives on the live event and on the
   *  download row, so the dialog updates without a refresh. */
  async function handleRescan() {
    setScanBusy('rescan')
    setActionError(null)
    try {
      await runAntivirusScan(downloadIdResolved)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  /** S14 — quarantine is an explicit user choice: the engine
   *  moves the flagged file into a Quarantine folder. The dialog
   *  then closes — the file no longer lives at the old path. */
  async function handleQuarantine() {
    setScanBusy('quarantine')
    setActionError(null)
    try {
      await quarantineDownload(downloadIdResolved)
      onClose()
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
      await deleteDownload(downloadIdResolved, true)
      onClose()
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    } finally {
      setScanBusy(null)
    }
  }

  const downloaded = live?.downloaded_bytes ?? download.downloaded_bytes
  const size = live?.size_bytes ?? download.size_bytes
  const speed = live?.speed_bps ?? download.speed_bps
  const eta = live?.eta_seconds ?? download.eta_seconds
  const percent = progressPercent(downloaded, size)
  const segments = live?.segments ?? []
  // The countdown is the engine's: `retry` arrives on the live
  // progress event (every 250 ms) with seconds left until the next
  // attempt, counted down from the engine's backoff deadline.
  const retry = download.status === 'downloading' ? (live?.retry ?? null) : null
  // S14 — the scan result: the live event carries it once the
  // detached scan finishes; otherwise the download row (refreshed
  // every second by the main window) carries the stored result.
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

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="progress-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h2 id="progress-title" className="truncate text-lg font-semibold" title={download.filename}>
              {download.filename}
            </h2>
            <p className="mt-1 text-sm text-text-muted">
              {formatBytes(downloaded)} of {formatBytes(size)} · {formatSpeed(speed)} · {formatEta(eta)}
              {download.speed_limit_kbps ? ` · capped at ${download.speed_limit_kbps} KB/s` : ''}
            </p>
            {download.quota_paused ? (
              <p className="mt-1 text-sm text-danger">
                Paused — the download quota is reached. It resumes on its own when the quota window
                rolls over, or sooner with “Reset quota &amp; resume now” in Settings.
              </p>
            ) : null}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2"
          >
            <X className="h-5 w-5" aria-hidden="true" />
          </button>
        </div>

        <div className="mb-2 flex items-center gap-3">
          <StatusBadge status={download.status} />
          <span className="font-mono text-sm text-text-muted">{download.url}</span>
        </div>

        <ProgressBar
          percent={percent}
          size="tall"
          status={download.status === 'complete' ? 'complete' : download.status === 'error' ? 'error' : download.status === 'paused' ? 'paused' : 'downloading'}
          label={`Progress of ${download.filename}`}
        />

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
              <Button variant="ghost" onClick={onCancel}>
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

        {actionError ? (
          <p role="alert" className="mt-4 text-sm text-danger">
            {actionError}
          </p>
        ) : null}

        <div className="mt-6 flex justify-end gap-2">
          {download.status === 'downloading' ? (
            <Button variant="secondary" onClick={onPause}>
              <Pause className="h-4 w-4" aria-hidden="true" /> Pause
            </Button>
          ) : null}
          {download.status === 'paused' || download.status === 'error' ? (
            <Button variant="primary" onClick={onResume}>
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
            <Button variant="danger" onClick={onCancel}>
              Cancel
            </Button>
          ) : null}
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>
    </div>
  )
}
