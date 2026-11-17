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
  /** Round 56 (§7.5): the configured default download folder,
   *  shown as the "Save to folder" placeholder so the field no
   *  longer shows a made-up path. Empty when no folder is
   *  configured — the placeholder then says so, and the engine
   *  falls back to the OS Downloads folder. */
  defaultFolder?: string
  /** S08: prefill the URL field (the video panel passes the picked
   *  video's link). Empty by default — the dialog clears on open. */
  initialUrl?: string
  onClose: () => void
  /** Round 55 (defect 3a): the created download's id, so the
   *  caller can switch to "All downloads" and select the row. */
  onAdded: (id: string) => void
  /** Round 55 (defect 3a/3b): a submit that finishes after the
   *  dialog timed out or closed reports its failure here, so it
   *  lands in the main error banner instead of vanishing. */
  onLateError: (message: string) => void
}

/** S02 — add a new download (components.md: Dialog, TextInput, Dropdown).
 *  URL, file name, save folder and category. "Start now" begins the
 *  download at once; "Save for later" only queues it. */
export function AddDownloadDialog({
  open,
  categories,
  defaultAction = 'start_now',
  defaultFolder = '',
  initialUrl = '',
  onClose,
  onAdded,
  onLateError,
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
  /** Round 55: a submit that already succeeded (any attempt) —
   *  a stale attempt's later failure is noise then, not an error
   *  to report. And the sequence of submits, so a retry supersedes
   *  the attempt before it. */
  const anySucceeded = useRef(false)
  const submitSeq = useRef(0)

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
      // Round 55 (defect 3a): Escape must not close the dialog
      // while a submit is in flight — the probe error would land
      // on an unmounted component and vanish.
      if (e.key === 'Escape' && !busy) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose, busy])

  if (!open) return null

  const urlValid = url.trim() === '' || /^https?:\/\/.+/i.test(url.trim())

  async function submit(startNow: boolean) {
    if (!url.trim() || !urlValid) {
      setError('Enter a link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    const runId = ++submitSeq.current
    const work: Promise<string> = (async () => {
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
      return id
    })()
    // Round 55 (defect 3a/3b): the submit promise outlives the
    // dialog — a late outcome (after a timeout, or after the dialog
    // was closed) still reaches the main window: a late success adds
    // the row, a late failure shows in the main error banner instead
    // of dying on an unmounted component. `consumed` marks an
    // outcome this submit already handled itself, so it is never
    // reported twice; a failure after some submit already succeeded
    // is not reported — the download exists, the stale attempt is
    // noise.
    let consumed = false
    void work.then(
      (id) => {
        if (!consumed) onAdded(id)
      },
      (e: unknown) => {
        if (!consumed && !anySucceeded.current) {
          onLateError(e instanceof Error ? e.message : String(e))
        }
      },
    )
    // Round 55 (defect 3b): the submit must not hang silently —
    // the probe used to stall for minutes with no error. 60 s is
    // deliberately longer than the engine's 30 s total timeout on
    // the probe client (CLIENT_TOTAL_TIMEOUT in
    // app/engine/src/proxy.rs), so the probe + store + start have
    // time to settle before the dialog reports a timeout.
    // (Round 58: the old comment claimed 60 s "matches" the
    // engine timeout — it never did; the engine bound is 30 s.)
    // On timeout the dialog stays open with the error and the user
    // can retry or close; the first attempt keeps running and its
    // late outcome is reported by the handler above.
    let timedOut = false
    const timeout = new Promise<never>((_, reject) => {
      window.setTimeout(() => {
        timedOut = true
        reject(
          new Error(
            'The server took too long to respond (over 60 seconds) — check the link and try again',
          ),
        )
      }, 60_000)
    })
    try {
      const id = await Promise.race([work, timeout])
      // Superseded by a retry: leave the outcome unclaimed, so the
      // detached handler above still reports this attempt's late
      // success (its row exists and should be shown).
      if (submitSeq.current !== runId) return
      consumed = true
      anySucceeded.current = true
      setBusy(false)
      onAdded(id)
      onClose()
    } catch (e) {
      // A work failure (not a timeout) is already claimed by the
      // detached handler above — mark it consumed so it is not
      // reported twice. A timeout leaves it unclaimed: the attempt
      // is still running and its late outcome will be reported.
      if (!timedOut) consumed = true
      if (submitSeq.current !== runId) return
      setBusy(false)
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="add-download-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        // Round 55 (defect 3a): a backdrop click must not close the
        // dialog while a submit is in flight.
        if (e.target === e.currentTarget && !busy) onClose()
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
            disabled={busy}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2 disabled:opacity-45"
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
              placeholder={defaultFolder || 'Leave empty for Downloads'}
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
            {/* Round 56 (§7.5): type="text" + inputMode="numeric",
             *  not type="number" — a number input ignores Ctrl+A /
             *  select-all in this webview, so typing appended to
             *  the old value ("0512") instead of replacing it.
             *  Focus selects the whole value, and non-digits are
             *  stripped on input, so the field still only ever
             *  holds a non-negative integer. */}
            <input
              id="add-cap"
              type="text"
              inputMode="numeric"
              value={cap}
              onChange={(e) => setCap(Math.max(0, Number(e.target.value.replace(/[^0-9]/g, '')) || 0))}
              onFocus={(e) => e.target.select()}
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
          <Button variant="ghost" onClick={onClose} disabled={busy}>
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
