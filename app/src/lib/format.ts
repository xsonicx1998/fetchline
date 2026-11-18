// Fresh, human formatting helpers. No copy from the original app.

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return '—'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`
}

export function formatSpeed(bytesPerSecond: number, zeroAs = '—'): string {
  if (bytesPerSecond <= 0) return zeroAs
  return `${formatBytes(bytesPerSecond)}/s`
}

export function formatEta(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return '—'
  if (seconds < 60) return `${seconds}s left`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${seconds % 60}s left`
  const hours = Math.floor(minutes / 60)
  return `${hours}h ${minutes % 60}m left`
}

export function progressPercent(downloaded: number, size: number | null): number | null {
  if (size === null || size <= 0) return null
  return Math.min(100, Math.round((downloaded / size) * 100))
}

/** UTC ISO string (or null) → the reader's local date and time. */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** `<input type="datetime-local">` value ("2026-11-14T20:30", local
 *  time) → UTC ISO string for the engine, or null when empty. */
export function datetimeLocalToUtcIso(value: string): string | null {
  if (!value.trim()) return null
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return null
  return d.toISOString()
}

/** UTC ISO string (or null) → `<input type="datetime-local">` value
 *  in the reader's local time, or '' when unset. */
export function utcIsoToDatetimeLocal(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** The last part of a path, for showing which file just finished. */
export function basename(path: string | null | undefined): string {
  if (!path) return ''
  const parts = path.split(/[/\\]/)
  return parts[parts.length - 1] || path
}

export function completeActionLabel(action: string): string {
  switch (action) {
    case 'open_file':
      return 'Open the file when done'
    case 'run_program':
      return 'Run a program when done'
    case 'shutdown':
      return 'Shut down the computer when done'
    case 'hibernate':
      return 'Hibernate the computer when done'
    default:
      return 'Do nothing when done'
  }
}

export function statusLabel(status: string): string {
  switch (status) {
    case 'queued':
      return 'Waiting'
    case 'downloading':
      return 'Downloading'
    case 'paused':
      return 'Paused'
    case 'complete':
      return 'Finished'
    case 'error':
      return 'Failed'
    default:
      return status
  }
}
