interface ProgressRingProps {
  /** 0–100, or null when the total size is unknown. */
  percent: number | null
  status?: 'downloading' | 'complete' | 'error' | 'paused'
  /** Outer diameter in px. At 40 px and up the percent prints inside. */
  size?: number
  label?: string
}

/** Circular progress ring (Sonic Manager look, from the reference
 *  mockup's download cards). The percentage is never colour-only:
 *  at larger sizes it prints inside the ring, and everywhere the
 *  same number is also shown as text next to it. */
export function ProgressRing({ percent, status = 'downloading', size = 24, label }: ProgressRingProps) {
  const stroke = size >= 40 ? 5 : 3
  const radius = (size - stroke) / 2
  const circumference = 2 * Math.PI * radius
  const filled = percent === null ? 0 : (circumference * Math.min(100, Math.max(0, percent))) / 100
  const color =
    status === 'complete'
      ? 'var(--color-success)'
      : status === 'error'
        ? 'var(--color-danger)'
        : status === 'paused'
          ? 'var(--color-text-faint)'
          : 'var(--color-accent)'
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      role="progressbar"
      aria-valuenow={percent ?? undefined}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={label ?? 'Download progress'}
      className="shrink-0"
    >
      <circle cx={size / 2} cy={size / 2} r={radius} fill="none" stroke="var(--color-surface-2)" strokeWidth={stroke} />
      {percent === null ? (
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={`${circumference * 0.25} ${circumference * 0.75}`}
        />
      ) : (
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={`${filled} ${circumference}`}
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
        />
      )}
      {size >= 40 && percent !== null ? (
        <text
          x="50%"
          y="50%"
          dominantBaseline="central"
          textAnchor="middle"
          fill="var(--color-text)"
          fontSize={size >= 56 ? 13 : 11}
          fontWeight={600}
        >
          {percent}%
        </text>
      ) : null}
    </svg>
  )
}
