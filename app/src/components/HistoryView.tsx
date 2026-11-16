import { useEffect, useState } from 'react'
import { Download, FolderOpen, History, Search, Trash2 } from 'lucide-react'
import {
  clearDownloadHistory,
  listDownloadHistory,
  openHistoryFolder,
  redownloadFromHistory,
} from '../lib/api'
import { formatBytes, formatDateTime } from '../lib/format'
import type { HistoryRow } from '../lib/types'
import { Button } from './Button'

interface HistoryViewProps {
  /** A re-download succeeded — the caller switches back to the
   *  downloads view so the new download is visible. */
  onRedownloaded: () => void
  /** Surface a failed action in the app's error banner. */
  onError: (message: string) => void
}

/** Round 53 — download history: every finished download, kept
 *  after it leaves the main table. Search matches filename or URL
 *  (debounced, so typing does not hammer the engine); each row can
 *  open its folder or download the file again; Clear history erases
 *  the records — never the files on disk — after the same
 *  confirmation the Remove dialog uses. */
export function HistoryView({ onRedownloaded, onError }: HistoryViewProps) {
  const [rows, setRows] = useState<HistoryRow[]>([])
  const [query, setQuery] = useState('')
  const [confirmClear, setConfirmClear] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [clearing, setClearing] = useState(false)

  async function fetchHistory(q: string) {
    try {
      setRows(await listDownloadHistory(q))
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  // Fetch on open, then again ~300 ms after the search text
  // settles — the debounce keeps a fast typist to one call.
  useEffect(() => {
    const timer = window.setTimeout(
      () => void fetchHistory(query),
      query === '' ? 0 : 300,
    )
    return () => window.clearTimeout(timer)
    // fetchHistory / onError are stable in effect (setRows and the
    // app's setError); listing them would re-run on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query])

  async function handleOpenFolder(row: HistoryRow) {
    try {
      await openHistoryFolder(row.id)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleRedownload(row: HistoryRow) {
    setBusyId(row.id)
    try {
      await redownloadFromHistory(row.id)
      onRedownloaded()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyId(null)
    }
  }

  async function handleClear() {
    setClearing(true)
    try {
      await clearDownloadHistory()
      setConfirmClear(false)
      await fetchHistory(query)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setClearing(false)
    }
  }

  const searching = query.trim() !== ''

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
        <div className="relative min-w-0 flex-1">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-text-muted"
            aria-hidden="true"
          />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search history by name or link…"
            aria-label="Search download history"
            className="h-8 w-full rounded-md border border-border-input bg-bg pl-8 pr-3 text-base focus:border-accent"
          />
        </div>
        <Button
          variant="danger"
          size="sm"
          disabled={rows.length === 0}
          onClick={() => setConfirmClear(true)}
        >
          <Trash2 className="h-4 w-4" aria-hidden="true" />
          Clear history
        </Button>
      </div>

      {rows.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-4 p-6 text-center">
          <History className="icon-glow-white h-16 w-16 text-white" aria-hidden="true" />
          <p className="text-xl font-bold">
            {searching ? 'No matches in history' : 'No download history yet'}
          </p>
          <p className="max-w-md text-[15px] text-text-muted">
            {searching
              ? `Nothing in your history matches “${query.trim()}”. Try a different name or link.`
              : 'Finished downloads are recorded here, even after they leave the main list — so a file can be found again, re-downloaded, or its folder opened.'}
          </p>
        </div>
      ) : (
        <table className="w-full border-collapse text-base" aria-label="Download history">
          <thead className="sticky top-0 bg-surface">
            <tr className="border-b border-border text-left text-xs font-semibold uppercase text-text-muted">
              <th scope="col" className="px-3 py-2">
                File name
              </th>
              <th scope="col" className="px-3 py-2">
                Size
              </th>
              <th scope="col" className="px-3 py-2">
                Completed
              </th>
              <th scope="col" className="px-3 py-2">
                Folder
              </th>
              <th scope="col" className="px-3 py-2">
                URL
              </th>
              <th scope="col" className="px-3 py-2">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.id} className="h-10 border-b border-border hover:bg-surface">
                <td className="max-w-0 px-3 py-2" title={row.filename}>
                  <span className="block truncate font-medium">{row.filename}</span>
                </td>
                <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                  {formatBytes(row.size_bytes)}
                </td>
                <td className="whitespace-nowrap px-3 py-2 font-mono text-sm text-text-muted">
                  {formatDateTime(row.completed_at)}
                </td>
                <td className="max-w-0 px-3 py-2 text-sm text-text-muted" title={row.folder}>
                  <span className="block truncate">{row.folder}</span>
                </td>
                <td
                  className="max-w-0 px-3 py-2 text-sm text-text-muted"
                  title={row.final_url ?? row.url}
                >
                  <span className="block truncate">{row.final_url ?? row.url}</span>
                </td>
                <td className="whitespace-nowrap px-3 py-2">
                  <span className="flex items-center justify-end gap-1">
                    <Button variant="ghost" size="sm" onClick={() => void handleOpenFolder(row)}>
                      <FolderOpen className="h-4 w-4" aria-hidden="true" />
                      Open folder
                    </Button>
                    <Button
                      variant="secondary"
                      size="sm"
                      loading={busyId === row.id}
                      onClick={() => void handleRedownload(row)}
                    >
                      <Download className="h-4 w-4" aria-hidden="true" />
                      Re-download
                    </Button>
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {confirmClear ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="confirm-clear-history-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
          onClick={(e) => {
            if (e.target === e.currentTarget) setConfirmClear(false)
          }}
        >
          <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <h2 id="confirm-clear-history-title" className="text-lg font-semibold">
              Clear download history?
            </h2>
            <p className="mt-2 text-sm text-text-muted">
              Every history record will be erased. The files on disk are kept — only the list
              goes.
            </p>
            <div className="mt-6 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setConfirmClear(false)}>
                Keep history
              </Button>
              <Button variant="danger" loading={clearing} onClick={() => void handleClear()}>
                Clear history
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
