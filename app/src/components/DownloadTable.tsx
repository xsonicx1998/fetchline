import { useEffect, useRef, useState } from 'react'
import { formatBytes, formatEta, formatSpeed, progressPercent } from '../lib/format'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { PackIcon } from './PackIcon'
import { ProgressBar } from './ProgressBar'
import { StatusBadge } from './StatusBadge'

interface DownloadTableProps {
  downloads: DownloadRow[]
  categories: CategoryRow[]
  /** Round 65 — queues, for the right-click menu's "Move to
   *  queue" section. */
  queues: QueueRow[]
  selectedId: string | null
  onSelect: (id: string) => void
  onOpenProgress: (id: string) => void
  onAdd: () => void
  /** Right-click menu: open the finished file. */
  onOpenFile: (id: string) => void
  /** Right-click menu: Windows "Open with" dialog for the file. */
  onOpenWith: (id: string) => void
  /** Right-click menu: open the finished file's folder. */
  onOpenFolder: (id: string) => void
  /** Right-click menu: start the same URL over from scratch. */
  onRedownload: (download: DownloadRow) => void
  /** Right-click menu: resume a paused/failed download. */
  onResume: (id: string) => void
  /** Right-click menu: pause a running download. */
  onPause: (id: string) => void
  /** Right-click menu: move the selected download to a category. */
  onMoveToCategory: (id: string, categoryId: string) => void
  /** Right-click menu: refresh the address of a paused/failed
   *  download (Round 51) — opens the dialog with the new link. */
  onRefreshAddress: (download: DownloadRow) => void
  /** Right-click menu: remove the row (the confirm setting
   *  decides whether a dialog runs first). */
  onRemove: (download: DownloadRow) => void
  /** Right-click menu: move the download into a queue. */
  onMoveToQueue: (id: string, queueId: string) => void
  /** Right-click menu: take the download out of its queue. */
  onDeleteFromQueue: (id: string) => void
  /** Right-click menu: show the file's details. */
  onShowProperties: (download: DownloadRow) => void
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
  // Round 71 (D4) — a Finished row whose file was deleted
  // outside the app must say so, not pretend it can be opened.
  if (d.status === 'complete' && d.file_missing) return 'File missing from disk'
  return ''
}

/** S01 — the main download table (components.md: DownloadTable),
 *  Round 62 columns: File Name, Size, Status, Transfer rate,
 *  Time left. A downloading / paused row shows its gradient
 *  progress bar inline under the filename and its percent next
 *  to the status pill, so the live state is in the row itself.
 *  The old Q / Last Try / Description columns are gone (the
 *  rejected IDM-heavy variant had them); the description text
 *  — error message, quota pause, scan threat — survives as the
 *  status cell's tooltip. Empty, filled, downloading, paused,
 *  complete and error states. Rows are draggable onto a category
 *  or a queue in the sidebar. Right-click opens the row menu,
 *  like IDM's (Round 65): Open, Open with..., Open folder, Open
 *  progress window, Redownload, Resume Download, Stop Download,
 *  Refresh download address, Remove, Move to queue, Delete from
 *  queue, Properties, Move to category — every item enabled or
 *  disabled by the row's state (downloaded or not, paused,
 *  failed, in a queue). */
export function DownloadTable({
  downloads,
  categories,
  queues,
  selectedId,
  onSelect,
  onOpenProgress,
  onAdd,
  onOpenFile,
  onOpenWith,
  onOpenFolder,
  onRedownload,
  onResume,
  onPause,
  onMoveToCategory,
  onRefreshAddress,
  onRemove,
  onMoveToQueue,
  onDeleteFromQueue,
  onShowProperties,
}: DownloadTableProps) {
  const [menu, setMenu] = useState<{ x: number; y: number; download: DownloadRow } | null>(null)
  const menuRef = useRef<HTMLDivElement | null>(null)
  /** Round 65 — the "Move to queue" section of the row menu is
   *  collapsed until the user opens it, like a submenu; it
   *  resets whenever the menu opens on another row. */
  const [queueSubOpen, setQueueSubOpen] = useState(false)
  /** Round 64 — hold the last known speed / ETA per
   *  downloading row: the engine reports an instantaneous
   *  per-tick speed, so a tick with no new bytes carries
   *  speed 0 and eta null, and the Transfer rate / Time left
   *  columns flickered between a value and '—'. Same 5 s
   *  hold as the progress window; a sustained idle shows
   *  0 B/s / '—'. A row that stops downloading drops its
   *  entry, so a later run never shows a stale value. */
  const heldRef = useRef(new Map<string, { speed: number; eta: number | null; at: number }>())

  useEffect(() => {
    const now = Date.now()
    for (const d of downloads) {
      if (d.status !== 'downloading') {
        heldRef.current.delete(d.id)
        continue
      }
      if (d.speed_bps > 0) {
        heldRef.current.set(d.id, { speed: d.speed_bps, eta: d.eta_seconds, at: now })
      }
    }
  }, [downloads])

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
    <div className="relative bg-[#0B0F24]">
      <table className="w-full border-collapse text-base" aria-label="Downloads">
        <thead className="sticky top-0 bg-surface">
          <tr className="border-b border-border text-left text-xs font-semibold uppercase text-text-muted">
            <th scope="col" className="px-3 py-2">File Name</th>
            <th scope="col" className="px-3 py-2">Size</th>
            <th scope="col" className="px-3 py-2">Status</th>
            <th scope="col" className="px-3 py-2">Transfer rate</th>
            <th scope="col" className="px-3 py-2">Time left</th>
          </tr>
        </thead>
        <tbody>
          {downloads.map((d) => {
            const percent = progressPercent(d.downloaded_bytes, d.size_bytes)
            const selected = d.id === selectedId
            const description = descriptionFor(d)
            const showBar = percent !== null && (d.status === 'downloading' || d.status === 'paused')
            // Round 64 — displayed rate / ETA: while downloading,
            // a zero/null tick holds the last positive value
            // (up to 5 s) instead of blanking to '—'; only a
            // sustained idle shows 0 B/s / '—'. Other statuses
            // show '—' as before.
            const heldEntry = d.status === 'downloading' ? heldRef.current.get(d.id) : undefined
            const holding = heldEntry !== undefined && Date.now() - heldEntry.at <= 5_000
            const showSpeed =
              d.speed_bps > 0 ? d.speed_bps : holding && heldEntry ? heldEntry.speed : 0
            const showEta = d.eta_seconds ?? (holding && heldEntry ? heldEntry.eta : null)
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
                  setQueueSubOpen(false)
                  setMenu({ x: e.clientX, y: e.clientY, download: d })
                }}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') onOpenProgress(d.id)
                }}
                className={`cursor-pointer border-b border-border ${selected ? 'bg-accent-soft' : 'hover:bg-surface'}`}
                aria-selected={selected}
              >
                <td className="max-w-0 px-3 py-2" title={d.filename}>
                  <span className="block truncate font-medium">{d.filename}</span>
                  {showBar ? (
                    <span className="mt-1 block">
                      <ProgressBar
                        percent={percent}
                        gradient
                        size="thin"
                        status={d.status === 'paused' ? 'paused' : 'downloading'}
                        label={`Progress of ${d.filename}`}
                      />
                    </span>
                  ) : null}
                  {/* Round 72 (D) — connections and parts, side by
                      side: the part count grows past the
                      connections setting as splits happen (6 parts
                      on 2 connections), so showing only one number
                      made the setting look ignored in the Round 71
                      retest. Both come from the ~1 s checkpoint;
                      only downloading rows show them (a paused row
                      holds 0 connections). */}
                  {d.status === 'downloading' && d.segment_count > 0 ? (
                    <span className="mt-0.5 block text-xs text-text-muted">
                      {d.active_connections} connection{d.active_connections === 1 ? '' : 's'} ·{' '}
                      {d.segment_count} part{d.segment_count === 1 ? '' : 's'}
                    </span>
                  ) : null}
                </td>
                <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                  {formatBytes(d.size_bytes)}
                </td>
                <td className="whitespace-nowrap px-3 py-2" title={description || undefined}>
                  <span className="flex items-center gap-1.5">
                    <StatusBadge status={d.status} fileMissing={d.file_missing} />
                    {percent !== null && (d.status === 'downloading' || d.status === 'paused') ? (
                      <span className="font-mono text-xs text-text-muted">{percent}%</span>
                    ) : null}
                  </span>
                </td>
                <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                  {d.status === 'downloading' ? formatSpeed(showSpeed, '0 B/s') : '—'}
                </td>
                <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                  {d.status === 'downloading' ? formatEta(showEta) : '—'}
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
      {menu ? (
        <RowMenu
          menuRef={menuRef}
          x={menu.x}
          y={menu.y}
          download={menu.download}
          queues={queues}
          queueSubOpen={queueSubOpen}
          onToggleQueueSub={() => setQueueSubOpen((open) => !open)}
          onOpenProgress={() => onOpenProgress(menu.download.id)}
          onOpenFile={() => onOpenFile(menu.download.id)}
          onOpenWith={() => onOpenWith(menu.download.id)}
          onOpenFolder={() => onOpenFolder(menu.download.id)}
          onRedownload={() => onRedownload(menu.download)}
          onResume={() => onResume(menu.download.id)}
          onPause={() => onPause(menu.download.id)}
          onRefreshAddress={() => onRefreshAddress(menu.download)}
          onRemove={() => onRemove(menu.download)}
          onMoveToQueue={(queueId) => onMoveToQueue(menu.download.id, queueId)}
          onDeleteFromQueue={() => onDeleteFromQueue(menu.download.id)}
          onShowProperties={() => onShowProperties(menu.download)}
          onMoveToCategory={(categoryId) => onMoveToCategory(menu.download.id, categoryId)}
          categories={categories}
          closeMenu={closeMenu}
        />
      ) : null}
    </div>
  )
}

/** One row-menu item. A disabled item explains itself in its
 *  tooltip — the user rule (Round 65) is that every item is
 *  enabled or disabled by the row's state, exactly like IDM. */
function MenuItem({
  label,
  disabled,
  title,
  onClick,
}: {
  label: string
  disabled?: boolean
  title?: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      title={title}
      onClick={onClick}
      className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2 disabled:opacity-45"
    >
      {label}
    </button>
  )
}

function MenuSeparator() {
  return <div role="separator" className="my-1 h-px bg-border" aria-hidden="true" />
}

/** Round 65 — one download row's right-click menu, modelled on
 *  IDM's: Open, Open with..., Open folder, Open progress
 *  window, Redownload, Resume Download, Stop Download, Refresh
 *  download address, Remove, Move to queue, Delete from queue,
 *  Move to category, Properties. Every item is enabled or
 *  disabled by the row's state:
 *  - Open: only when the download is complete (there is a
 *    finished file to open).
 *  - Open with... / Open folder: only when a file is on disk
 *    (complete, or paused/failed with bytes already saved).
 *  - Resume Download: only when paused, failed, or waiting
 *    (queued) — a queued row is a download that was added but
 *    never started (Round 69 (b)).
 *  - Stop Download: only while downloading.
 *  - Refresh download address: only when paused or failed.
 *  - Move to queue: only shown when queues exist; a queue the
 *    download is already in is disabled. Delete from queue is
 *    always shown, disabled while the download is in no queue
 *    (Round 69 (e)).
 *  - Open progress window, Redownload, Remove, Properties:
 *    always available. */
function RowMenu({
  menuRef,
  x,
  y,
  download,
  queues,
  categories,
  queueSubOpen,
  onToggleQueueSub,
  onOpenProgress,
  onOpenFile,
  onOpenWith,
  onOpenFolder,
  onRedownload,
  onResume,
  onPause,
  onRefreshAddress,
  onRemove,
  onMoveToQueue,
  onDeleteFromQueue,
  onShowProperties,
  onMoveToCategory,
  closeMenu,
}: {
  menuRef: React.MutableRefObject<HTMLDivElement | null>
  x: number
  y: number
  download: DownloadRow
  queues: QueueRow[]
  categories: CategoryRow[]
  queueSubOpen: boolean
  onToggleQueueSub: () => void
  onOpenProgress: () => void
  onOpenFile: () => void
  onOpenWith: () => void
  onOpenFolder: () => void
  onRedownload: () => void
  onResume: () => void
  onPause: () => void
  onRefreshAddress: () => void
  onRemove: () => void
  onMoveToQueue: (queueId: string) => void
  onDeleteFromQueue: () => void
  onShowProperties: () => void
  onMoveToCategory: (categoryId: string) => void
  closeMenu: (action: () => void) => () => void
}) {
  const d = download
  const isComplete = d.status === 'complete'
  /** Round 74 (4) — a Finished row whose file was deleted from
   *  Windows has no file to open: Open / Open with... / Open
   *  folder are all disabled, and the row reads "File missing"
   *  (the StatusBadge). Redownload stays enabled — it is the
   *  way to get the file back. */
  const fileMissing = isComplete && d.file_missing
  /** A file exists on disk once the download finished (and the
   *  file is still there) or any bytes were saved (paused /
   *  failed with progress). */
  const hasFile = (isComplete && !fileMissing) || (!isComplete && d.downloaded_bytes > 0)
  // Round 69 (b) — `queued` ("Waiting") rows are resumable too
  // (same reason as the toolbar: the engine accepts any
  // non-complete, non-downloading row, and a queued row is a
  // download that was added but never started). The menu's Resume
  // used to be greyed for exactly the rows that most needed it.
  const canResume = d.status === 'paused' || d.status === 'error' || d.status === 'queued'
  const isDownloading = d.status === 'downloading'
  // Round 69 (e) — the queue section is always shown: "Delete
  // from queue" is a state-gated item like the rest (disabled
  // when the row is in no queue), never hidden. It used to render
  // only when `d.queue_id !== null`, so the Round 68 retest found
  // no such item on any row it checked — every row it checked
  // was in no queue.
  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label="Download actions"
      className="fixed z-20 max-h-[80vh] min-w-60 overflow-y-auto rounded-md border border-border bg-surface-raised p-1 shadow-pop"
      style={{
        left: Math.min(x, window.innerWidth - 260),
        top: Math.min(y, window.innerHeight - 200),
      }}
    >
      <MenuItem
        label="Open"
        disabled={!isComplete || fileMissing}
        title={
          fileMissing
            ? 'The file is no longer on disk — Redownload it to get it again'
            : isComplete
              ? undefined
              : 'Only a finished download can be opened'
        }
        onClick={closeMenu(onOpenFile)}
      />
      <MenuItem
        label="Open with..."
        disabled={!hasFile}
        title={
          fileMissing
            ? 'The file is no longer on disk — Redownload it to get it again'
            : hasFile
              ? undefined
              : 'No file on disk yet to open'
        }
        onClick={closeMenu(onOpenWith)}
      />
      <MenuItem
        label="Open folder"
        disabled={!hasFile}
        title={
          fileMissing
            ? 'The file is no longer on disk — Redownload it to get it again'
            : hasFile
              ? undefined
              : 'No file on disk yet to show'
        }
        onClick={closeMenu(onOpenFolder)}
      />
      <MenuItem label="Open progress window" onClick={closeMenu(onOpenProgress)} />
      <MenuItem
        label="Redownload"
        title="Download the same link again from scratch (the row stays; downloaded bytes are discarded)"
        onClick={closeMenu(onRedownload)}
      />
      <MenuItem
        label="Resume Download"
        disabled={!canResume}
        title={canResume ? undefined : 'Only a paused or failed download can be resumed'}
        onClick={closeMenu(onResume)}
      />
      <MenuItem
        label="Stop Download"
        disabled={!isDownloading}
        title={isDownloading ? undefined : 'Only a running download can be stopped'}
        onClick={closeMenu(onPause)}
      />
      <MenuSeparator />
      <MenuItem
        label="Refresh download address"
        disabled={!canResume}
        title={
          canResume
            ? 'Replace an expired link and resume from the downloaded bytes'
            : 'Only a paused or failed download can get a new address'
        }
        onClick={closeMenu(onRefreshAddress)}
      />
      <MenuItem label="Remove" onClick={closeMenu(onRemove)} />
      <MenuSeparator />
      {queues.length > 0 ? (
        <>
          <button
            type="button"
            aria-expanded={queueSubOpen}
            onClick={onToggleQueueSub}
            className="flex w-full items-center justify-between rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2"
          >
            <span>Move to queue</span>
            <span aria-hidden="true">{queueSubOpen ? '▾' : '▸'}</span>
          </button>
          {queueSubOpen ? (
            <div className="ml-3 border-l border-border pl-1">
              {queues.map((q) => (
                <MenuItem
                  key={q.id}
                  label={q.name}
                  disabled={d.queue_id === q.id}
                  title={d.queue_id === q.id ? 'Already in this queue' : undefined}
                  onClick={closeMenu(() => onMoveToQueue(q.id))}
                />
              ))}
            </div>
          ) : null}
        </>
      ) : null}
      <MenuItem
        label="Delete from queue"
        disabled={d.queue_id === null}
        title={
          d.queue_id === null
            ? 'Only a download in a queue can be removed from its queue'
            : undefined
        }
        onClick={closeMenu(onDeleteFromQueue)}
      />
      <MenuSeparator />
      <p className="px-3 pb-1 pt-2 text-xs font-semibold uppercase text-text-muted">
        Move to category
      </p>
      {categories.map((c) => (
        <MenuItem
          key={c.id}
          label={c.name}
          onClick={closeMenu(() => onMoveToCategory(c.id))}
        />
      ))}
      <MenuSeparator />
      <MenuItem label="Properties" onClick={closeMenu(onShowProperties)} />
    </div>
  )
}
