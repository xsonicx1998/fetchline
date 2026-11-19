import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { refreshDownloadAddress } from '../lib/api'
import { formatBytes } from '../lib/format'
import type { DownloadRow, RefreshAddressResult } from '../lib/types'
import { Button } from './Button'

interface RefreshAddressDialogProps {
  open: boolean
  download: DownloadRow | null
  onClose: () => void
  onRefreshed: () => void
}

type Mismatch = Extract<RefreshAddressResult, { outcome: 'mismatch' }>

/** Round 51 — refresh download address: replace an expired link
 *  on a paused/failed download without losing progress. The engine
 *  re-probes the new link first: the same file resumes from the
 *  downloaded bytes; a different file is reported as a mismatch
 *  and only switches when the user confirms (which restarts the
 *  download from zero); a probe failure is an error and the old
 *  link stays. */
export function RefreshAddressDialog({
  open,
  download,
  onClose,
  onRefreshed,
}: RefreshAddressDialogProps) {
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [mismatch, setMismatch] = useState<Mismatch | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setUrl('')
      setBusy(false)
      setError(null)
      setMismatch(null)
      urlRef.current?.focus()
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open || !download) return null
  const target = download

  const urlValid = url.trim() === '' || /^(https?|ftp):\/\/.+/i.test(url.trim())

  async function submit(force: boolean) {
    if (!url.trim() || !urlValid) {
      setError('Enter a link starting with http://, https:// or ftp://')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const result = await refreshDownloadAddress(target.id, url.trim(), force)
      if (result.outcome === 'switched') {
        onRefreshed()
        onClose()
      } else {
        // A different file: nothing changed — show the warning
        // and let the user decide, instead of switching silently.
        setMismatch(result)
      }
    } catch (e) {
      // Probe failure (or a refused state): the old link and all
      // progress are still there — the error says so.
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="refresh-address-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="refresh-address-title" className="text-lg font-semibold">
            Refresh download address
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

        <div className="flex flex-col gap-4">
          <p className="text-sm text-text-muted">
            <span className="font-semibold text-text">{target.filename}</span> —{' '}
            {formatBytes(target.downloaded_bytes)} of {formatBytes(target.size_bytes)} downloaded
            so far. Paste the new link and the app checks it first: if it is the same file, the
            download resumes from where it stopped.
          </p>

          <div>
            <label className="mb-1 block text-sm font-semibold">Current link</label>
            <p className="break-all rounded-md border border-border bg-bg px-3 py-2 font-mono text-sm text-text-muted">
              {target.url}
            </p>
          </div>

          <div>
            <label htmlFor="refresh-url" className="mb-1 block text-sm font-semibold">
              New link
            </label>
            <input
              ref={urlRef}
              id="refresh-url"
              type="url"
              value={url}
              disabled={mismatch !== null}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com/file.zip"
              aria-invalid={!urlValid || undefined}
              aria-describedby={error ? 'refresh-error' : undefined}
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent disabled:opacity-60"
            />
          </div>

          {mismatch ? (
            <div role="alert" className="rounded-md border border-danger bg-danger-soft p-3 text-sm">
              <p className="font-semibold text-danger">
                The new link serves a different file — nothing was changed yet.
              </p>
              <p className="mt-1 text-text">
                Old file: {formatBytes(target.size_bytes)}
                {target.etag ? ` (${target.etag})` : ''} · New file:{' '}
                {formatBytes(mismatch.size_bytes)}
                {mismatch.filename ? ` (${mismatch.filename})` : ''}
                {mismatch.etag ? ` (${mismatch.etag})` : ''}. Switching will restart the download
                from the beginning and the {formatBytes(target.downloaded_bytes)} already
                downloaded will be lost.
              </p>
            </div>
          ) : null}

          {error ? (
            <p id="refresh-error" role="alert" className="text-sm text-danger">
              {error} The old link and your progress are still there.
            </p>
          ) : null}
        </div>

        <div className="mt-6 flex justify-end gap-2">
          {mismatch ? (
            <>
              <Button
                variant="ghost"
                onClick={() => {
                  setMismatch(null)
                  setError(null)
                }}
              >
                Keep the old link
              </Button>
              <Button variant="primary" loading={busy} onClick={() => void submit(true)}>
                Switch anyway — restart from zero
              </Button>
            </>
          ) : (
            <>
              <Button variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button variant="primary" loading={busy} onClick={() => void submit(false)}>
                Check &amp; resume
              </Button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
