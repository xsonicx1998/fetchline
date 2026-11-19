import { statusLabel } from '../lib/format'
import type { DownloadStatus } from '../lib/types'

const tones: Record<DownloadStatus, string> = {
  queued: 'bg-surface-2 text-text-muted',
  downloading: 'bg-accent-soft text-accent',
  paused: 'bg-surface-2 text-text-muted',
  complete: 'bg-success-soft text-success',
  error: 'bg-danger-soft text-danger',
  deleted: 'bg-surface-2 text-text-muted',
}

/** Status badge primitive (components.md). Text, never colour-only.
 *
 *  Round 74 (5): every pill is the same size — one fixed
 *  min-width (wide enough for the longest label, "Downloading")
 *  and centred text, so Finished / Failed / Paused never change
 *  width as the status changes. Round 74 (4): a Finished row
 *  whose file is gone from disk reads "File missing" in the
 *  danger tone instead of pretending it can be opened. */
export function StatusBadge({
  status,
  fileMissing = false,
}: {
  status: DownloadStatus
  fileMissing?: boolean
}) {
  const missing = status === 'complete' && fileMissing
  return (
    <span
      className={`inline-flex min-w-[96px] items-center justify-center whitespace-nowrap rounded-pill px-2 py-0.5 text-xs font-semibold ${
        missing ? 'bg-danger-soft text-danger' : tones[status]
      }`}
    >
      {missing ? 'File missing' : statusLabel(status)}
    </span>
  )
}
