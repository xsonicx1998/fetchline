import { Download, Link2 } from 'lucide-react'
import { formatBytes, formatDateTime, formatEta, formatSpeed, progressPercent } from '../lib/format'
import type { DownloadRow, QueueRow } from '../lib/types'
import { ProgressRing } from './ProgressRing'
import { StatusBadge } from './StatusBadge'

interface DownloadTableProps {
  downloads: DownloadRow[]
  queues: QueueRow[]
  selectedId: string | null
  onSelect: (id: string) => void
  onOpenProgress: (id: string) => void
  onAdd: () => void
}

/** What the Description column shows. Errors first (the row's own
 *  message), then the two states that need explaining (quota pause,
 *  a scan threat); ordinary rows have nothing to say. */
function descriptionFor(d: DownloadRow): string {
  if (d.status === 'error' && d.error_message) return d.error_message
  if (d.quota_paused) return 'Paused — the download quota is reached'
  if (d.scan_status === 'infected') {
    return `Threat found${d.scan_detail ? `: ${d.scan_detail}` : ''}`
  }
  return ''
}

/** S01 — the main download table (components.md: DownloadTable),
 *  IDM's columns: File Name, Q (queue), Size, Status, Time left,
 *  Transfer rate, Last Try, Description. The progress ring lives
 *  in the File Name cell and the percent shows next to the status
 *  while a download runs, so nothing the old table showed is lost
 *  (the old Category column is one click away in the sidebar
 *  filter; the old progress bar is the ring). Empty, filled,
 *  downloading, paused, complete and error states. Rows are
 *  draggable onto a category or a queue in the sidebar. */
export function DownloadTable({ downloads, queues, selectedId, onSelect, onOpenProgress, onAdd }: DownloadTableProps) {
  const queueName = (id: string | null) => (id === null ? '—' : (queues.find((q) => q.id === id)?.name ?? '—'))
  if (downloads.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
        <Download className="h-10 w-10 text-text-faint" aria-hidden="true" />
        <p className="text-lg font-semibold">No downloads yet</p>
        <p className="max-w-md text-base text-text-muted">
          Paste a link to start your first download. It will show up here with live speed and time left.
        </p>
        <button
          type="button"
          onClick={onAdd}
          className="inline-flex h-10 items-center gap-2 rounded-md bg-accent px-4 font-semibold text-on-accent shadow-glow hover:bg-accent-hover"
        >
          <Link2 className="h-4 w-4" aria-hidden="true" />
          Add a link
        </button>
      </div>
    )
  }

  return (
    <table className="w-full border-collapse text-base" aria-label="Downloads">
      <thead className="sticky top-0 bg-surface">
        <tr className="border-b border-border text-left text-xs font-semibold uppercase text-text-muted">
          <th scope="col" className="px-3 py-2">File Name</th>
          <th scope="col" className="px-3 py-2" title="Queue">Q</th>
          <th scope="col" className="px-3 py-2">Size</th>
          <th scope="col" className="px-3 py-2">Status</th>
          <th scope="col" className="px-3 py-2">Time left</th>
          <th scope="col" className="px-3 py-2">Transfer rate</th>
          <th scope="col" className="px-3 py-2">Last Try</th>
          <th scope="col" className="px-3 py-2">Description</th>
        </tr>
      </thead>
      <tbody>
        {downloads.map((d) => {
          const percent = progressPercent(d.downloaded_bytes, d.size_bytes)
          const selected = d.id === selectedId
          const description = descriptionFor(d)
          return (
            <tr
              key={d.id}
              draggable
              onDragStart={(e) => {
                e.dataTransfer.setData('text/plain', JSON.stringify({ type: 'download', id: d.id }))
                e.dataTransfer.effectAllowed = 'move'
              }}
              onClick={() => onSelect(d.id)}
              onDoubleClick={() => onOpenProgress(d.id)}
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === 'Enter') onOpenProgress(d.id)
              }}
              className={`h-10 cursor-pointer border-b border-border ${selected ? 'bg-accent-soft' : 'hover:bg-surface'}`}
              aria-selected={selected}
            >
              <td className="max-w-0 px-3 py-2" title={d.filename}>
                <span className="flex items-center gap-2">
                  <ProgressRing
                    percent={percent}
                    status={d.status === 'complete' ? 'complete' : d.status === 'error' ? 'error' : d.status === 'paused' ? 'paused' : 'downloading'}
                    size={24}
                    label={`Progress of ${d.filename}`}
                  />
                  <span className="min-w-0 flex-1 truncate font-medium">{d.filename}</span>
                </span>
              </td>
              <td className="whitespace-nowrap px-3 py-2 text-sm text-text-muted" title={queueName(d.queue_id)}>
                {queueName(d.queue_id)}
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {formatBytes(d.size_bytes)}
              </td>
              <td className="whitespace-nowrap px-3 py-2">
                <span className="flex items-center gap-1.5">
                  <StatusBadge status={d.status} />
                  {percent !== null && (d.status === 'downloading' || d.status === 'paused') ? (
                    <span className="font-mono text-xs text-text-muted">{percent}%</span>
                  ) : null}
                </span>
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {d.status === 'downloading' ? formatEta(d.eta_seconds) : '—'}
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {d.status === 'downloading' ? formatSpeed(d.speed_bps) : '—'}
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {formatDateTime(d.updated_at)}
              </td>
              <td className="max-w-0 truncate px-3 py-2 text-sm text-text-muted" title={description || undefined}>
                {description}
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}
