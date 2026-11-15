import { Download, Link2 } from 'lucide-react'
import { formatBytes, formatEta, formatSpeed, progressPercent } from '../lib/format'
import type { CategoryRow, DownloadRow } from '../lib/types'
import { ProgressBar } from './ProgressBar'
import { StatusBadge } from './StatusBadge'

interface DownloadTableProps {
  downloads: DownloadRow[]
  categories: CategoryRow[]
  selectedId: string | null
  onSelect: (id: string) => void
  onOpenProgress: (id: string) => void
  onAdd: () => void
}

/** S01 — the main download table (components.md: DownloadTable).
 *  Empty, filled, downloading, paused, complete and error states.
 *  Rows are draggable onto a category or a queue in the sidebar. */
export function DownloadTable({ downloads, categories, selectedId, onSelect, onOpenProgress, onAdd }: DownloadTableProps) {
  const categoryName = (id: string) => categories.find((c) => c.id === id)?.name ?? '—'
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
          className="inline-flex h-10 items-center gap-2 rounded-md bg-accent px-4 font-semibold text-on-accent hover:bg-accent-hover"
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
          <th scope="col" className="px-3 py-2">Name</th>
          <th scope="col" className="px-3 py-2">Category</th>
          <th scope="col" className="px-3 py-2">Size</th>
          <th scope="col" className="px-3 py-2">Status</th>
          <th scope="col" className="px-3 py-2">Progress</th>
          <th scope="col" className="px-3 py-2">Speed</th>
          <th scope="col" className="px-3 py-2">Time left</th>
        </tr>
      </thead>
      <tbody>
        {downloads.map((d) => {
          const percent = progressPercent(d.downloaded_bytes, d.size_bytes)
          const selected = d.id === selectedId
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
              <td className="max-w-0 truncate px-3 py-2 font-medium" title={d.filename}>
                {d.filename}
                {d.status === 'error' && d.error_message ? (
                  <span className="block truncate text-xs text-danger">{d.error_message}</span>
                ) : null}
              </td>
              <td className="whitespace-nowrap px-3 py-2 text-sm text-text-muted">
                {categoryName(d.category_id)}
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {formatBytes(d.size_bytes)}
              </td>
              <td className="whitespace-nowrap px-3 py-2">
                <StatusBadge status={d.status} />
              </td>
              <td className="px-3 py-2">
                <ProgressBar
                  percent={percent}
                  status={d.status === 'complete' ? 'complete' : d.status === 'error' ? 'error' : d.status === 'paused' ? 'paused' : 'downloading'}
                  label={`Progress of ${d.filename}`}
                />
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {d.status === 'downloading' ? formatSpeed(d.speed_bps) : '—'}
              </td>
              <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                {d.status === 'downloading' ? formatEta(d.eta_seconds) : '—'}
              </td>
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}
