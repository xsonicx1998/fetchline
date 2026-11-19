import { useEffect } from 'react'
import { X } from 'lucide-react'
import { formatBytes, formatDateTime, formatSpeed, statusLabel } from '../lib/format'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { Button } from './Button'

interface DownloadPropertiesDialogProps {
  open: boolean
  download: DownloadRow | null
  categories: CategoryRow[]
  queues: QueueRow[]
  onClose: () => void
}

/** Round 65 — Properties (right-click menu, like IDM): a
 *  read-only sheet of the download's details — name, link,
 *  state, sizes, speed, where it is saved, when it was added
 *  and when it finished. Nothing here edits anything; it is
 *  the file's identity card. */
export function DownloadPropertiesDialog({
  open,
  download,
  categories,
  queues,
  onClose,
}: DownloadPropertiesDialogProps) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open || !download) return null
  const d = download
  const percent =
    d.size_bytes !== null && d.size_bytes > 0
      ? Math.min(100, Math.round((d.downloaded_bytes / d.size_bytes) * 100))
      : null
  const category = categories.find((c) => c.id === d.category_id)?.name ?? '—'
  const queue = d.queue_id === null ? '—' : (queues.find((q) => q.id === d.queue_id)?.name ?? '—')

  const rows: Array<[string, string]> = [
    ['File name', d.filename],
    ['URL', d.final_url ?? d.url],
    [
      'Status',
      `${statusLabel(d.status)}${percent !== null ? ` — ${percent}%` : ''}${d.error_message ? ` (${d.error_message})` : ''}`,
    ],
    ['Size', formatBytes(d.size_bytes)],
    ['Downloaded', formatBytes(d.downloaded_bytes)],
    ['Speed', d.status === 'downloading' ? formatSpeed(d.speed_bps, '0 B/s') : '—'],
    ['Saved to', d.save_path],
    ['Category', category],
    ['Queue', queue],
    ['Added', formatDateTime(d.created_at)],
    ['Completed', formatDateTime(d.completed_at)],
  ]

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={`Properties of ${d.filename}`}
      className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="flex items-start justify-between gap-4">
          <h2 className="min-w-0 truncate text-lg font-semibold" title={d.filename}>
            {d.filename}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="shrink-0 rounded-md p-1 text-text-muted transition-colors hover:bg-surface-2 hover:text-text"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
        <dl className="mt-4 space-y-2 text-sm">
          {rows.map(([label, value]) => (
            <div key={label} className="grid grid-cols-[110px_1fr] gap-2">
              <dt className="shrink-0 text-text-muted">{label}</dt>
              <dd className="min-w-0 break-words text-text" title={value}>
                {value}
              </dd>
            </div>
          ))}
        </dl>
        <div className="mt-6 flex justify-end">
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>
    </div>
  )
}
