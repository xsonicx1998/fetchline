import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ProgressWindow } from './components/ProgressWindow'
import './index.css'
import './background.css'

/** A progress window loads the same bundle with
 *  `#progress=<download id>` in the URL (set by the
 *  `open_progress_window` Tauri command) and renders the progress
 *  view instead of the main window (S03). Download ids are UUIDs,
 *  so only hex + hyphens are accepted here — anything else falls
 *  back to the main window rather than opening a window for a
 *  made-up id. */
function progressDownloadId(): string | null {
  const match = /^#progress=([0-9a-fA-F-]+)$/.exec(window.location.hash)
  return match ? match[1] : null
}

const progressId = progressDownloadId()

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
