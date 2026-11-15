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

/** Status badge primitive (components.md). Text, never colour-only. */
export function StatusBadge({ status }: { status: DownloadStatus }) {
  return (
    <span className={`inline-flex items-center rounded-pill px-2 py-0.5 text-xs font-semibold ${tones[status]}`}>
      {statusLabel(status)}
    </span>
  )
}
