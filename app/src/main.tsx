import React, { useLayoutEffect } from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { DownloadCompleteDialog } from './components/DownloadCompleteDialog'
import { DownloadErrorDialog } from './components/DownloadErrorDialog'
import { ProgressWindow } from './components/ProgressWindow'
import { StandaloneAddUrlWindow } from './components/StandaloneAddUrlWindow'
import { webviewHeartbeat, windowReady } from './lib/api'
import './index.css'

/** Round 74 (8) — the WebView's default right-click menu
 *  (Refresh / Save as / Print) is suppressed in EVERY window:
 *  this bundle loads in all of them, so one document-level
 *  listener covers the main window, the progress windows and
 *  the dialog windows. Our own menu is unaffected — it is a
 *  React `onContextMenu` handler on the download rows
 *  (DownloadTable's RowMenu), which runs before this listener
 *  and draws its own DOM menu; it never needs the native one.
 *  Round 83 — the blanket suppression also killed Paste in
 *  text fields (the Add URL field had no right-click menu at
 *  all, R82 retest): when the click lands inside an input,
 *  textarea or contenteditable element the native menu is
 *  left alone, so WebView2's own menu offers Cut / Copy /
 *  Paste there. Everywhere else the menu stays suppressed. */
document.addEventListener('contextmenu', (e) => {
  const target = e.target as HTMLElement | null
  if (target?.closest('input, textarea, [contenteditable]')) return
  e.preventDefault()
})

/** Round 71 (C3) — show-when-ready: every window is built
 *  hidden (`visible: false`) so it never flashes a blank first
 *  paint. This fires once, in `useLayoutEffect` after the root
 *  component's first commit, and Rust shows the window WITH
 *  content. A failure is swallowed — the 3 s Rust fallback in
 *  `spawn_show_fallback` shows the window anyway, so a missed
 *  signal can never hide a window forever. One component serves
 *  all five window kinds (main / progress / complete / error /
 *  add-url): it renders nothing and sits next to the real root. */
function WindowReady() {
  useLayoutEffect(() => {
    void windowReady().catch(() => undefined)
  }, [])
  return null
}

/** A progress window loads the same bundle and renders the
 *  progress view instead of the main window (S03). The download
 *  id comes from the initialization script (`window.
 *  __PROGRESS_ID__`, set by the `open_progress_window` Tauri
 *  command before any document script runs — Round 57 §9.3, the
 *  A/B test for the blank progress window) or, as a fallback,
 *  from `#progress=<download id>` in the URL. Download ids are
 *  UUIDs, so only hex + hyphens are accepted here — anything
 *  else falls back to the main window rather than opening a
 *  window for a made-up id. */
function progressDownloadId(): string | null {
  const fromInit = (window as unknown as { __PROGRESS_ID__?: unknown }).__PROGRESS_ID__
  if (typeof fromInit === 'string' && /^[0-9a-fA-F-]+$/.test(fromInit)) return fromInit
  const match = /^#progress=([0-9a-fA-F-]+)$/.exec(window.location.hash)
  return match ? match[1] : null
}

const progressId = progressDownloadId()

/** Round 66 — the "Download complete" dialog window loads the
 *  same bundle and renders the dialog view instead of the main
 *  window. The download id comes from the initialization script
 *  (`window.__COMPLETE_ID__`, set by the `open_complete_dialog`
 *  Tauri command) or, as a fallback, from `#complete=<download
 *  id>` in the URL — the same Round 57 mechanism as the
 *  progress window. */
function completeDownloadId(): string | null {
  const fromInit = (window as unknown as { __COMPLETE_ID__?: unknown }).__COMPLETE_ID__
  if (typeof fromInit === 'string' && /^[0-9a-fA-F-]+$/.test(fromInit)) return fromInit
  const match = /^#complete=([0-9a-fA-F-]+)$/.exec(window.location.hash)
  return match ? match[1] : null
}

const completeId = completeDownloadId()

/** Round 68 — the "Download failed" dialog window loads the
 *  same bundle and renders the dialog view instead of the main
 *  window. The download id comes from the initialization script
 *  (`window.__ERROR_ID__`, set by the `open_error_dialog` Tauri
 *  command) or, as a fallback, from `#error=<download id>` in
 *  the URL — the same Round 57 mechanism as the progress window
 *  and the complete dialog. */
function errorDownloadId(): string | null {
  const fromInit = (window as unknown as { __ERROR_ID__?: unknown }).__ERROR_ID__
  if (typeof fromInit === 'string' && /^[0-9a-fA-F-]+$/.test(fromInit)) return fromInit
  const match = /^#error=([0-9a-fA-F-]+)$/.exec(window.location.hash)
  return match ? match[1] : null
}

const errorId = errorDownloadId()

/** Round 74 (13) — the standalone Add URL window for a browser
 *  capture: label `add-url-<pending-link-id>`, the link id from
 *  the initialization script (`window.__ADD_URL_ID__`, set by
 *  the `open_add_url_window` Tauri command) or, as a fallback,
 *  from `#add-url=<link id>` in the URL — the same Round 57
 *  mechanism as the progress / complete / error windows, with
 *  the same UUID validation. */
function addUrlLinkId(): string | null {
  const fromInit = (window as unknown as { __ADD_URL_ID__?: unknown }).__ADD_URL_ID__
  if (typeof fromInit === 'string' && /^[0-9a-fA-F-]+$/.test(fromInit)) return fromInit
  const match = /^#add-url=([0-9a-fA-F-]+)$/.exec(window.location.hash)
  return match ? match[1] : null
}

const addUrlId = addUrlLinkId()

/** Webview heartbeat (§7.4, Round 56): this window's JS pings
 *  the backend every 5 s with its label (`main`,
 *  `progress-<download id>`); Rust logs "heartbeat lost" when
 *  the pings stop for 30 s — the renderer may be dead even though
 *  the window still exists (the r55 live test: both windows blank
 *  white, no console data, nothing in the log). The ping is
 *  guarded so a hung backend never piles up invokes: a new ping
 *  is skipped while the previous one is still in flight. A
 *  failed ping is swallowed — the heartbeat is a diagnostic, not
 *  a user-facing error (the poll guard in App/ProgressWindow is
 *  what tells the user the backend is not responding). Module
 *  level, not a React effect: it runs once per window, and it
 *  keeps running even if React fails to mount (which is exactly
 *  the case the heartbeat must distinguish from a dead
 *  renderer — a mounted window pings, a dead one does not).
 *  Skipped for the "Download complete" dialog window (Round
 *  66) and the "Download failed" dialog window (Round 68):
 *  both are transient, register no heartbeat entry, and the
 *  monitor must never auto-reload either as a progress window.
 *  Skipped for the standalone Add URL window too (Round 74):
 *  same reasons — transient, and never a progress window. */
const windowLabel = progressId !== null ? `progress-${progressId}` : 'main'
if (completeId === null && errorId === null && addUrlId === null) {
  let pingInFlight = false
  window.setInterval(() => {
    if (pingInFlight) return
    pingInFlight = true
    // Round 57 (§9.1d): race the ping against a 5 s timeout so
    // `pingInFlight` always clears — one hung ping used to
    // silence all later pings (the `.finally()` never ran for a
    // hung invoke), so the monitor read "lost" forever and a
    // recovered window never logged "resumed". The timeout is
    // cleared when the ping settles first.
    let timer = 0
    const timeout = new Promise<never>((_resolve, reject) => {
      timer = window.setTimeout(() => reject(new Error('heartbeat ping timed out')), 5000)
    })
    void Promise.race([webviewHeartbeat(windowLabel), timeout])
      .catch(() => undefined)
      .finally(() => {
        window.clearTimeout(timer)
        pingInFlight = false
      })
  }, 5000)
}

/** Round 55 (defect 2, diagnostics): if React never mounts — a JS
 *  error before render, a bundle that never loads — the static
 *  shell in index.html stays on screen, and this plain-DOM fallback
 *  replaces it with an error after 2 s, so the window says what
 *  happened instead of sitting blank (or stuck on "Loading…").
 *  React clears the shell on mount, so a mounted window never sees
 *  this. The download itself is not affected: the window is only a
 *  view of it. */
window.setTimeout(() => {
  const shell = document.getElementById('boot-shell')
  if (shell) {
    shell.replaceChildren()
    const title = document.createElement('div')
    title.textContent = 'Sonic Manager'
    title.setAttribute('style', 'font-size:18px;font-weight:600;color:#e6e9f5;')
    const msg = document.createElement('div')
    msg.textContent =
      'This window did not start. The download itself is not affected — close this window and reopen it from the download list. If it keeps happening, the startup log holds the details.'
    msg.setAttribute(
      'style',
      'font-size:13px;line-height:1.5;max-width:420px;text-align:center;',
    )
    shell.append(title, msg)
  }
}, 2000)

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <WindowReady />
    {progressId !== null ? (
      <ProgressWindow downloadId={progressId} />
    ) : completeId !== null ? (
      <DownloadCompleteDialog downloadId={completeId} />
    ) : errorId !== null ? (
      <DownloadErrorDialog downloadId={errorId} />
    ) : addUrlId !== null ? (
      <StandaloneAddUrlWindow linkId={addUrlId} />
    ) : (
      <App />
    )}
  </React.StrictMode>,
)
