import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { addDownload, setDownloadSpeedLimit, startDownload } from '../lib/api'
import type { CategoryRow } from '../lib/types'
import { Button } from './Button'

interface AddDownloadDialogProps {
  open: boolean
  categories: CategoryRow[]
  /** S09 Downloads setting: which button is the primary one. */
  defaultAction?: 'start_now' | 'save_for_later'
  /** S08: prefill the URL field (the video panel passes the picked
   *  video's link). Empty by default — the dialog clears on open. */
  initialUrl?: string
  onClose: () => void
  onAdded: () => void
}

/** S02 — add a new download (components.md: Dialog, TextInput, Dropdown).
 *  URL, file name, save folder and category. "Start now" begins the
 *  download at once; "Save for later" only queues it. */
export function AddDownloadDialog({
  open,
  categories,
  defaultAction = 'start_now',
  initialUrl = '',
  onClose,
  onAdded,
}: AddDownloadDialogProps) {
  const [url, setUrl] = useState('')
  const [filename, setFilename] = useState('')
  const [folder, setFolder] = useState('')
  const [category, setCategory] = useState('')
  /** Per-download speed cap in KB/s (S12); 0 = no cap, the global cap still applies. */
  const [cap, setCap] = useState(0)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setUrl(initialUrl)
      setFilename('')
      setFolder('')
      setCategory('')
      setCap(0)
      setError(null)
      setBusy(false)
      urlRef.current?.focus()
    }
  }, [open, initialUrl])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  const urlValid = url.trim() === '' || /^https?:\/\/.+/i.test(url.trim())

  async function submit(startNow: boolean) {
    if (!url.trim() || !urlValid) {
      setError('Enter a link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const [id] = await addDownload({
        url: url.trim(),
        filename: filename.trim() || null,
        folder: folder.trim() || null,
        category: category || null,
        // "Save for later" files the download in the main queue, so
        // the scheduler (or Start Now) can pick it up later.
        queue: startNow ? null : 'queue-main',
      })
      // Per-download cap (S12): stored on the row before the
      // download starts, so the cap applies from the first byte.
      if (cap > 0) {
        await setDownloadSpeedLimit(id, cap)
      }
      if (startNow) {
        await startDownload(id)
      }
      onAdded()
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
      aria-labelledby="add-download-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="add-download-title" className="text-lg font-semibold">
            Add a new download
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
          <div>
            <label htmlFor="add-url" className="mb-1 block text-sm font-semibold">
              Link
            </label>
            <input
              ref={urlRef}
              id="add-url"
              type="url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com/file.zip"
              aria-invalid={!urlValid || undefined}
              aria-describedby={error ? 'add-error' : undefined}
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            />
          </div>

          <div>
            <label htmlFor="add-filename" className="mb-1 block text-sm font-semibold">
              File name <span className="font-normal text-text-muted">(leave empty to use the link&apos;s name)</span>
            </label>
            <input
              id="add-filename"
              type="text"
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
              placeholder="file.zip"
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            />
          </div>

          <div>
            <label htmlFor="add-folder" className="mb-1 block text-sm font-semibold">
              Save to folder <span className="font-normal text-text-muted">(leave empty for your Downloads folder)</span>
            </label>
            <input
              id="add-folder"
              type="text"
              value={folder}
              onChange={(e) => setFolder(e.target.value)}
              placeholder="C:\\Users\\you\\Downloads"
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 font-mono text-base focus:border-accent"
            />
          </div>

          <div>
            <label htmlFor="add-category" className="mb-1 block text-sm font-semibold">
              Category
            </label>
            <select
              id="add-category"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            >
              <option value="">Pick automatically from the file type</option>
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </div>

          <div>
            <label htmlFor="add-cap" className="mb-1 block text-sm font-semibold">
              Speed cap for this download <span className="font-normal text-text-muted">(KB/s, 0 = no cap)</span>
            </label>
            <input
              id="add-cap"
              type="number"
              min={0}
              max={10000000}
              value={cap}
              onChange={(e) => setCap(Math.max(0, Number(e.target.value) || 0))}
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            />
            <p className="mt-1 text-sm text-text-muted">
              Caps only this download; the app-wide limit still applies, and the slower of the two wins.
            </p>
          </div>

          {error ? (
            <p id="add-error" role="alert" className="text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant={defaultAction === 'save_for_later' ? 'primary' : 'secondary'}
            loading={busy}
            onClick={() => void submit(false)}
          >
            Save for later
          </Button>
          <Button
            variant={defaultAction === 'save_for_later' ? 'secondary' : 'primary'}
            loading={busy}
            onClick={() => void submit(true)}
          >
            Start download
          </Button>
        </div>
      </div>
    </div>
  )
}
