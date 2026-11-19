import { useEffect } from 'react'
import { X } from 'lucide-react'
import type { ClipboardOffer } from '../lib/types'
import { Button } from './Button'

interface ClipboardOfferDialogProps {
  offer: ClipboardOffer
  onDownload: () => void
  onDismiss: () => void
}

/** Round 52 — clipboard monitoring: a download link was copied
 *  to the clipboard. The dialog offers to download it — one
 *  click adds it, dismiss ignores it. Nothing is ever downloaded
 *  without the user, and the engine never offers the same URL
 *  again once it is dismissed. */
export function ClipboardOfferDialog({ offer, onDownload, onDismiss }: ClipboardOfferDialogProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onDismiss()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onDismiss])

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="clipboard-offer-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="clipboard-offer-title" className="text-lg font-semibold">
            Download this link?
          </h2>
          <button
            type="button"
            onClick={onDismiss}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2"
          >
            <X className="h-5 w-5" aria-hidden="true" />
          </button>
        </div>

        <div className="flex flex-col gap-4">
          <p className="text-sm text-text-muted">
            You copied a download link to the clipboard:{' '}
            <span className="font-semibold text-text">{offer.filename}</span>
          </p>

          <div>
            <label className="mb-1 block text-sm font-semibold">Link</label>
            <p className="break-all rounded-md border border-border bg-bg px-3 py-2 font-mono text-sm text-text-muted">
              {offer.url}
            </p>
          </div>
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onDismiss}>
            Dismiss
          </Button>
          <Button variant="primary" onClick={onDownload}>
            Download
          </Button>
        </div>
      </div>
    </div>
  )
}
