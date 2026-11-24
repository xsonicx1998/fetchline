import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { createQueue } from '../lib/api'
import { Button } from './Button'

interface AddQueueDialogProps {
  open: boolean
  onClose: () => void
  onAdded: (id: string) => void
}

/** S05 — create a custom queue. A queue is just a named list of files
 *  that download one after another when it is started. */
export function AddQueueDialog({ open, onClose, onAdded }: AddQueueDialogProps) {
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setName('')
      setError(null)
      setBusy(false)
      nameRef.current?.focus()
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

  async function submit() {
    if (!name.trim()) {
      setError('Give the queue a name.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const queue = await createQueue(name.trim())
      onAdded(queue.id)
      onClose()
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
      aria-labelledby="add-queue-title"
      // Round 88.9 (3) — z-20, above the scheduler (z-10): the
      // scheduler's "New queue" button opens this dialog, and at
      // z-10 it painted UNDER the scheduler (App renders this
      // dialog before the scheduler in the DOM, same z-index),
      // so the user could not see or reach it.
      className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="add-queue-title" className="text-lg font-semibold">
            New queue
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

        <div>
          <label htmlFor="add-queue-name" className="mb-1 block text-sm font-semibold">
            Name
          </label>
          <input
            ref={nameRef}
            id="add-queue-name"
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
            placeholder="Evening downloads"
            className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
          />
          {error ? (
            <p role="alert" className="mt-2 text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void submit()}>
            Create queue
          </Button>
        </div>
      </div>
    </div>
  )
}
