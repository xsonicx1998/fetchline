import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { createCategory } from '../lib/api'
import { Button } from './Button'

interface AddCategoryDialogProps {
  open: boolean
  onClose: () => void
  onAdded: () => void
}

/** S04 — create a custom category (components.md: Dialog, TextInput).
 *  Name, save folder and file extensions. Extensions are typed as a
 *  comma-separated list ("mp4, mkv") and sent without dots. */
export function AddCategoryDialog({ open, onClose, onAdded }: AddCategoryDialogProps) {
  const [name, setName] = useState('')
  const [folder, setFolder] = useState('')
  const [extensions, setExtensions] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setName('')
      setFolder('')
      setExtensions('')
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
    const exts = extensions
      .split(',')
      .map((e) => e.trim().replace(/^\./, '').toLowerCase())
      .filter((e) => e.length > 0)
    if (!name.trim()) {
      setError('Give the category a name.')
      return
    }
    if (!folder.trim()) {
      setError('Choose the folder files in this category go to.')
      return
    }
    if (exts.length === 0) {
      setError('Add at least one file type, for example "mp4".')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await createCategory({ name: name.trim(), folder: folder.trim(), extensions: exts })
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
      aria-labelledby="add-category-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="add-category-title" className="text-lg font-semibold">
            New category
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
            <label htmlFor="add-cat-name" className="mb-1 block text-sm font-semibold">
              Name
            </label>
            <input
              ref={nameRef}
              id="add-cat-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Podcasts"
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            />
          </div>
          <div>
            <label htmlFor="add-cat-folder" className="mb-1 block text-sm font-semibold">
              Save to folder
            </label>
            <input
              id="add-cat-folder"
              type="text"
              value={folder}
              onChange={(e) => setFolder(e.target.value)}
              placeholder="C:\Users\you\Downloads\Podcasts"
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 font-mono text-base focus:border-accent"
            />
          </div>
          <div>
            <label htmlFor="add-cat-ext" className="mb-1 block text-sm font-semibold">
              File types <span className="font-normal text-text-muted">(comma-separated, without dots)</span>
            </label>
            <input
              id="add-cat-ext"
              type="text"
              value={extensions}
              onChange={(e) => setExtensions(e.target.value)}
              placeholder="mp3, m4a, ogg"
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 font-mono text-base focus:border-accent"
            />
            <p className="mt-1 text-xs text-text-muted">
              New downloads with one of these file types are put in this category automatically.
            </p>
          </div>
          {error ? (
            <p role="alert" className="text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void submit()}>
            Create category
          </Button>
        </div>
      </div>
    </div>
  )
}
