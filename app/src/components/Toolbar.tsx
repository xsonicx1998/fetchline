import { useEffect, useState } from 'react'
import { ChevronDown, Clapperboard, FolderDown, Globe, ListPlus } from 'lucide-react'
import type { DownloadRow, QueueRow } from '../lib/types'
import { PackIcon, type PackIconName } from './PackIcon'

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
  icon,
  label,
  color,
  glow,
  disabled,
  onClick,
  ariaLabel,
}: {
  icon: PackIconName
  label: string
  color: string
  /** Neon glow matching the icon colour (Round 47, strengthened
   *  Round 48) — the reference's icons glow constantly, brighter
   *  on hover. Disabled buttons stay at 75% opacity (was 45%):
   *  at 45% the icons read as grey and washed-out next to the
   *  glowing Add URL button and Options gear, which is what the
   *  user flagged; at 75% they stay visibly dimmer than enabled
   *  buttons while still reading as bright and neon. */
  glow: 'cyan' | 'pink' | 'white' | 'purple'
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
      className="group flex min-w-fit flex-auto flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 hover:bg-surface-2 disabled:opacity-75"
    >
      <PackIcon name={icon} className={`h-8 w-8 ${color} icon-glow-${glow}`} />
      <span className="max-w-full whitespace-nowrap text-xs font-medium leading-tight text-text">
        {label}
      </span>
    </button>
  )
}

function ToolbarSeparator() {
  return <span className="mx-1 w-px shrink-0 self-stretch bg-border" aria-hidden="true" />
}

function GrabberItem({
  onClick,
  children,
}: {
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2"
    >
      {children}
    </button>
  )
}

/** S01 — the main toolbar, the user's asset pack (Round 45): the
 *  pack's SVG icons in the reference's colours — cyan for go/add,
 *  pink for stop/delete, white for plain delete. Add URL is the big
 *  glowing purple button; Options is the big cyan gear in a purple
 *  halo. Buttons size to their content and grow to fill the row
 *  (flex-auto, min-width fit-content), so all 11 fit at 1024 px
 *  with no scrollbar and "Delete Completed" stays on one line
 *  (Round 46: fixed 96 px columns overflowed at 1024 px, cutting
 *  off Stop Queue and Grabber behind a scrollbar). The container
 *  is overflow-hidden, so a scrollbar can never appear. Round 47:
 *  icons 28 → 32 px, labels 11 → 12 px, icons glow constantly
 *  (drop-shadow per colour, brighter on hover) and the Options
 *  halo is a stronger cyan + purple multi-layer glow. The Grabber
 *  dropdown already holds Video grabber, Site grabber, Download
 *  all links and Batch — the menu bar's Downloads actions live
 *  there now that the menu bar is gone. Disabled states match the
 *  old menu bar exactly. */
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
      className="flex h-[92px] shrink-0 items-stretch gap-0.5 overflow-hidden border-b border-border bg-surface px-2 py-1 shadow-[0_1px_12px_rgba(139,32,255,0.12)]"
    >
      <button
        type="button"
        onClick={actions.add}
        aria-label="Add URL"
        title="Add URL"
        className="mr-1 flex h-12 shrink-0 items-center gap-2 self-center rounded-xl bg-accent px-5 text-[15px] font-semibold text-on-accent shadow-glow hover:bg-accent-hover"
      >
        <PackIcon name="add-url" className="h-6 w-6" />
        Add URL
      </button>
      <ToolbarSeparator />
      <ToolbarButton
        icon="resume"
        label="Resume"
        color="text-info"
        glow="cyan"
        disabled={!canResume}
        onClick={actions.resume}
      />
      <ToolbarButton
        icon="stop"
        label="Stop"
        color="text-danger"
        glow="pink"
        disabled={!canStop}
        onClick={actions.stop}
      />
      <ToolbarButton
        icon="stop-all"
        label="Stop All"
        color="text-danger"
        glow="pink"
        disabled={!hasDownloading}
        onClick={actions.stopAll}
      />
      <ToolbarSeparator />
      <ToolbarButton
        icon="delete"
        label="Delete"
        color="text-text"
        glow="white"
        disabled={!selected}
        onClick={actions.deleteSelected}
      />
      <ToolbarButton
        icon="delete-completed"
        label="Delete Completed"
        color="text-danger"
        glow="pink"
        disabled={!hasComplete}
        onClick={actions.deleteCompleted}
        ariaLabel="Delete Completed"
      />
      <ToolbarSeparator />
      <button
        type="button"
        onClick={actions.settings}
        aria-label="Options"
        title="Options"
        className="group flex min-w-fit flex-auto flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 hover:bg-surface-2"
      >
        <span
          className="flex h-14 w-14 items-center justify-center rounded-full"
          style={{
            background: 'radial-gradient(circle, rgba(139,32,255,.70), rgba(0,200,255,.25) 55%, rgba(139,32,255,.08))',
            boxShadow:
              '0 0 18px rgba(0,200,255,.60), 0 0 36px rgba(139,32,255,.55), 0 0 60px rgba(139,32,255,.30)',
          }}
        >
          <PackIcon name="options" className="h-9 w-9 text-info icon-glow-cyan" />
        </span>
        <span className="whitespace-nowrap text-xs font-medium leading-tight text-text">Options</span>
      </button>
      <ToolbarButton
        icon="scheduler"
        label="Scheduler"
        color="text-danger"
        glow="pink"
        onClick={actions.scheduler}
      />
      <ToolbarSeparator />
      <ToolbarButton
        icon="start-queue"
        label="Start Queue"
        color="text-info"
        glow="cyan"
        disabled={!canStartQueue}
        onClick={actions.startQueue}
      />
      <ToolbarButton
        icon="stop-queue"
        label="Stop Queue"
        color="text-danger"
        glow="pink"
        disabled={!canStopQueue}
        onClick={actions.stopQueue}
      />
      <ToolbarSeparator />
      <div className="relative min-w-fit flex-auto">
        <button
          type="button"
          aria-haspopup="menu"
          aria-expanded={grabberOpen}
          onClick={() => setGrabberOpen((v) => !v)}
          className="group flex h-full w-full flex-col items-center justify-center gap-1 rounded-md px-1 py-1.5 hover:bg-surface-2"
        >
          <PackIcon name="grabber" className="h-8 w-8 text-info icon-glow-cyan" />
          <span className="flex items-center gap-0.5 whitespace-nowrap text-xs font-medium leading-tight text-text">
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
              <GrabberItem onClick={grabberItem(actions.videoGrabber)}>
                <Clapperboard className="h-4 w-4 shrink-0 text-info" aria-hidden="true" />
                Video grabber…
              </GrabberItem>
              <GrabberItem onClick={grabberItem(actions.siteGrabber)}>
                <FolderDown className="h-4 w-4 shrink-0 text-info" aria-hidden="true" />
                Site grabber…
              </GrabberItem>
              <GrabberItem onClick={grabberItem(actions.downloadAll)}>
                <Globe className="h-4 w-4 shrink-0 text-info" aria-hidden="true" />
                Download all links…
              </GrabberItem>
              <GrabberItem onClick={grabberItem(actions.batch)}>
                <ListPlus className="h-4 w-4 shrink-0 text-info" aria-hidden="true" />
                Batch…
              </GrabberItem>
            </div>
          </>
        ) : null}
      </div>
    </div>
  )
}
