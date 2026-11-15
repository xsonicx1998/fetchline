import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { addBatch } from '../lib/api'
import type { BatchResult, QueueRow } from '../lib/types'
import { Button } from './Button'

interface BatchDialogProps {
  open: boolean
  queues: QueueRow[]
  onClose: () => void
  onAdded: () => void
}

/** S10 — add many downloads in one go (components.md: Dialog,
 *  TextInput). One link per line. Blank lines are ignored; bad lines
 *  are listed with their line number and skipped, never fatal — the
 *  rest of the batch still goes in. Each link is added through the
 *  same path as a single add, so category auto-assign and the
 *  duplicate-file guard apply. */
export function BatchDialog({ open, queues, onClose, onAdded }: BatchDialogProps) {
  const [text, setText] = useState('')
  const [queueId, setQueueId] = useState('queue-main')
  const [result, setResult] = useState<BatchResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const areaRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (open) {
      setText('')
      setQueueId('queue-main')
      setResult(null)
      setError(null)
      setBusy(false)
      areaRef.current?.focus()
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

  const lines = text.split('\n').filter((l) => l.trim() !== '')
  const done = result !== null

  async function submit(startNow: boolean) {
    if (lines.length === 0) {
      setError('Paste at least one link, one per line.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const res = await addBatch({
        urls: lines,
        queue: startNow ? null : queueId,
        startNow,
      })
      setResult(res)
      onAdded()
      if (res.invalid.length === 0) {
        onClose()
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
      aria-labelledby="batch-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="flex max-h-full w-full max-w-lg flex-col rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="batch-title" className="text-lg font-semibold">
            Add batch download
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

        {done && result ? (
          <div className="flex flex-col gap-4">
            <p className="text-sm">
              Added {result.created.length} download{result.created.length === 1 ? '' : 's'}.
              {result.invalid.length > 0
                ? ` ${result.invalid.length} line${result.invalid.length === 1 ? '' : 's'} could not be added:`
                : ''}
            </p>
            {result.invalid.length > 0 ? (
              <ul className="max-h-64 overflow-auto rounded-md border border-border bg-bg p-3 text-sm">
                {result.invalid.map((bad) => (
                  <li key={`${bad.line}-${bad.url}`} className="py-1">
                    <span className="font-semibold">Line {bad.line}: </span>
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
            <div>
              <label htmlFor="batch-urls" className="mb-1 block text-sm font-semibold">
                Links <span className="font-normal text-text-muted">(one per line)</span>
              </label>
              <textarea
                ref={areaRef}
                id="batch-urls"
                value={text}
                onChange={(e) => setText(e.target.value)}
                rows={10}
                placeholder={'https://example.com/file1.zip\nhttps://example.com/file2.mp4'}
                aria-describedby="batch-count"
                className="w-full rounded-md border border-border-input bg-bg px-3 py-2 font-mono text-sm focus:border-accent"
              />
              <p id="batch-count" className="mt-1 text-sm text-text-muted">
                {lines.length} link{lines.length === 1 ? '' : 's'} — blank lines are ignored.
              </p>
            </div>

            <div>
              <label htmlFor="batch-queue" className="mb-1 block text-sm font-semibold">
                Queue <span className="font-normal text-text-muted">(for “Add to queue”)</span>
              </label>
              <select
                id="batch-queue"
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
              <Button variant="ghost" onClick={onClose}>
                Cancel
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
