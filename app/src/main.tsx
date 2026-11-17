import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ProgressWindow } from './components/ProgressWindow'
import { webviewHeartbeat } from './lib/api'
import './index.css'

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
 *  renderer — a mounted window pings, a dead one does not). */
const windowLabel = progressId !== null ? `progress-${progressId}` : 'main'
{
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
    {progressId !== null ? <ProgressWindow downloadId={progressId} /> : <App />}
  </React.StrictMode>,
)
