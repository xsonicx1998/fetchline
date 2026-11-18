import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { listDownloads, playErrorSound, withTimeout } from '../lib/api'
import type { DownloadRow } from '../lib/types'
import { Button } from './Button'
import { TitleBar } from './TitleBar'

/** Round 68 — the "Download failed" dialog, from the user's
 *  approved mockup (`workspace/download-failed-preview.html`),
 *  which follows IDM's real error popup (the screenshot he sent:
 *  a small dialog over the progress window, warning triangle +
 *  "An error occurred during download. Please try again." +
 *  "Details:" + the reason + OK). A separate OS window per
 *  failed download (opened by the `open_error_dialog` Tauri
 *  command), frameless with the shared title bar — the FILENAME
 *  as the title (like IDM's popup) — and a purple→pink gradient
 *  divider under the title bar, like the progress bar. The body
 *  shows a yellow warning triangle, the message, and "Details:"
 *  with the ACTUAL engine error for this download
 *  (`error_message` — connection lost, server not found, 404,
 *  no disk space...), never a fixed string. One OK button;
 *  pressing OK (or X) closes the dialog and returns to the
 *  progress window, which stays open behind it with its Error
 *  badge and Resume button — Resume continues from where the
 *  download stopped.
 *
 *  The error alert plays once when the dialog shows (the
 *  `play_error_sound` command: the user's configured
 *  `download_failed` sound when one is set, else the built-in
 *  descending alert, else the Windows default sound). A sound
 *  failure is silent — a quiet dialog beats an error banner.
 *
 *  The row is polled until it appears (same pattern as the
 *  progress window and the complete dialog); a download that
 *  vanished from the list shows the empty state instead. */
export function DownloadErrorDialog({ downloadId }: { downloadId: string }) {
  const [download, setDownload] = useState<DownloadRow | null>(null)
  const [loaded, setLoaded] = useState(false)
  const [pollError, setPollError] = useState<string | null>(null)
  const soundPlayedRef = useRef(false)
  const foundRef = useRef(false)

  // Dark only (Round 45): same as the progress window.
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
  }, [])

  // The error alert, once, when the dialog shows.
  useEffect(() => {
    if (soundPlayedRef.current) return
    soundPlayedRef.current = true
    void playErrorSound().catch(() => undefined)
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

  if (!loaded || !download) {
    return (
      <div className="flex h-full flex-col bg-bg text-text">
        <TitleBar title="Download failed" showMinimize={false} showMaximize={false} />
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
      <TitleBar title={download.filename} showMinimize={false} showMaximize={false} />
      {/* Purple→pink divider under the title bar, like the
          progress bar (approved mockup). */}
      <div
        aria-hidden="true"
        className="h-[2px] shrink-0 bg-gradient-to-r from-[#8B20FF] to-[#F02DCD]"
      />
      <div className="flex flex-1 items-start gap-4 overflow-y-auto px-6 py-5">
        {/* Yellow warning triangle, like IDM's popup. */}
        <svg
          className="h-[46px] w-[52px] shrink-0"
          viewBox="0 0 52 46"
          aria-hidden="true"
        >
          <path
            d="M26 3 L50 43 L2 43 Z"
            fill="#F5B301"
            stroke="#8a6500"
            strokeWidth="1.5"
            strokeLinejoin="round"
          />
          <rect x="24" y="16" width="4" height="14" fill="#0B0F24" />
          <circle cx="26" cy="36" r="2.4" fill="#0B0F24" />
        </svg>
        <div className="min-w-0">
          <p className="text-[15px] leading-relaxed">
            An error occurred during download. Please try again.
          </p>
          <p className="mt-3 text-[15px] leading-relaxed">
            <b className="font-semibold">Details:</b>
            <br />
            <span className="text-danger">
              {download.error_message ?? 'The download failed for an unknown reason.'}
            </span>
          </p>
        </div>
      </div>
      <div className="flex shrink-0 justify-end px-6 pb-5">
        <Button
          variant="primary"
          size="md"
          onClick={closeDialog}
          style={{
            background: 'linear-gradient(90deg, #8B20FF, #F02DCD)',
            boxShadow: '0 4px 16px rgba(240,45,205,0.40)',
            paddingLeft: 34,
            paddingRight: 34,
          }}
          className="hover:brightness-110"
        >
          OK
        </Button>
      </div>
    </div>
  )
}
