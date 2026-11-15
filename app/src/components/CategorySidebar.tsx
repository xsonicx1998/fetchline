import { useState } from 'react'
import { Folder, FolderOpen, List, Play, Plus, Trash2 } from 'lucide-react'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'

interface CategorySidebarProps {
  categories: CategoryRow[]
  downloads: DownloadRow[]
  queues: QueueRow[]
  selectedCategory: string | null
  selectedQueue: string | null
  onSelectCategory: (categoryId: string | null) => void
  onSelectQueue: (queueId: string | null) => void
  onAddCategory: () => void
  onAddQueue: () => void
  onDeleteCategory: (category: CategoryRow) => void
  onOpenScheduler: (queueId: string) => void
  /** A download row was dropped on a category or queue. */
  onDropDownload: (downloadId: string, target: { kind: 'category' | 'queue'; id: string }) => void
}

/** S04 + S05 — categories and queues sidebar (components.md:
 *  CategoriesTree). Categories filter the main table; queues do the
 *  same for their files. Downloads can be dragged from the table onto
 *  a category (moves them there) or onto a queue (adds them to it). */
export function CategorySidebar({
  categories,
  downloads,
  queues,
  selectedCategory,
  selectedQueue,
  onSelectCategory,
  onSelectQueue,
  onAddCategory,
  onAddQueue,
  onDeleteCategory,
  onOpenScheduler,
  onDropDownload,
}: CategorySidebarProps) {
  const [dropTarget, setDropTarget] = useState<string | null>(null)

  const countFor = (id: string | null) =>
    id === null ? downloads.length : downloads.filter((d) => d.category_id === id).length

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

  const rowClass = (active: boolean, key: string) =>
    `flex h-8 items-center gap-2 rounded-md px-2 text-left text-base ${
      active ? 'bg-accent-soft font-semibold text-accent' : 'text-text hover:bg-surface-2'
    } ${dropTarget === key ? 'outline outline-2 outline-accent' : ''}`

  return (
    <nav
      aria-label="Categories and queues"
      className="flex h-full w-sidebar shrink-0 flex-col gap-1 overflow-y-auto border-r border-border bg-surface p-2"
    >
      <p className="px-2 pb-1 pt-1 text-xs font-semibold uppercase text-text-muted">Categories</p>
      <button
        type="button"
        onClick={() => {
          onSelectCategory(null)
          onSelectQueue(null)
        }}
        aria-current={selectedCategory === null && selectedQueue === null || undefined}
        className={rowClass(selectedCategory === null && selectedQueue === null, 'all')}
      >
        <FolderOpen className="h-4 w-4 shrink-0" aria-hidden="true" />
        <span className="flex-1 truncate">All downloads</span>
        <span className="text-xs text-text-muted">{countFor(null)}</span>
      </button>
      {categories.map((c) => (
        <div key={c.id} className="group flex items-center gap-1" {...dropProps(`cat:${c.id}`, { kind: 'category', id: c.id })}>
          <button
            type="button"
            onClick={() => {
              onSelectCategory(c.id)
              onSelectQueue(null)
            }}
            aria-current={selectedCategory === c.id || undefined}
            className={`${rowClass(selectedCategory === c.id, `cat:${c.id}`)} flex-1`}
          >
            <Folder className="h-4 w-4 shrink-0" aria-hidden="true" />
            <span className="flex-1 truncate">{c.name}</span>
            <span className="text-xs text-text-muted">{countFor(c.id)}</span>
          </button>
          {c.is_builtin ? null : (
            <button
              type="button"
              onClick={() => onDeleteCategory(c)}
              aria-label={`Delete category ${c.name}`}
              title={`Delete category ${c.name}`}
              className="shrink-0 rounded-md p-1 text-text-muted opacity-0 hover:bg-surface-2 hover:text-danger group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          )}
        </div>
      ))}
      <button
        type="button"
        onClick={onAddCategory}
        className="flex h-8 items-center gap-2 rounded-md px-2 text-left text-base text-text-muted hover:bg-surface-2"
      >
        <Plus className="h-4 w-4 shrink-0" aria-hidden="true" />
        New category
      </button>

      <p className="px-2 pb-1 pt-3 text-xs font-semibold uppercase text-text-muted">Queues</p>
      {queues.map((q) => (
        <div key={q.id} className="group flex items-center gap-1" {...dropProps(`queue:${q.id}`, { kind: 'queue', id: q.id })}>
          <button
            type="button"
            onClick={() => {
              onSelectQueue(q.id)
              onSelectCategory(null)
            }}
            onDoubleClick={() => onOpenScheduler(q.id)}
            aria-current={selectedQueue === q.id || undefined}
            title={`${q.name} — double-click to open the scheduler`}
            className={`${rowClass(selectedQueue === q.id, `queue:${q.id}`)} flex-1`}
          >
            <List className="h-4 w-4 shrink-0" aria-hidden="true" />
            <span className="flex-1 truncate">{q.name}</span>
            <span className="text-xs text-text-muted">
              {q.file_count} file{q.file_count === 1 ? '' : 's'}
            </span>
            {q.running ? (
              <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs font-semibold text-on-accent">
                Running
              </span>
            ) : null}
          </button>
          <button
            type="button"
            onClick={() => onOpenScheduler(q.id)}
            aria-label={`Schedule ${q.name}`}
            title={`Schedule ${q.name}`}
            className="shrink-0 rounded-md p-1 text-text-muted opacity-0 hover:bg-surface-2 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100"
          >
            <Play className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
      ))}
      <button
        type="button"
        onClick={onAddQueue}
        className="flex h-8 items-center gap-2 rounded-md px-2 text-left text-base text-text-muted hover:bg-surface-2"
      >
        <Plus className="h-4 w-4 shrink-0" aria-hidden="true" />
        New queue
      </button>
    </nav>
  )
}
