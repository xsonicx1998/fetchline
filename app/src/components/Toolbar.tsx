import { Pause, Play, Plus, Search, Square, Trash2 } from 'lucide-react'
import type { DownloadRow } from '../lib/types'

/** The actions the toolbar offers. "Stop" pauses the selected
 *  download — IDM calls pause "Stop", and this app has no
 *  separate stop command (cancel deletes the row, so it is not
 *  a Stop). Everything else that used to live on the big toolbar
 *  (Stop All, Delete Completed, Options, Scheduler, Start/Stop
 *  Queue, Grabber) moved to the menu bar and the TOOLS strip. */
export interface ToolbarActions {
  add: () => void
  resume: () => void
  stop: () => void
  deleteSelected: () => void
}

interface ToolbarProps {
  actions: ToolbarActions
  /** The download Resume / Stop / Delete act on, if one is selected. */
  selected: DownloadRow | null
  searchQuery: string
  onSearchChange: (query: string) => void
}

/** Round 62 — the slim toolbar from the approved mockup: a
 *  gradient "+ Add URL", Resume / Pause / Stop / Delete as
 *  compact buttons, and a search box on the right that filters
 *  the table by filename. The old 92 px big-icon toolbar (11
 *  buttons + Options gear) is gone: its actions live in the
 *  menu bar and the TOOLS strip now, and there is no settings
 *  gear anywhere — Settings is reached from the Options menu. */
export function Toolbar({ actions, selected, searchQuery, onSearchChange }: ToolbarProps) {
  // Round 69 (b) — a `queued` ("Waiting") row is resumable too:
  // the engine's `resume_download` accepts any non-complete,
  // non-downloading row, and a queued row is exactly a download
  // that was added but never started (clipboard offer, Download
  // Later, Redownload). Gating Resume to paused/error left those
  // rows with no start path at all in the UI — the Round 68
  // retest's stuck "Waiting" row, whose toolbar Resume "did
  // nothing" because the button was disabled.
  const canResume =
    selected !== null &&
    (selected.status === 'paused' || selected.status === 'error' || selected.status === 'queued')
  const canStop = selected !== null && selected.status === 'downloading'

  return (
    <div
      role="toolbar"
      aria-label="Download actions"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-surface px-3 py-2"
    >
      <button
        type="button"
        onClick={actions.add}
        aria-label="Add URL"
        title="Add URL"
        className="flex shrink-0 items-center gap-1.5 rounded-lg bg-[linear-gradient(90deg,#8b20ff,#f02dcd)] px-3.5 py-2 text-sm font-semibold text-on-accent shadow-glow hover:brightness-110"
      >
        <Plus className="h-4 w-4" aria-hidden="true" />
        Add URL
      </button>
      <button
        type="button"
        onClick={actions.resume}
        disabled={!canResume}
        aria-label="Resume"
        title="Resume"
        className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-sm text-text hover:bg-surface-2 disabled:opacity-45"
      >
        <Play className="h-4 w-4" aria-hidden="true" />
        Resume
      </button>
      <button
        type="button"
        onClick={actions.stop}
        disabled={!canStop}
        aria-label="Pause"
        title="Pause"
        className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-sm text-text hover:bg-surface-2 disabled:opacity-45"
      >
        <Pause className="h-4 w-4" aria-hidden="true" />
        Pause
      </button>
      <button
        type="button"
        onClick={actions.stop}
        disabled={!canStop}
        aria-label="Stop"
        title="Stop"
        className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-sm text-text hover:bg-surface-2 disabled:opacity-45"
      >
        <Square className="h-4 w-4" aria-hidden="true" />
        Stop
      </button>
      <button
        type="button"
        onClick={actions.deleteSelected}
        disabled={!selected}
        aria-label="Delete"
        title="Delete"
        className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-2 text-sm text-text hover:bg-surface-2 disabled:opacity-45"
      >
        <Trash2 className="h-4 w-4" aria-hidden="true" />
        Delete
      </button>
      <label className="ml-auto flex min-w-0 max-w-64 flex-1 items-center gap-2 rounded-lg border border-border bg-bg px-3 py-2">
        <Search className="h-4 w-4 shrink-0 text-text-muted" aria-hidden="true" />
        <input
          type="search"
          value={searchQuery}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder="Search downloads"
          aria-label="Search downloads"
          className="min-w-0 flex-1 bg-transparent text-sm text-text outline-none placeholder:text-text-faint"
        />
      </label>
    </div>
  )
}
