import { useEffect, useState } from 'react'
import { Power } from 'lucide-react'
import { dismissQueueAction, executeQueueAction } from '../lib/api'
import { basename, completeActionLabel } from '../lib/format'
import type { PendingAction } from '../lib/types'
import { Button } from './Button'

interface QueueActionDialogProps {
  action: PendingAction | null
  onDone: () => void
}

const COUNTDOWN_SECONDS = 60

/** The countdown shown before a shutdown / hibernate action runs.
 *  Nothing happens silently: the reader gets 60 seconds to cancel,
 *  can run it now, or can cancel it outright. When the countdown
 *  reaches zero the action runs. */
export function QueueActionDialog({ action, onDone }: QueueActionDialogProps) {
  const [secondsLeft, setSecondsLeft] = useState(COUNTDOWN_SECONDS)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setSecondsLeft(COUNTDOWN_SECONDS)
    setBusy(false)
    setError(null)
  }, [action?.queue_id])

  useEffect(() => {
    if (!action) return
    const timer = window.setInterval(() => {
      setSecondsLeft((s) => Math.max(0, s - 1))
    }, 1000)
    return () => window.clearInterval(timer)
  }, [action])

  useEffect(() => {
    if (!action || secondsLeft > 0) return
    void run()
    // Only the countdown expiry runs the action; the effect deps
    // intentionally exclude `run` to avoid double execution.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [secondsLeft, action])

  if (!action) return null
  const queueId = action.queue_id

  async function run() {
    setBusy(true)
    try {
      await executeQueueAction(queueId)
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  async function cancel() {
    setBusy(true)
    try {
      await dismissQueueAction(queueId)
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="queue-action-title"
      aria-describedby="queue-action-desc"
      className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-2 flex items-center gap-3">
          <Power className="h-6 w-6 text-danger" aria-hidden="true" />
          <h2 id="queue-action-title" className="text-lg font-semibold">
            {action.action === 'hibernate' ? 'Hibernate the computer?' : 'Shut down the computer?'}
          </h2>
        </div>
        <p id="queue-action-desc" className="text-base text-text-muted">
          Queue “{action.queue_name}” finished
          {action.file_path ? ` — last file: ${basename(action.file_path)}` : ''}.{' '}
          {completeActionLabel(action.action)}.
        </p>
        <p className="mt-4 text-center text-3xl font-bold tabular-nums" aria-live="polite">
          {secondsLeft}s
        </p>
        <p className="text-center text-sm text-text-muted">
          {action.action === 'hibernate' ? 'Hibernating' : 'Shutting down'} when the countdown ends.
        </p>
        {error ? (
          <p role="alert" className="mt-2 text-sm text-danger">
            {error}
          </p>
        ) : null}
        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" loading={busy} onClick={() => void cancel()}>
            Cancel
          </Button>
          <Button variant="danger" loading={busy} onClick={() => void run()}>
            {action.action === 'hibernate' ? 'Hibernate now' : 'Shut down now'}
          </Button>
        </div>
      </div>
    </div>
  )
}
