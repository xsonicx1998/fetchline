import { useEffect, useState } from 'react'
import {
  Ban,
  ChevronDown,
  Clapperboard,
  Clock,
  Download,
  FolderDown,
  Globe,
  Hand,
  ListPlus,
  Play,
  Plus,
  Settings as SettingsIcon,
  Square,
  Trash2,
  type LucideIcon,
} from 'lucide-react'
import type { DownloadRow, QueueRow } from '../lib/types'

/** The actions the toolbar offers. Every entry wires to an existing
 *  command or dialog — the toolbar rearranges the app, it adds
 *  nothing. */
export interface ToolbarActions {
  add: () => void
  resume: () => void
  stop: () => void
  stopAll: () => void
  deleteSelected: () => void
  deleteCompleted: () => void
  settings: () => void
  scheduler: () => void
  startQueue: () => void
  stopQueue: () => void
  videoGrabber: () => void
  siteGrabber: () => void
  downloadAll: () => void
  batch: () => void
}

interface ToolbarProps {
  actions: ToolbarActions
  /** The download Resume / Stop / Delete act on, if one is selected. */
  selected: DownloadRow | null
  /** The queue Start/Stop act on: the selected queue, else the
   *  main queue. Null only when the app has no queue at all. */
  queue: QueueRow | null
  hasDownloading: boolean
  hasComplete: boolean
}

function ToolbarButton({
  icon: Icon,
  label,
  color,
  disabled,
  onClick,
  ariaLabel,
}: {
  icon: LucideIcon
  label: string
  color: string
  disabled?: boolean
  onClick: () => void
  ariaLabel?: string
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      aria-label={ariaLabel ?? label}
      title={ariaLabel ?? label}
      className="flex w-[76px] shrink-0 flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 hover:bg-surface-2 disabled:opacity-45"
    >
      <Icon className={`h-7 w-7 ${color}`} aria-hidden="true" />
      <span className="max-w-full truncate whitespace-nowrap text-[11px] font-medium leading-tight text-text">
        {label}
      </span>
    </button>
  )
}

function ToolbarSeparator() {
  return <span className="mx-1 w-px shrink-0 self-stretch bg-border" aria-hidden="true" />
}

function GrabberItem({
  icon: Icon,
  onClick,
  children,
}: {
  icon: LucideIcon
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2"
    >
      <Icon className="h-4 w-4 shrink-0 text-info" aria-hidden="true" />
      {children}
    </button>
  )
}

/** S01 — the main toolbar, IDM's order and shape: big icons with
 *  the label underneath, one line each (the old "Add a link" label
 *  wrapped to two lines — the fixed width + `truncate` ends that).
 *  Icons are Lucide, drawn in our own style, coloured in the Sonic
 *  palette the way the user's recolored reference shows them:
 *  blue for go/add actions, red for stop/delete, grey for plain
 *  delete. No IDM artwork is copied. Disabled states match the
 *  menu bar exactly. */
export function Toolbar({ actions, selected, queue, hasDownloading, hasComplete }: ToolbarProps) {
  const [grabberOpen, setGrabberOpen] = useState(false)

  useEffect(() => {
    if (!grabberOpen) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setGrabberOpen(false)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [grabberOpen])

  const canResume = selected !== null && (selected.status === 'paused' || selected.status === 'error')
  const canStop = selected !== null && selected.status === 'downloading'
  const canStartQueue = queue !== null && !queue.running && queue.file_count > 0
  const canStopQueue = queue !== null && queue.running

  function grabberItem(action: () => void) {
    return () => {
      setGrabberOpen(false)
      action()
    }
  }

  return (
    <div
      role="toolbar"
      aria-label="Download actions"
      className="flex h-[68px] shrink-0 items-stretch gap-0.5 overflow-x-auto border-b border-border bg-surface px-2 py-1"
    >
      <ToolbarButton icon={Plus} label="Add URL" color="text-info" onClick={actions.add} />
      <ToolbarSeparator />
      <ToolbarButton
        icon={Download}
        label="Resume"
        color="text-info"
        disabled={!canResume}
        onClick={actions.resume}
      />
      <ToolbarButton
        icon={Ban}
        label="Stop"
        color="text-danger"
        disabled={!canStop}
        onClick={actions.stop}
      />
      <ToolbarButton
        icon={Hand}
        label="Stop All"
        color="text-danger"
        disabled={!hasDownloading}
        onClick={actions.stopAll}
      />
      <ToolbarSeparator />
      <ToolbarButton
        icon={Trash2}
        label="Delete"
        color="text-text-muted"
        disabled={!selected}
        onClick={actions.deleteSelected}
      />
      <ToolbarButton
        icon={Trash2}
        label="Delete Completed"
        color="text-danger"
        disabled={!hasComplete}
        onClick={actions.deleteCompleted}
        ariaLabel="Delete Completed"
      />
      <ToolbarSeparator />
      <ToolbarButton
        icon={SettingsIcon}
        label="Options"
        color="text-info"
        onClick={actions.settings}
      />
      <ToolbarButton
        icon={Clock}
        label="Scheduler"
        color="text-accent"
        onClick={actions.scheduler}
      />
      <ToolbarSeparator />
      <ToolbarButton
        icon={Play}
        label="Start Queue"
        color="text-info"
        disabled={!canStartQueue}
        onClick={actions.startQueue}
      />
      <ToolbarButton
        icon={Square}
        label="Stop Queue"
        color="text-danger"
        disabled={!canStopQueue}
        onClick={actions.stopQueue}
      />
      <ToolbarSeparator />
      <div className="relative shrink-0">
        <button
          type="button"
          aria-haspopup="menu"
          aria-expanded={grabberOpen}
          onClick={() => setGrabberOpen((v) => !v)}
          className="flex h-full w-[76px] flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 hover:bg-surface-2"
        >
          <Globe className="h-7 w-7 text-info" aria-hidden="true" />
          <span className="flex items-center gap-0.5 whitespace-nowrap text-[11px] font-medium leading-tight text-text">
            Grabber
            <ChevronDown className="h-3 w-3 text-text-muted" aria-hidden="true" />
          </span>
        </button>
        {grabberOpen ? (
          <>
            <button
              type="button"
              aria-label="Close grabber menu"
              tabIndex={-1}
              onClick={() => setGrabberOpen(false)}
              className="fixed inset-0 z-10 cursor-default bg-transparent"
            />
            <div
              role="menu"
              aria-label="Grabber"
              className="absolute left-0 z-20 mt-1 min-w-60 rounded-md border border-border bg-surface-raised p-1 shadow-pop"
            >
              <GrabberItem icon={Clapperboard} onClick={grabberItem(actions.videoGrabber)}>
                Video grabber…
              </GrabberItem>
              <GrabberItem icon={FolderDown} onClick={grabberItem(actions.siteGrabber)}>
                Site grabber…
              </GrabberItem>
              <GrabberItem icon={Globe} onClick={grabberItem(actions.downloadAll)}>
                Download all links…
              </GrabberItem>
              <GrabberItem icon={ListPlus} onClick={grabberItem(actions.batch)}>
                Batch…
              </GrabberItem>
            </div>
          </>
        ) : null}
      </div>
    </div>
  )
}
