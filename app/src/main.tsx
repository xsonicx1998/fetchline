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

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    {progressId !== null ? <ProgressWindow downloadId={progressId} /> : <App />}
  </React.StrictMode>,
)
