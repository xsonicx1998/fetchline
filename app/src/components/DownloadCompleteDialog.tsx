import { useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Check } from 'lucide-react'
import { formatBytes } from '../lib/format'
import {
  getSettings,
  listDownloads,
  openFile,
  openFileWith,
  openFolder,
  playCompletionSound,
  updateSettings,
  withTimeout,
} from '../lib/api'
import type { DownloadRow, Settings } from '../lib/types'
import { Button } from './Button'
import { TitleBar } from './TitleBar'

/** Round 66 — the "Download complete" dialog, from the user's
 *  approved mockup (`workspace/download-complete-preview.html`).
 *  A separate OS window per finished download (opened by the
 *  `open_complete_dialog` Tauri command), frameless with the
 *  shared title bar — the real Sonic logo in it, "Download
 *  complete" as the title — and a purple→pink gradient divider
 *  under the title bar, like the progress bar. The body shows a
 *  cyan check (the "Done" status colour), the FILENAME as the
 *  big heading (not "Download complete" again — the title bar
 *  already says it, per the user's 02:52 EET call), "Downloaded
 *  X MB (N Bytes)", the Address (URL) and "The file saved as"
 *  (the full save path). The footer is Open (primary, the
 *  purple→pink gradient) / Open with... / Open folder / Close,
 *  plus a "Don't show this dialog again" checkbox that turns
 *  the dialog off in Settings.
 *
 *  The dialog replaces the progress window: the main window
 *  opens it when a download flips to 'complete', and the
 *  progress window closes itself at the same moment. Close /
 *  shut-down-after-download take precedence — when either is
 *  checked for the download, no dialog opens at all. The
 *  completion sound plays once when the dialog shows (the
 *  `play_completion_sound` command: the user's configured
 *  sound when one is set, else the built-in chime, else the
 *  Windows default sound). A sound failure is silent — a
 *  quiet dialog beats an error banner.
 *
 *  The row is polled until it appears (same pattern as the
 *  progress window); a download that vanished from the list
 *  shows the empty state instead of the dialog.
 *
 *  Round 87 (10) — ONE dialog for every completion, like the
 *  R85 error-dialog queue: several downloads finishing together
 *  used to stack one OS window each, so one Close click only
 *  dismissed the top copy. `open_complete_dialog` now opens a
 *  single window and emits `complete-dialog-queued` for later
 *  completions; this dialog keeps them in a queue and Close
 *  steps through them in the SAME window ("N more completed" +
 *  a "Dismiss all" button), so one click is always exactly one
 *  completion and the last Close closes the window. */
export function DownloadCompleteDialog({ downloadId }: { downloadId: string }) {
  const [download, setDownload] = useState<DownloadRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [settings, setSettings] = useState<Settings | null>(null)
  const [dontShow, setDontShow] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [pollError, setPollError] = useState<string | null>(null)
  const soundPlayedRef = useRef(false)
  const foundRef = useRef(false)
  /** Round 87 (10) — the queue of finished download ids this
   *  ONE window shows, head first. It starts with the id the
   *  window was opened for; later completions arrive through
   *  the `complete-dialog-queued` event (see the listener
   *  below). */
  const [queue, setQueue] = useState<string[]>([downloadId])
  const currentId = queue[0]
  const queuedCount = queue.length > 0 ? queue.length - 1 : 0

  // Dark only (Round 45): same as the progress window.
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
    void getSettings()
      .then((s) => setSettings(s))
      .catch(() => undefined)
  }, [])

  // Round 87 (10) — later completions queue into this window
  // instead of opening another one (Rust emits the event from
  // `open_complete_dialog`). Duplicates are ignored — the same
  // completion re-noticed across polls must not show twice.
  useEffect(() => {
    let unlisten: (() => void) | undefined
    void listen<{ download_id: string }>('complete-dialog-queued', (e) => {
      const id = e.payload.download_id
      if (typeof id === 'string' && id.length > 0) {
        setQueue((q) => (q.includes(id) ? q : [...q, id]))
      }
    })
      .then((fn) => {
        unlisten = fn
      })
      .catch(() => undefined)
    return () => {
      unlisten?.()
    }
  }, [])

  // Round 74 (14) — Escape closes the dialog, like X and Close.
  // (The error dialog already had this; the complete dialog did
  // not.) Round 87 (10) — Escape dismisses the current
  // completion like Close does (steps through the queue, closes
  // on the last).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dismissCurrent()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queue])

  // The completion sound, once, when the dialog shows.
  useEffect(() => {
    if (soundPlayedRef.current) return
    soundPlayedRef.current = true
    void playCompletionSound().catch(() => undefined)
  }, [])

  useEffect(() => {
    // Round 87 (10) — the queue's head changed (Close stepped
    // to the next completion): forget the previous row and poll
    // for this one.
    foundRef.current = false
    setDownload(null)
    setLoaded(false)
    setPollError(null)
    let stopped = false
    async function poll() {
      if (foundRef.current) return
      try {
        const rows = await withTimeout(
          listDownloads(),
          5000,
          'Backend did not respond within 5 seconds',
        )
        if (stopped) return
        const row = rows.find((d) => d.id === currentId) ?? null
        if (row) {
          foundRef.current = true
          setDownload(row)
          setLoaded(true)
          setPollError(null)
        }
      } catch (e) {
        if (stopped) return
        setPollError(e instanceof Error ? e.message : String(e))
        setLoaded(true)
      }
    }
    void poll()
    const timer = window.setInterval(() => void poll(), 500)
    return () => {
      stopped = true
      window.clearInterval(timer)
    }
  }, [currentId])

  // Round 84 — the R83 retest's Close button did not dismiss
  // the dialog while the X did, although both already called
  // the same `getCurrentWindow().close()` (and
  // `core:window:allow-close` IS in capabilities/default.json —
  // the missing-permission hypothesis is wrong). The remaining
  // explanation is a `close()` that rejects (a WebView2 flake)
  // and fails silently — `void` swallows it. `destroy()` is
  // the fallback so a failed close cannot strand the dialog.
  function closeDialog() {
    void getCurrentWindow()
      .close()
      .catch(() => getCurrentWindow().destroy().catch(() => undefined))
  }

  /** Round 87 (10) — Close / X / Escape dismisses THIS
   *  completion; when more are queued the dialog steps to the
   *  next one in the same window instead of closing, so one
   *  click is always exactly one completion. The last one
   *  closes the dialog. */
  function dismissCurrent() {
    if (queue.length > 1) {
      setQueue((q) => q.slice(1))
      return
    }
    closeDialog()
  }

  /** Round 87 (10) — "Dismiss all" clears the whole queue at
   *  once: the remaining completions stay visible as Finished
   *  rows in the main window, so nothing is lost. */
  function dismissAll() {
    closeDialog()
  }

  async function handleOpenFile() {
    setActionError(null)
    try {
      await openFile(currentId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenWith() {
    setActionError(null)
    try {
      await openFileWith(currentId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenFolder() {
    setActionError(null)
    try {
      await openFolder(currentId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  /** "Don't show this dialog again": persists at once, so a
   *  dialog that opens again in this session already knows.
   *  Re-enable it in Settings (General tab). */
  async function handleDontShowChange(checked: boolean) {
    setDontShow(checked)
    if (!settings) return
    try {
      const next = await updateSettings({
        ...settings,
        show_download_complete_dialog: !checked,
      })
      setSettings(next)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  if (!loaded || !download) {
    return (
      <div className="flex h-full flex-col bg-bg text-text">
        {/* Round 76 (3) — draggable: the user overrides Round 74
            (16) for this dialog, like the standalone Add URL. */}
        <TitleBar title="Download complete" showMinimize={false} showMaximize={false} draggable onClose={dismissCurrent} />
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          {pollError ? (
            <p className="text-sm text-danger">{pollError}</p>
          ) : (
            <p className="text-sm text-text-muted">
              {loaded
                ? 'This download is no longer in the list.'
                : 'Loading download…'}
            </p>
          )}
          <Button variant="secondary" onClick={dismissCurrent}>
            Close
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col bg-bg text-text">
      {/* Round 76 (3) — draggable: the user overrides Round 74
          (16) for this dialog, like the standalone Add URL. */}
      <TitleBar title="Download complete" showMinimize={false} showMaximize={false} draggable onClose={dismissCurrent} />
      {/* Purple→pink divider under the title bar, like the
          progress bar (approved mockup). */}
      <div
        aria-hidden="true"
        className="h-[2px] shrink-0 bg-gradient-to-r from-[#8B20FF] to-[#F02DCD]"
      />
      <div className="flex-1 overflow-y-auto px-5 py-4">
        <div className="flex items-start gap-3.5">
          <div className="flex h-[46px] w-[46px] shrink-0 items-center justify-center rounded-full border-[1.5px] border-info bg-info-soft text-info shadow-[0_0_18px_rgba(0,200,255,0.35)]">
            <Check className="h-6 w-6" strokeWidth={3} aria-hidden="true" />
          </div>
          <div className="min-w-0">
            <p className="truncate text-base font-bold" title={download.filename}>
              {download.filename}
            </p>
            <p className="mt-1 text-[13px] text-text-muted">
              Downloaded{' '}
              <b className="font-semibold text-text">
                {formatBytes(download.downloaded_bytes)} ({download.downloaded_bytes} Bytes)
              </b>
            </p>
          </div>
        </div>
        <div className="mt-4">
          <label className="mb-1 block text-xs font-semibold text-text-muted">Address</label>
          <div className="break-all rounded-md border border-border bg-surface px-2.5 py-2 text-[13px] text-text-muted">
            {download.final_url ?? download.url}
          </div>
        </div>
        <div className="mt-4">
          <label className="mb-1 block text-xs font-semibold text-text-muted">
            The file saved as
          </label>
          <div className="break-all rounded-md border border-border bg-surface px-2.5 py-2 text-[13px] text-text-muted">
            {download.save_path}
          </div>
        </div>
        {actionError ? <p className="mt-3 text-[13px] text-danger">{actionError}</p> : null}
      </div>
      {/* Round 87 (10) — the queue: while more completions wait,
          the footer says how many and offers "Dismiss all";
          Close dismisses just this one and steps to the next. */}
      <div className="flex shrink-0 items-center justify-between gap-3 px-5 pb-3 pt-1">
        {queuedCount > 0 ? (
          <p className="text-[13px] text-text-muted">
            {queuedCount} more completed
          </p>
        ) : (
          <span />
        )}
        <div className="flex shrink-0 items-center gap-2.5">
          {queuedCount > 0 ? (
            <Button variant="ghost" size="md" onClick={dismissAll}>
              Dismiss all
            </Button>
          ) : null}
        <Button
          variant="primary"
          size="md"
          onClick={handleOpenFile}
          style={{
            background: 'linear-gradient(90deg, #8B20FF, #F02DCD)',
            boxShadow: '0 4px 16px rgba(240,45,205,0.40)',
          }}
          className="hover:brightness-110"
        >
          Open
        </Button>
        <Button variant="secondary" size="md" onClick={handleOpenWith}>
          Open with...
        </Button>
        <Button variant="secondary" size="md" onClick={handleOpenFolder}>
          Open folder
        </Button>
        <Button variant="secondary" size="md" onClick={dismissCurrent}>
          Close
        </Button>
        </div>
      </div>
      <label className="flex shrink-0 cursor-pointer items-center gap-2 px-5 pb-4 text-xs text-text-muted">
        <input
          type="checkbox"
          checked={dontShow}
          onChange={(e) => void handleDontShowChange(e.target.checked)}
          className="h-[15px] w-[15px] accent-accent"
        />
        Don&apos;t show this dialog again
      </label>
    </div>
  )
}
