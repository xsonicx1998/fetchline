interface ProgressBarProps {
  /** 0–100, or null when the total size is unknown. */
  percent: number | null
  status?: 'downloading' | 'complete' | 'error' | 'paused'
  /** Thin inline bar (table) or the taller dialog bar. */
  size?: 'thin' | 'tall'
  /** Round 62: the Sonic gradient fill (purple → pink) instead
   *  of the solid accent — the progress window's big bar and the
   *  table's inline bars. */
  gradient?: boolean
  label?: string
}

/** Progress bar primitive (components.md). Never colour-only: the
 *  percentage is always printed as text next to it. */
export function ProgressBar({ percent, status = 'downloading', size = 'thin', gradient = false, label }: ProgressBarProps) {
  const fill =
    status === 'complete'
      ? 'bg-success'
      : status === 'error'
        ? 'bg-danger'
        : status === 'paused'
          ? 'bg-text-faint'
          : gradient
            ? 'bg-[linear-gradient(90deg,#8b20ff,#f02dcd)]'
            : 'bg-accent'
  return (
    <div className="flex items-center gap-2">
      <div
        role="progressbar"
        aria-valuenow={percent ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={label ?? 'Download progress'}
        className={`w-full overflow-hidden rounded-pill bg-surface-2 ${size === 'tall' ? 'h-2' : 'h-1'}`}
      >
        {percent === null ? (
          <div className={`h-full w-1/3 animate-pulse ${fill}`} />
        ) : (
          <div className={`h-full ${fill}`} style={{ width: `${percent}%` }} />
        )}
      </div>
      <span className="w-10 shrink-0 text-right font-mono text-xs text-text-muted">
        {percent === null ? '—' : `${percent}%`}
      </span>
    </div>
  )
}
