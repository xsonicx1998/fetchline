import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { downloadAllLinks, fetchPageLinks } from '../lib/api'
import { formatBytes } from '../lib/format'
import type { BatchResult, LinkKind, PageLink, QueueRow } from '../lib/types'
import { Button } from './Button'

interface DownloadAllDialogProps {
  open: boolean
  queues: QueueRow[]
  onClose: () => void
  onAdded: () => void
}

type Filter = 'all' | LinkKind

const FILTERS: { value: Filter; label: string }[] = [
  { value: 'all', label: 'All types' },
  { value: 'video', label: 'Video' },
  { value: 'audio', label: 'Audio' },
  { value: 'image', label: 'Images' },
  { value: 'document', label: 'Documents' },
  { value: 'archive', label: 'Archives' },
  { value: 'program', label: 'Programs' },
  { value: 'other', label: 'Other' },
]

/** S07 — download every link on a page at once (components.md: Dialog,
 *  Checkbox list). Fetch the page, check its links, filter by type,
 *  pick the ones to keep, then start them now or file them in a queue.
 *  Links that fail the probe are listed with their error and cannot be
 *  picked; links already in the download list are flagged and left
 *  unchecked, but can still be picked. */
export function DownloadAllDialog({ open, queues, onClose, onAdded }: DownloadAllDialogProps) {
  const [pageUrl, setPageUrl] = useState('')
  const [links, setLinks] = useState<PageLink[]>([])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filter, setFilter] = useState<Filter>('all')
  const [queueId, setQueueId] = useState('queue-main')
  const [phase, setPhase] = useState<'url' | 'links' | 'done'>('url')
  const [result, setResult] = useState<BatchResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setPageUrl('')
      setLinks([])
      setSelected(new Set())
      setFilter('all')
      setQueueId('queue-main')
      setPhase('url')
      setResult(null)
      setError(null)
      setBusy(false)
      urlRef.current?.focus()
    }
  }, [open ])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  const pageValid = pageUrl.trim() === '' || /^https?:\/\/.+/i.test(pageUrl.trim())
  const visible = filter === 'all' ? links : links.filter((l) => l.kind === filter)
  const selectable = links.filter((l) => l.error === null)
  const selectedLinks = links.filter((l) => selected.has(l.url))
  const selectedSize = selectedLinks.reduce((sum, l) => sum + (l.size ?? 0), 0)
  const unknownSize = selectedLinks.some((l) => l.size === null)

  async function fetchLinks() {
    if (!pageUrl.trim() || !pageValid) {
      setError('Enter a page link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const found = await fetchPageLinks(pageUrl.trim())
      setLinks(found)
      // Dead links are excluded; already-added links start unchecked.
      setSelected(new Set(found.filter((l) => l.error === null && !l.already_exists).map((l) => l.url)))
      setPhase('links')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  function toggle(url: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(url)) next.delete(url)
      else next.add(url)
      return next
    })
  }

  function selectAll() {
    setSelected(new Set(selectable.filter((l) => filter === 'all' || l.kind === filter).map((l) => l.url)))
  }

  function selectNone() {
    setSelected(new Set())
  }

  function invert() {
    setSelected((prev) => {
      const next = new Set(prev)
      for (const l of selectable) {
        if (filter !== 'all' && l.kind !== filter) continue
        if (next.has(l.url)) next.delete(l.url)
        else next.add(l.url)
      }
      return next
    })
  }

  async function submit(startNow: boolean) {
    if (selected.size === 0) {
      setError('Pick at least one link to download.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const res = await downloadAllLinks({
        pageUrl: pageUrl.trim(),
        links: [...selected],
        queue: startNow ? null : queueId,
        startNow,
      })
      setResult(res)
      onAdded()
      if (res.invalid.length === 0) {
        onClose()
      } else {
        setPhase('done')
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="download-all-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="flex max-h-full w-full max-w-2xl flex-col rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="download-all-title" className="text-lg font-semibold">
            Download all links on a page
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2"
          >
            <X className="h-5 w-5" aria-hidden="true" />
          </button>
        </div>

        {phase === 'url' ? (
          <div className="flex flex-col gap-4">
            <div>
              <label htmlFor="download-all-page" className="mb-1 block text-sm font-semibold">
                Page link
              </label>
              <input
                ref={urlRef}
                id="download-all-page"
                type="url"
                value={pageUrl}
                onChange={(e) => setPageUrl(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void fetchLinks()
                }}
                placeholder="https://example.com/gallery.html"
                aria-invalid={!pageValid || undefined}
                className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
              />
              <p className="mt-1 text-sm text-text-muted">
                The page is fetched once to list its links. Only openly downloadable links are shown.
              </p>
            </div>
            {error ? (
              <p role="alert" className="text-sm text-danger">
                {error}
              </p>
            ) : null}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button variant="primary" loading={busy} onClick={() => void fetchLinks()}>
                Fetch links
              </Button>
            </div>
          </div>
        ) : phase === 'done' && result ? (
          <div className="flex flex-col gap-4">
            <p className="text-sm">
              Added {result.created.length} download{result.created.length === 1 ? '' : 's'}.
              {result.invalid.length > 0
                ? ` ${result.invalid.length} link${result.invalid.length === 1 ? '' : 's'} could not be added:`
                : ''}
            </p>
            {result.invalid.length > 0 ? (
              <ul className="max-h-64 overflow-auto rounded-md border border-border bg-bg p-3 text-sm">
                {result.invalid.map((bad) => (
                  <li key={`${bad.line}-${bad.url}`} className="py-1">
                    <span className="font-mono text-xs text-text-muted">{bad.url}</span>
                    <span className="text-danger"> — {bad.error}</span>
                  </li>
                ))}
              </ul>
            ) : null}
            <div className="flex justify-end">
              <Button variant="primary" onClick={onClose}>
                Done
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex min-h-0 flex-col gap-4">
            <div className="flex flex-wrap items-center gap-2">
              <label htmlFor="download-all-filter" className="text-sm font-semibold">
                Show
              </label>
              <select
                id="download-all-filter"
                value={filter}
                onChange={(e) => setFilter(e.target.value as Filter)}
                className="h-8 rounded-md border border-border-input bg-bg px-2 text-sm focus:border-accent"
              >
                {FILTERS.map((f) => (
                  <option key={f.value} value={f.value}>
                    {f.label}
                  </option>
                ))}
              </select>
              <Button variant="secondary" size="sm" onClick={selectAll}>
                Select all
              </Button>
              <Button variant="secondary" size="sm" onClick={selectNone}>
                Select none
              </Button>
              <Button variant="secondary" size="sm" onClick={invert}>
                Invert
              </Button>
              <span className="ml-auto text-sm text-text-muted">
                {selected.size} of {selectable.length} selected
                {selected.size > 0
                  ? ` · ${formatBytes(selectedSize)}${unknownSize ? ' + unknown' : ''}`
                  : ''}
              </span>
            </div>

            {links.length === 0 ? (
              <p className="rounded-md border border-border bg-bg p-4 text-sm text-text-muted">
                This page has no downloadable links.
              </p>
            ) : visible.length === 0 ? (
              <p className="rounded-md border border-border bg-bg p-4 text-sm text-text-muted">
                No links of this type on the page.
              </p>
            ) : (
              <ul className="max-h-80 min-h-0 overflow-auto rounded-md border border-border">
                {visible.map((link) => (
                  <li
                    key={link.url}
                    className="flex items-center gap-3 border-b border-border px-3 py-2 last:border-b-0"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(link.url)}
                      disabled={link.error !== null}
                      onChange={() => toggle(link.url)}
                      aria-label={`Download ${link.filename}`}
                      className="h-4 w-4 shrink-0 accent-accent"
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-semibold" title={link.url}>
                        {link.filename}
                      </span>
                      <span className="block truncate font-mono text-xs text-text-muted" title={link.url}>
                        {link.url}
                      </span>
                      {link.error ? (
                        <span className="block text-xs text-danger">{link.error}</span>
                      ) : null}
                      {link.already_exists ? (
                        <span className="block text-xs text-text-muted">Already in your downloads</span>
                      ) : null}
                    </span>
                    <span className="shrink-0 rounded-full bg-surface-2 px-2 py-0.5 text-xs font-semibold text-text-muted">
                      {link.kind}
                    </span>
                    <span className="w-16 shrink-0 text-right font-mono text-xs text-text-muted">
                      {link.size !== null ? formatBytes(link.size) : '—'}
                    </span>
                  </li>
                ))}
              </ul>
            )}

            <div>
              <label htmlFor="download-all-queue" className="mb-1 block text-sm font-semibold">
                Queue <span className="font-normal text-text-muted">(for “Add to queue”)</span>
              </label>
              <select
                id="download-all-queue"
                value={queueId}
                onChange={(e) => setQueueId(e.target.value)}
                className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
              >
                {queues.map((q) => (
                  <option key={q.id} value={q.id}>
                    {q.name}
                  </option>
                ))}
              </select>
            </div>

            {error ? (
              <p role="alert" className="text-sm text-danger">
                {error}
              </p>
            ) : null}
            <div className="flex justify-end gap-2">
              <Button
                variant="ghost"
                onClick={() => {
                  setLinks([])
                  setPhase('url')
                }}
              >
                Back
              </Button>
              <Button variant="secondary" loading={busy} onClick={() => void submit(false)}>
                Add to queue
              </Button>
              <Button variant="primary" loading={busy} onClick={() => void submit(true)}>
                Start now
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
