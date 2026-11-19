import { useEffect } from 'react'
import { X } from 'lucide-react'
import { Button } from './Button'

interface ConfirmDialogProps {
  open: boolean
  title: string
  message: string
  /** Label of the confirming button (danger-styled). */
  confirmLabel: string
  /** Label of the dismissing button. Defaults to "Keep it". */
  cancelLabel?: string
  onConfirm: () => void
  onCancel: () => void
}

/** Round 74 (11) — the small fixed-size "Are you sure?" confirm
 *  (cancel-a-download, and anything else that needs one). It
 *  matches the other confirm dialogs (remove / redownload):
 *  max-w-md, the same border / surface / shadow. FIXED SIZE by
 *  the user's decision — no drag region, no resize — and it
 *  closes only via Cancel / X / Escape, never a backdrop click
 *  (Round 74 (7)). */
export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel = 'Keep it',
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onCancel])

  if (!open) return null

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
      className="fixed inset-0 z-30 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="flex items-start justify-between gap-4">
          <h2 id="confirm-dialog-title" className="text-lg font-semibold">
            {title}
          </h2>
          <button
            type="button"
            onClick={onCancel}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted transition-colors hover:bg-surface-2 hover:text-text"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
        <p className="mt-2 text-sm text-text-muted">{message}</p>
        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onCancel}>
            {cancelLabel}
          </Button>
          <Button variant="danger" onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </div>
    </div>
  )
}
