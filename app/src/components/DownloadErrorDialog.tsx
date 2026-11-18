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
 *  vanished from the list shows the empty state instead.
 *
 *  Round 69 (d) — the message variant: an ADD-time failure (the
 *  probe said 404, the server could not be reached) creates no
 *  download row, so the Round 68 trigger — a row flipping to
 *  `error` — could never fire for it, and the failure existed
 *  only as the Add dialog's inline pink text. With `message`
 *  given, the dialog shows that text directly (no polling), with
 *  `title` in the title bar and `onClose` dismissing it — the
 *  Add dialog renders it as an overlay in the main window, so
 *  OK/X must dismiss the overlay, never close the main window. */
export function DownloadErrorDialog({
  downloadId,
  message,
  title,
  onClose,
}: {
  /** The failed download's row id — the dialog polls
   *  `listDownloads` for it. Absent when `message` is given. */
  downloadId?: string
  /** An add-time error: there is no download row, so the Details
   *  text is passed directly and no polling happens. */
  message?: string
  /** Title-bar title for the message variant (the file name the
   *  user typed, or "Download failed"). */
  title?: string
  /** Dismiss handler for the message variant (overlay in the
   *  main window). Absent, OK/X close this OS window. */
  onClose?: () => void
}) {
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
    // Round 69 (d) — the message variant has no row to poll for.
    if (message !== undefined || !downloadId) return
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
  }, [downloadId, message])

  function closeDialog() {
    if (onClose) {
      onClose()
      return
    }
    void getCurrentWindow().close()
  }

  // Round 69 (d) — the message variant shows its text directly;
  // the row variant shows the row's `error_message`.
  const isMessage = message !== undefined
  const detailsText =
    message ?? download?.error_message ?? 'The download failed for an unknown reason.'
  const barTitle = title ?? download?.filename ?? 'Download failed'

  if (!isMessage && (!loaded || !download)) {
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
      <TitleBar title={barTitle} showMinimize={false} showMaximize={false} onClose={onClose} />
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
            <span className="text-danger">{detailsText}</span>
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
