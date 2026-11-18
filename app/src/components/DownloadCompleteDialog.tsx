import { useEffect, useRef, useState } from 'react'
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
 *  shows the empty state instead of the dialog. */
export function DownloadCompleteDialog({ downloadId }: { downloadId: string }) {
  const [download, setDownload] = useState<DownloadRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [settings, setSettings] = useState<Settings | null>(null)
  const [dontShow, setDontShow] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [pollError, setPollError] = useState<string | null>(null)
  const soundPlayedRef = useRef(false)
  const foundRef = useRef(false)

  // Dark only (Round 45): same as the progress window.
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
    void getSettings()
      .then((s) => setSettings(s))
      .catch(() => undefined)
  }, [])

  // The completion sound, once, when the dialog shows.
  useEffect(() => {
    if (soundPlayedRef.current) return
    soundPlayedRef.current = true
    void playCompletionSound().catch(() => undefined)
  }, [])

  useEffect(() => {
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
        const row = rows.find((d) => d.id === downloadId) ?? null
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
  }, [downloadId])

  function closeDialog() {
    void getCurrentWindow().close()
  }

  async function handleOpenFile() {
    setActionError(null)
    try {
      await openFile(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenWith() {
    setActionError(null)
    try {
      await openFileWith(downloadId)
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e))
    }
  }

  async function handleOpenFolder() {
    setActionError(null)
    try {
      await openFolder(downloadId)
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
        <TitleBar title="Download complete" showMinimize={false} showMaximize={false} />
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
          <Button variant="secondary" onClick={closeDialog}>
            Close
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col bg-bg text-text">
      <TitleBar title="Download complete" showMinimize={false} showMaximize={false} />
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
      <div className="flex shrink-0 justify-end gap-2.5 px-5 pb-3 pt-1">
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
        <Button variant="secondary" size="md" onClick={closeDialog}>
          Close
        </Button>
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
