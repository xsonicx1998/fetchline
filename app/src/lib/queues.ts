import type { DownloadRow, QueueRow } from './types'

/** Round 88.9 (8) — a bulk Move to queue skips finished
 *  downloads instead of failing the whole move: the engine
 *  refuses a finished row in a one-time queue ("already
 *  finished"), and a bulk move that aborts on the first
 *  finished row is the bug the user reported (he had to move
 *  files one by one). Periodic synchronisation queues are the
 *  exception — their whole point is watching finished files,
 *  so finished rows belong in them. */
export function partitionQueueMove(
  rows: DownloadRow[],
  queue: QueueRow | undefined,
): { movable: DownloadRow[]; skipped: DownloadRow[] } {
  const movable: DownloadRow[] = []
  const skipped: DownloadRow[] = []
  for (const row of rows) {
    if (row.status === 'complete' && queue?.schedule_type !== 'periodic') {
      skipped.push(row)
    } else {
      movable.push(row)
    }
  }
  return { movable, skipped }
}

/** Round 88.9 (8) — the status line for a bulk move that
 *  skipped finished rows. Nothing to report when nothing was
 *  skipped (the rows visibly move). */
export function formatQueueMoveResult(
  moved: number,
  skipped: number,
  queueName: string | null,
): string | null {
  if (skipped === 0) return null
  if (moved === 0) return `Nothing moved — ${skipped} already finished`
  return `Moved ${moved} to ${queueName ?? 'queue'}, skipped ${skipped} already finished`
}
