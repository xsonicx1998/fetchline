import { useEffect, useRef, useState } from 'react'
import { formatBytes, formatDateTime, formatEta, formatSpeed, progressPercent } from '../lib/format'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { PackIcon } from './PackIcon'
import { ProgressRing } from './ProgressRing'
import { StatusBadge } from './StatusBadge'

interface DownloadTableProps {
  downloads: DownloadRow[]
  queues: QueueRow[]
  categories: CategoryRow[]
  selectedId: string | null
  onSelect: (id: string) => void
  onOpenProgress: (id: string) => void
  onAdd: () => void
  /** Right-click menu: open the finished file's folder. */
  onOpenFolder: (id: string) => void
  /** Right-click menu: move the selected download to a category. */
  onMoveToCategory: (id: string, categoryId: string) => void
  /** Right-click menu: refresh the address of a paused/failed
   *  download (Round 51) — opens the dialog with the new link. */
  onRefreshAddress: (download: DownloadRow) => void
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
 *  draggable onto a category or a queue in the sidebar.
 *  Right-click opens the row menu (Round 45): Open folder and
 *  Move to category used to live in the menu bar's File and
 *  Downloads menus — the menu bar is gone, so they live here. */
export function DownloadTable({ downloads, queues, categories, selectedId, onSelect, onOpenProgress, onAdd, onOpenFolder, onMoveToCategory, onRefreshAddress }: DownloadTableProps) {
  const queueName = (id: string | null) => (id === null ? '—' : (queues.find((q) => q.id === id)?.name ?? '—'))
  const [menu, setMenu] = useState<{ x: number; y: number; download: DownloadRow } | null>(null)
  const menuRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (menu === null) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setMenu(null)
    }
    function onPointerDown(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenu(null)
    }
    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('pointerdown', onPointerDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('pointerdown', onPointerDown)
    }
  }, [menu])

  function closeMenu(action: () => void) {
    return () => {
      setMenu(null)
      action()
    }
  }

  if (downloads.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-6 text-center">
        <PackIcon
          name="download"
          className="icon-glow-white h-16 w-16 text-white"
        />
        <p className="text-xl font-bold">No downloads yet</p>
        <p className="max-w-md text-[15px] text-text-muted">
          Paste a link to start your first download. It will show up here with live speed and time left.
        </p>
        <button
          type="button"
          onClick={onAdd}
          className="inline-flex h-11 items-center gap-2 rounded-lg bg-[linear-gradient(90deg,#8b20ff,#2563eb)] px-5 text-[15px] font-semibold text-on-accent shadow-glow hover:brightness-110"
        >
          <PackIcon name="link" className="h-5 w-5" />
          Add a link
        </button>
      </div>
    )
  }

  return (
    <div className="relative">
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
                onContextMenu={(e) => {
                  e.preventDefault()
                  onSelect(d.id)
                  setMenu({ x: e.clientX, y: e.clientY, download: d })
                }}
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
      {menu ? (
        <div
          ref={menuRef}
          role="menu"
          aria-label="Download actions"
          className="fixed z-20 min-w-60 rounded-md border border-border bg-surface-raised p-1 shadow-pop"
          style={{ left: Math.min(menu.x, window.innerWidth - 260), top: Math.min(menu.y, window.innerHeight - 240) }}
        >
          <button
            type="button"
            role="menuitem"
            onClick={closeMenu(() => onOpenProgress(menu.download.id))}
            className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2"
          >
            Open progress window
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={menu.download.status !== 'complete'}
            onClick={closeMenu(() => onOpenFolder(menu.download.id))}
            className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2 disabled:opacity-45"
          >
            Open folder
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={menu.download.status !== 'paused' && menu.download.status !== 'error'}
            title={
              menu.download.status === 'paused' || menu.download.status === 'error'
                ? 'Replace an expired link and resume from the downloaded bytes'
                : 'Only a paused or failed download can get a new address'
            }
            onClick={closeMenu(() => onRefreshAddress(menu.download))}
            className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2 disabled:opacity-45"
          >
            Refresh download address
          </button>
          <div role="separator" className="my-1 h-px bg-border" aria-hidden="true" />
          <p className="px-3 pb-1 pt-2 text-xs font-semibold uppercase text-text-muted">Move to category</p>
          {categories.map((c) => (
            <button
              key={c.id}
              type="button"
              role="menuitem"
              onClick={closeMenu(() => onMoveToCategory(menu.download.id, c.id))}
              className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2"
            >
              {c.name}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  )
}
