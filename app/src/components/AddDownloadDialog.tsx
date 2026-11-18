import { useEffect, useMemo, useRef, useState } from 'react'
import { Plus, X } from 'lucide-react'
import { open as openFolderPicker } from '@tauri-apps/plugin-dialog'
import {
  addDownload,
  confirmPendingLink,
  probeUrl,
  setDownloadDescription,
  setDownloadSpeedLimit,
  startDownload,
  updateCategory,
} from '../lib/api'
import type { CategoryRow, ProbeInfo } from '../lib/types'
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
  /** Round 67: open the "add a category" dialog above this one
   *  (App renders it after this dialog in the DOM, so it paints
   *  on top). */
  onAddCategory?: () => void
  /** Round 67: a browser-extension capture waiting for its
   *  confirm — the submit confirms this row (through the real
   *  add path, with its cookies) instead of adding a fresh
   *  download. */
  pendingLinkId?: string | null
  /** Round 67: the capture's browser cookies (JSON array), so
   *  the size probe works on cookie-gated links. */
  initialCookies?: string | null
}

/** The Save As field shows the full path (folder + filename).
 *  Split on the last separator so the filename part is the one
 *  the user edits; a bare name (no separator) is a rename — the
 *  folder stays. */
function splitSaveAs(value: string): { folder: string; filename: string } {
  const idx = Math.max(value.lastIndexOf('\\'), value.lastIndexOf('/'))
  if (idx < 0) return { folder: '', filename: value }
  return { folder: value.slice(0, idx), filename: value.slice(idx + 1) }
}

function joinSaveAs(folder: string, filename: string): string {
  const f = folder.replace(/[\\/]+$/, '')
  if (!f) return filename
  return filename ? `${f}\\${filename}` : `${f}\\`
}

/** S02 — add a new download (components.md: Dialog, TextInput, Dropdown).
 *  URL, file name, save folder and category. "Start now" begins the
 *  download at once; "Save for later" only queues it.
 *
 *  Round 67 — redesigned to the approved mockup (IDM's Download
 *  File Info in Sonic identity): dark title bar with the real
 *  logo + "Add URL" + X, a purple→pink gradient divider, fields
 *  in IDM order (URL, Category + "+", Save As as one editable
 *  full path, "Remember this path" with the file size in cyan on
 *  the same row, a read-only folder-path field under it,
 *  Description), and Download Later / Start Download / Cancel
 *  with the default-action button as a purple→pink gradient.
 *  Tightened vertical spacing so the dialog is shorter/squarer
 *  than the old one. The per-download speed cap stays (compact
 *  row at the bottom). */
export function AddDownloadDialog({
  open,
  categories,
  defaultAction = 'start_now',
  defaultFolder = '',
  initialUrl = '',
  onClose,
  onAdded,
  onLateError,
  onAddCategory,
  pendingLinkId = null,
  initialCookies = null,
}: AddDownloadDialogProps) {
  const [url, setUrl] = useState('')
  const [filename, setFilename] = useState('')
  const [folder, setFolder] = useState('')
  const [category, setCategory] = useState('')
  /** Per-download speed cap in KB/s (S12); 0 = no cap, the global cap still applies. */
  const [cap, setCap] = useState(0)
  const [description, setDescription] = useState('')
  const [rememberPath, setRememberPath] = useState(false)
  /** File size from the URL probe, for the cyan size on the
   *  remember-path row; null while unknown / unprobed. */
  const [fileSize, setFileSize] = useState<number | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)
  /** Round 55: a submit that already succeeded (any attempt) —
   *  a stale attempt's later failure is noise then, not an error
   *  to report. And the sequence of submits, so a retry supersedes
   *  the attempt before it. */
  const anySucceeded = useRef(false)
  const submitSeq = useRef(0)
  // Round 67: the user typed a folder of their own (Save As edit
  // or the "..." picker) — the category autofill below must not
  // overwrite it.
  const folderEdited = useRef(false)
  // Round 67: the user typed a filename of their own — the size
  // probe must not overwrite it with the server's name.
  const userEditedFilename = useRef(false)
  // Round 67: the URL the probe last answered for — a slow probe
  // must not clobber the size of a URL the user has moved on from.
  const probedUrl = useRef('')

  useEffect(() => {
    if (open) {
      setUrl(initialUrl)
      setFilename('')
      setFolder('')
      setCategory('')
      setCap(0)
      setDescription('')
      setRememberPath(false)
      setFileSize(null)
      setError(null)
      setBusy(false)
      folderEdited.current = false
      userEditedFilename.current = false
      probedUrl.current = ''
      urlRef.current?.focus()
    }
  }, [open, initialUrl, pendingLinkId])

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

  // Round 67 — probe the URL (debounced) for the file size
  // shown in cyan on the remember-path row, and prefill the
  // filename from the server when the user has not typed one.
  // A capture's cookies ride the probe, so a cookie-gated link
  // shows its size instead of failing with 401.
  useEffect(() => {
    if (!open) return
    const target = url.trim()
    if (!target) {
      setFileSize(null)
      return
    }
    const timer = window.setTimeout(async () => {
      try {
        const info: ProbeInfo = await probeUrl(target, initialCookies)
        if (probedUrl.current !== target) return
        setFileSize(info.size ?? null)
        if (!userEditedFilename.current && info.filename) {
          setFilename(info.filename)
        }
      } catch {
        if (probedUrl.current !== target) return
        setFileSize(null)
      }
    }, 500)
    probedUrl.current = target
    return () => window.clearTimeout(timer)
  }, [open, url, initialCookies])

  // Round 67 — IDM behaviour: picking a category fills the
  // folder with that category's default folder (the engine
  // resolves the same way when no folder is given), unless the
  // user already typed a folder of their own.
  useEffect(() => {
    if (folderEdited.current) return
    const cat = categories.find((c) => c.id === category)
    if (cat && cat.default_folder.trim()) {
      setFolder(cat.default_folder.trim())
    } else if (!cat) {
      setFolder('')
    }
  }, [category, categories])

  const categoryName = useMemo(
    () => categories.find((c) => c.id === category)?.name ?? '',
    [categories, category],
  )

  // Round 67 — the folder the file will land in: the typed one,
  // else the global default, else the OS Downloads folder (what
  // the engine falls back to). Shown read-only under the
  // remember-path row, with a trailing separator.
  const effectiveFolder = folder.trim() || defaultFolder?.trim() || 'Downloads'
  const folderDisplay = `${effectiveFolder.replace(/[\\/]+$/, '')}\\`

  // Round 67 — the Save As field shows the full path; editing it
  // splits on the last separator. A bare name is a rename — the
  // folder stays (the folder field below still shows it).
  const saveAsValue = joinSaveAs(folder, filename)
  const onSaveAsChange = (value: string) => {
    if (value === '') {
      setFolder('')
      setFilename('')
      folderEdited.current = true
      return
    }
    const { folder: f, filename: n } = splitSaveAs(value)
    if (f) {
      setFolder(f)
      folderEdited.current = true
    }
    if (n !== filename) userEditedFilename.current = true
    setFilename(n)
  }

  const onBrowseFolder = async () => {
    try {
      const picked = await openFolderPicker({
        directory: true,
        multiple: false,
        defaultPath: effectiveFolder || undefined,
      })
      if (typeof picked === 'string' && picked) {
        setFolder(picked)
        folderEdited.current = true
      }
    } catch {
      // The picker was cancelled or is unavailable — the user
      // can still type the folder in the Save As field.
    }
  }

  const onCategoryChange = (value: string) => {
    setCategory(value)
    if (!value) setRememberPath(false)
  }

  if (!open) return null

  const urlValid = url.trim() === '' || /^https?:\/\/.+/i.test(url.trim())

  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
  }

  async function submit(startNow: boolean) {
    if (!url.trim() || !urlValid) {
      setError('Enter a link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    const runId = ++submitSeq.current
    const work: Promise<string> = (async () => {
      const folderValue = folder.trim() || null
      const categoryValue = category || null
      // Round 67 — "Remember this path" stores the folder as
      // the category's default folder, so later adds to that
      // category land there too. A failure here must not block
      // the download itself.
      if (rememberPath && categoryValue && folderValue) {
        try {
          await updateCategory({ id: categoryValue, folder: folderValue })
        } catch {
          // Keep going — the download still starts.
        }
      }
      if (pendingLinkId) {
        // Round 67 — a browser capture: confirm it through the
        // real add path with its cookies. The engine applies
        // the speed cap before the start and starts it when
        // asked; it marks the row rejected with the reason when
        // the add itself fails, so the dialog shows the error
        // and stays open for the user to fix it.
        const [id] = await confirmPendingLink({
          id: pendingLinkId,
          filename: filename.trim() || null,
          folder: folderValue,
          category: categoryValue,
          queue: startNow ? null : 'queue-main',
          speedLimitKbps: cap > 0 ? cap : null,
          startNow,
        })
        if (description.trim()) {
          try {
            await setDownloadDescription(id, description.trim())
          } catch {
            // Non-fatal — the download is already added.
          }
        }
        return id
      }
      const [id] = await addDownload({
        url: url.trim(),
        filename: filename.trim() || null,
        folder: folderValue,
        category: categoryValue,
        // "Save for later" files the download in the main queue, so
        // the scheduler (or Start Now) can pick it up later.
        queue: startNow ? null : 'queue-main',
      })
      // Round 67 — the Description note, stored right after the
      // add. Non-fatal: the download is already added.
      if (description.trim()) {
        try {
          await setDownloadDescription(id, description.trim())
        } catch {
          // Keep going — the download still starts.
        }
      }
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

  const primaryIsLater = defaultAction === 'save_for_later'
  const primaryStyle = {
    background: 'linear-gradient(90deg, #8B20FF, #F02DCD)',
    boxShadow: '0 4px 16px rgba(240,45,205,0.40)',
  } as const

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
      <div className="w-full max-w-[620px] overflow-hidden rounded-xl border border-[#8B20FF]/45 bg-bg text-text shadow-[0_0_0_1px_rgba(0,200,255,0.12),0_24px_70px_rgba(0,0,0,0.65),0_0_44px_rgba(139,32,255,0.22)]">
        {/* Title bar — the real logo, "Add URL", X. */}
        <div className="flex items-center gap-2.5 px-3.5 py-3">
          <img
            src="/icon.png"
            alt="Sonic Manager logo"
            className="h-7 w-7 rounded-full shadow-[0_0_12px_rgba(139,32,255,0.6)]"
          />
          <h2 id="add-download-title" className="flex-1 text-[15px] font-semibold">
            Add URL
          </h2>
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted transition-colors hover:bg-[#F02DCD]/20 hover:text-text"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>
        <div className="h-[2px] bg-gradient-to-r from-accent to-[#F02DCD]" />

        {/* Body — IDM field order, tightened spacing. */}
        <div className="flex flex-col gap-[11px] px-[22px] pb-1 pt-4">
          <div>
            <label htmlFor="add-url" className="mb-1 block text-xs font-semibold text-text-muted">
              URL
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
              className="h-9 w-full rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          <div>
            <label
              htmlFor="add-category"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Category
            </label>
            <div className="flex gap-2">
              <select
                id="add-category"
                value={category}
                onChange={(e) => onCategoryChange(e.target.value)}
                className="h-9 min-w-0 grow rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
              >
                <option value="">Pick automatically from the file type</option>
                {categories.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
              <button
                type="button"
                onClick={() => onAddCategory?.()}
                disabled={busy}
                title="Add category"
                aria-label="Add category"
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-surface-2 text-text transition-colors hover:border-accent hover:text-accent"
              >
                <Plus className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
          </div>

          <div>
            <label
              htmlFor="add-save-as"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Save As
            </label>
            <div className="flex gap-2">
              <input
                id="add-save-as"
                type="text"
                value={saveAsValue}
                onChange={(e) => onSaveAsChange(e.target.value)}
                placeholder="filename.ext"
                title="You can rename the file here"
                className="h-9 min-w-0 grow rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
              />
              <button
                type="button"
                onClick={() => void onBrowseFolder()}
                disabled={busy}
                title="Choose folder"
                aria-label="Choose folder"
                className="h-9 w-9 shrink-0 rounded-md border border-border bg-surface-2 text-sm font-bold leading-none text-text transition-colors hover:border-accent hover:text-accent"
              >
                …
              </button>
            </div>
          </div>

          <div className="flex items-center gap-2 text-xs text-text-muted">
            <input
              id="add-remember"
              type="checkbox"
              checked={rememberPath}
              onChange={(e) => setRememberPath(e.target.checked)}
              disabled={!category || busy}
              className="h-[15px] w-[15px] accent-[#8B20FF]"
            />
            <label htmlFor="add-remember" className="min-w-0 grow truncate">
              {category
                ? `Remember this path for “${categoryName}” category`
                : 'Remember this path for this category'}
            </label>
            {fileSize !== null && (
              <span className="shrink-0 text-[13px] font-semibold text-info">
                {formatBytes(fileSize)}
              </span>
            )}
          </div>

          <div>
            <input
              value={folderDisplay}
              readOnly
              title="Save folder"
              aria-label="Save folder"
              className="h-9 w-full cursor-default rounded-md border border-border-input bg-surface-2/60 px-3 text-[13px] text-text-muted focus:outline-none"
            />
          </div>

          <div>
            <label
              htmlFor="add-description"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Description
            </label>
            <input
              id="add-description"
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Add a description (optional)"
              className="h-9 w-full rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          <div className="flex items-center gap-3">
            <label
              htmlFor="add-cap"
              className="min-w-0 grow text-xs font-semibold text-text-muted"
            >
              Speed cap for this download (KB/s, 0 = no cap)
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
              className="h-9 w-32 shrink-0 rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          {error ? (
            <p id="add-error" role="alert" className="text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        {/* Actions — the default-action button is the gradient
         *  primary (with the default setting that is Start
         *  Download, exactly as the mockup shows). */}
        <div className="flex items-center justify-end gap-2.5 px-[22px] pb-[18px] pt-4">
          <Button
            variant={primaryIsLater ? 'primary' : 'secondary'}
            style={primaryIsLater ? primaryStyle : undefined}
            className={primaryIsLater ? 'px-6 hover:brightness-110' : undefined}
            loading={busy}
            onClick={() => void submit(false)}
          >
            Download Later
          </Button>
          <Button
            variant={primaryIsLater ? 'secondary' : 'primary'}
            style={primaryIsLater ? primaryStyle : undefined}
            className={primaryIsLater ? 'px-6 hover:brightness-110' : undefined}
            loading={busy}
            onClick={() => void submit(true)}
          >
            Start Download
          </Button>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  )
}
