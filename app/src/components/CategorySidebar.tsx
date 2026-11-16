import { useState } from 'react'
import { Play, Plus, RefreshCw, Trash2 } from 'lucide-react'
import type { CategoryRow, DownloadRow, GrabberProjectRow, QueueRow } from '../lib/types'
import { PackIcon } from './PackIcon'

/** Which list the main table shows. 'all' is every download (or
 *  one category's); 'unfinished' / 'finished' filter by status the
 *  way IDM's tree does; 'grabber' shows one site-grabber project's
 *  downloads. */
export type SidebarFilter = 'all' | 'unfinished' | 'finished' | 'grabber'

interface CategorySidebarProps {
  categories: CategoryRow[]
  downloads: DownloadRow[]
  queues: QueueRow[]
  grabberProjects: GrabberProjectRow[]
  selectedCategory: string | null
  selectedQueue: string | null
  selectedFilter: SidebarFilter
  selectedGrabberId: string | null
  onSelectCategory: (categoryId: string | null) => void
  onSelectQueue: (queueId: string | null) => void
  onSelectFilter: (filter: SidebarFilter, grabberId?: string | null) => void
  onAddCategory: () => void
  onAddQueue: () => void
  onDeleteCategory: (category: CategoryRow) => void
  onOpenScheduler: (queueId: string) => void
  onRunGrabber: (projectId: string) => void
  onNewGrabber: () => void
  /** Re-probe a sync queue now (was Downloads > Sync queue now in
   *  the menu bar; the menu bar is gone in Round 45). */
  onSyncQueue: (queueId: string) => void
  /** A download row was dropped on a category or queue. */
  onDropDownload: (downloadId: string, target: { kind: 'category' | 'queue'; id: string }) => void
}

/** S04 + S05 — categories and queues sidebar (components.md:
 *  CategoriesTree), IDM's arrangement: the category tree, then
 *  Unfinished / Finished, then Grabber projects, then Queues.
 *  Categories filter the main table; queues do the same for their
 *  files; the status rows filter by completion; a grabber project
 *  filters to the downloads it produced. Downloads can be dragged
 *  from the table onto a category (moves them there) or onto a
 *  queue (adds them to it). */
export function CategorySidebar({
  categories,
  downloads,
  queues,
  grabberProjects,
  selectedCategory,
  selectedQueue,
  selectedFilter,
  selectedGrabberId,
  onSelectCategory,
  onSelectQueue,
  onSelectFilter,
  onAddCategory,
  onAddQueue,
  onDeleteCategory,
  onOpenScheduler,
  onRunGrabber,
  onNewGrabber,
  onSyncQueue,
  onDropDownload,
}: CategorySidebarProps) {
  const [dropTarget, setDropTarget] = useState<string | null>(null)

  const countFor = (id: string | null) =>
    id === null ? downloads.length : downloads.filter((d) => d.category_id === id).length
  const unfinishedCount = downloads.filter((d) => d.status !== 'complete').length
  const finishedCount = downloads.filter((d) => d.status === 'complete').length

  function handleDrop(e: React.DragEvent, target: { kind: 'category' | 'queue'; id: string }) {
    e.preventDefault()
    setDropTarget(null)
    let downloadId = ''
    try {
      const data = JSON.parse(e.dataTransfer.getData('text/plain')) as { type?: string; id?: string }
      if (data.type === 'download' && data.id) downloadId = data.id
    } catch {
      return
    }
    if (downloadId) onDropDownload(downloadId, target)
  }

  const dropProps = (key: string, target: { kind: 'category' | 'queue'; id: string }) => ({
    onDragOver: (e: React.DragEvent) => {
      e.preventDefault()
      setDropTarget(key)
    },
    onDragLeave: () => setDropTarget((t) => (t === key ? null : t)),
    onDrop: (e: React.DragEvent) => handleDrop(e, target),
  })

  /** The active row is the reference's filled purple row, white
   *  text — not the old soft tint. The count inside it is white
   *  at 80% so it stays readable on the purple. Round 47: the
   *  active row glows, and rows are 36 px with 20 px icons (was
   *  32 px / 16 px) to match the reference's proportions.
   *  Round 48: the sidebar icons glow in their colours (purple
   *  folders / Unfinished / Finished / queues, cyan grabber) so
   *  the tree doesn't read dim next to the toolbar. */
  const rowClass = (active: boolean, key: string) =>
    `flex h-9 items-center gap-2 rounded-md px-2 text-left text-base ${
      active ? 'bg-accent font-semibold text-on-accent shadow-[0_0_12px_rgba(139,32,255,0.45)]' : 'text-text hover:bg-surface-2'
    } ${dropTarget === key ? 'outline outline-2 outline-accent' : ''}`
  const countClass = (active: boolean, fadeOnHover = false) =>
    `text-xs transition-opacity ${active ? 'text-white/80' : 'text-text-muted'} ${
      fadeOnHover ? 'group-hover:opacity-0' : ''
    }`

  /** Hover actions (delete category, run grabber, sync/schedule a
   *  queue) live in an absolutely-positioned overlay at the row's
   *  right edge instead of in the row's flow (Round 46): in-flow
   *  buttons, invisible or not, still took 24 px each, which is
   *  what truncated "Main download queue" at 1024 px. The overlay
   *  fades in on hover/focus and the count fades out under it; the
   *  buttons stay keyboard-focusable (opacity-0, never
   *  display:none). */

  const allActive = selectedCategory === null && selectedQueue === null && selectedFilter === 'all'

  return (
    <nav
      aria-label="Categories and queues"
      className="flex h-full w-sidebar shrink-0 flex-col gap-1 overflow-y-auto rounded-xl border border-border bg-surface p-2 shadow-panel"
    >
      <p className="px-2 pb-1 pt-1 text-xs font-semibold uppercase tracking-wider text-text-muted">Categories</p>
      <button
        type="button"
        onClick={() => {
          onSelectCategory(null)
          onSelectQueue(null)
          onSelectFilter('all')
        }}
        aria-current={allActive || undefined}
        className={rowClass(allActive, 'all')}
      >
        <PackIcon name="category-folder" className="h-5 w-5 icon-glow-purple" />
        <span className="flex-1 truncate">All downloads</span>
        <span className={countClass(allActive)}>{countFor(null)}</span>
      </button>
      {categories.map((c) => (
        <div key={c.id} className="group relative flex items-center gap-1" {...dropProps(`cat:${c.id}`, { kind: 'category', id: c.id })}>
          <button
            type="button"
            onClick={() => {
              onSelectCategory(c.id)
              onSelectQueue(null)
              onSelectFilter('all')
            }}
            aria-current={selectedCategory === c.id || undefined}
            className={`${rowClass(selectedCategory === c.id, `cat:${c.id}`)} flex-1`}
          >
            <PackIcon name="category-folder" className="h-5 w-5 icon-glow-purple" />
            <span className="flex-1 truncate">{c.name}</span>
            <span className={countClass(selectedCategory === c.id, true)}>{countFor(c.id)}</span>
          </button>
          {c.is_builtin ? null : (
            <button
              type="button"
              onClick={() => onDeleteCategory(c)}
              aria-label={`Delete category ${c.name}`}
              title={`Delete category ${c.name}`}
              className="absolute right-1 top-1/2 z-10 -translate-y-1/2 rounded-md p-1 text-text-muted opacity-0 hover:bg-surface-2 hover:text-danger group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          )}
        </div>
      ))}
      <button
        type="button"
        onClick={onAddCategory}
        className="flex h-9 items-center gap-2 rounded-md px-2 text-left text-base text-text-muted hover:bg-surface-2"
      >
        <Plus className="h-5 w-5 shrink-0" aria-hidden="true" />
        New category
      </button>

      <div className="my-1 h-px shrink-0 bg-border" aria-hidden="true" />
      <button
        type="button"
        onClick={() => onSelectFilter('unfinished')}
        aria-current={selectedFilter === 'unfinished' || undefined}
        className={rowClass(selectedFilter === 'unfinished', 'unfinished')}
      >
        <PackIcon name="unfinished" className="h-5 w-5 icon-glow-purple" />
        <span className="flex-1 truncate">Unfinished</span>
        <span className={countClass(selectedFilter === 'unfinished')}>{unfinishedCount}</span>
      </button>
      <button
        type="button"
        onClick={() => onSelectFilter('finished')}
        aria-current={selectedFilter === 'finished' || undefined}
        className={rowClass(selectedFilter === 'finished', 'finished')}
      >
        <PackIcon name="finished" className="h-5 w-5 icon-glow-purple" />
        <span className="flex-1 truncate">Finished</span>
        <span className={countClass(selectedFilter === 'finished')}>{finishedCount}</span>
      </button>

      <p className="px-2 pb-1 pt-3 text-xs font-semibold uppercase tracking-wider text-text-muted">Grabber projects</p>
      {grabberProjects.map((p) => {
        const running = p.status === 'exploring' || p.status === 'downloading'
        const active = selectedFilter === 'grabber' && selectedGrabberId === p.id
        return (
          <div key={p.id} className="group relative flex items-center gap-1">
            <button
              type="button"
              onClick={() => onSelectFilter('grabber', p.id)}
              aria-current={active || undefined}
              title={`${p.name} — ${p.files_downloaded} downloaded, ${p.files_matched} matched`}
              className={`${rowClass(active, `grabber:${p.id}`)} flex-1`}
            >
              <PackIcon name="grabber" className="h-5 w-5 icon-glow-cyan" />
              <span className="flex-1 truncate">{p.name}</span>
              <span className={countClass(active, true)}>
                {p.files_downloaded} file{p.files_downloaded === 1 ? '' : 's'}
              </span>
              {running ? (
                <span className="shrink-0 rounded-full bg-info px-2 py-0.5 text-xs font-semibold text-bg transition-opacity group-hover:opacity-0">
                  Running
                </span>
              ) : null}
            </button>
            {running ? null : (
              <button
                type="button"
                onClick={() => onRunGrabber(p.id)}
                aria-label={`Run grabber project ${p.name}`}
                title={`Run ${p.name}`}
                className="absolute right-1 top-1/2 z-10 -translate-y-1/2 rounded-md p-1 text-text-muted opacity-0 hover:bg-surface-2 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100"
              >
                <Play className="h-4 w-4" aria-hidden="true" />
              </button>
            )}
          </div>
        )
      })}
      <button
        type="button"
        onClick={onNewGrabber}
        className="flex h-9 items-center gap-2 rounded-md px-2 text-left text-base text-text-muted hover:bg-surface-2"
      >
        <Plus className="h-5 w-5 shrink-0" aria-hidden="true" />
        New grabber project
      </button>

      <p className="px-2 pb-1 pt-3 text-xs font-semibold uppercase tracking-wider text-text-muted">Queues</p>
      {queues.map((q) => (
        <div key={q.id} className="group relative flex items-center gap-1" {...dropProps(`queue:${q.id}`, { kind: 'queue', id: q.id })}>
          <button
            type="button"
            onClick={() => {
              onSelectQueue(q.id)
              onSelectCategory(null)
              onSelectFilter('all')
            }}
            onDoubleClick={() => onOpenScheduler(q.id)}
            aria-current={selectedQueue === q.id || undefined}
            title={`${q.name} — double-click to open the scheduler`}
            className={`${rowClass(selectedQueue === q.id, `queue:${q.id}`)} flex-1`}
          >
            <PackIcon name="queue" className="h-5 w-5 icon-glow-purple" />
            <span className="flex-1 truncate">{q.name}</span>
            <span className={countClass(selectedQueue === q.id, true)}>
              {q.file_count} file{q.file_count === 1 ? '' : 's'}
            </span>
            {q.running ? (
              <span className="shrink-0 rounded-full bg-info px-2 py-0.5 text-xs font-semibold text-bg transition-opacity group-hover:opacity-0">
                Running
              </span>
            ) : null}
          </button>
          <div className="absolute right-1 top-1/2 z-10 flex -translate-y-1/2 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            <button
              type="button"
              onClick={() => onSyncQueue(q.id)}
              aria-label={`Sync ${q.name} now`}
              title={`Sync ${q.name} now`}
              className="rounded-md p-1 text-text-muted hover:bg-surface-2 focus-visible:opacity-100"
            >
              <RefreshCw className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => onOpenScheduler(q.id)}
              aria-label={`Schedule ${q.name}`}
              title={`Schedule ${q.name}`}
              className="rounded-md p-1 text-text-muted hover:bg-surface-2 focus-visible:opacity-100"
            >
              <Play className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>
        </div>
      ))}
      <button
        type="button"
        onClick={onAddQueue}
        className="flex h-9 items-center gap-2 rounded-md px-2 text-left text-base text-text-muted hover:bg-surface-2"
      >
        <Plus className="h-5 w-5 shrink-0" aria-hidden="true" />
        New queue
      </button>
    </nav>
  )
}
