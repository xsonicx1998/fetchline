import { Clapperboard, Clipboard, Globe, ListOrdered, RefreshCw, Timer } from 'lucide-react'

interface ToolsStripProps {
  onQueue: () => void
  onScheduler: () => void
  onVideoGrabber: () => void
  onSiteGrabber: () => void
  onRefreshAddress: () => void
  /** Refresh address only works on a paused or failed download. */
  canRefreshAddress: boolean
  clipboardOn: boolean
  onToggleClipboard: () => void
}

/** Round 62 — the slim TOOLS strip under the toolbar (approved
 *  mockup): the features that used to live only in menus are
 *  visible in the main window itself — Queue, Scheduler,
 *  Video Grabber, Site Grabber, Refresh Address and Clipboard
 *  monitoring (with its ON/OFF state). History is NOT here
 *  (Round 63): it already lives in the sidebar. No settings
 *  gear here: Settings is reached from the Options menu, so a
 *  gear would be redundant. */
export function ToolsStrip({
  onQueue,
  onScheduler,
  onVideoGrabber,
  onSiteGrabber,
  onRefreshAddress,
  canRefreshAddress,
  clipboardOn,
  onToggleClipboard,
}: ToolsStripProps) {
  return (
    <div className="flex shrink-0 items-center gap-1.5 overflow-x-auto border-b border-border bg-surface px-3 py-1.5">
      <span className="mr-1 shrink-0 text-[11px] font-semibold uppercase tracking-wider text-accent">
        Tools
      </span>
      <StripButton onClick={onQueue} icon={<ListOrdered className="h-3.5 w-3.5" aria-hidden="true" />}>
        Queue
      </StripButton>
      <StripButton onClick={onScheduler} icon={<Timer className="h-3.5 w-3.5" aria-hidden="true" />}>
        Scheduler
      </StripButton>
      <StripButton
        onClick={onVideoGrabber}
        icon={<Clapperboard className="h-3.5 w-3.5" aria-hidden="true" />}
      >
        Video Grabber
      </StripButton>
      <StripButton onClick={onSiteGrabber} icon={<Globe className="h-3.5 w-3.5" aria-hidden="true" />}>
        Site Grabber
      </StripButton>
      <StripButton
        onClick={onRefreshAddress}
        disabled={!canRefreshAddress}
        title={
          canRefreshAddress
            ? 'Replace the selected download’s expired link and resume from the downloaded bytes'
            : 'Select a paused or failed download to refresh its address'
        }
        icon={<RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />}
      >
        Refresh Address
      </StripButton>
      <StripButton
        onClick={onToggleClipboard}
        title={clipboardOn ? 'Clipboard monitoring is on — click to turn it off' : 'Clipboard monitoring is off — click to turn it on'}
        icon={<Clipboard className="h-3.5 w-3.5" aria-hidden="true" />}
      >
        Clipboard
        <span className={clipboardOn ? 'text-info' : 'text-text-muted'}>
          • {clipboardOn ? 'ON' : 'OFF'}
        </span>
      </StripButton>
    </div>
  )
}

function StripButton({
  onClick,
  disabled,
  title,
  icon,
  children,
}: {
  onClick: () => void
  disabled?: boolean
  title?: string
  icon: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className="flex shrink-0 items-center gap-1.5 rounded-full border border-border px-2.5 py-1 text-xs text-text hover:bg-surface-2 disabled:opacity-45"
    >
      {icon}
      {children}
    </button>
  )
}
