// Round 88.9 (8) — a bulk Move to queue skips finished
// downloads instead of failing the whole move.
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { formatQueueMoveResult, partitionQueueMove } from './queues'
import type { DownloadRow, QueueRow } from './types'

function row(over: { id: string; status: string }): DownloadRow {
  return { id: over.id, status: over.status } as DownloadRow
}

function queue(over: { schedule_type: string }): QueueRow {
  return { id: 'queue-test', schedule_type: over.schedule_type } as QueueRow
}

describe('partitionQueueMove', () => {
  test('a one-time queue skips finished rows, moves the rest', () => {
    const { movable, skipped } = partitionQueueMove(
      [row({ id: 'd1', status: 'downloading' }), row({ id: 'd2', status: 'complete' }), row({ id: 'd3', status: 'paused' })],
      queue({ schedule_type: 'one_time' }),
    )
    expect(movable.map((r) => r.id)).toEqual(['d1', 'd3'])
    expect(skipped.map((r) => r.id)).toEqual(['d2'])
  })

  test('a periodic queue takes finished rows (they are what it watches)', () => {
    const { movable, skipped } = partitionQueueMove(
      [row({ id: 'd1', status: 'complete' }), row({ id: 'd2', status: 'complete' })],
      queue({ schedule_type: 'periodic' }),
    )
    expect(movable.map((r) => r.id)).toEqual(['d1', 'd2'])
    expect(skipped).toEqual([])
  })

  test('an unknown queue skips finished rows too', () => {
    const { movable, skipped } = partitionQueueMove(
      [row({ id: 'd1', status: 'complete' })],
      undefined,
    )
    expect(movable).toEqual([])
    expect(skipped.map((r) => r.id)).toEqual(['d1'])
  })
})

describe('formatQueueMoveResult', () => {
  test('nothing skipped, nothing to report', () => {
    expect(formatQueueMoveResult(3, 0, 'TestQueue')).toBeNull()
  })

  test('everything skipped', () => {
    expect(formatQueueMoveResult(0, 2, 'TestQueue')).toBe('Nothing moved — 2 already finished')
  })

  test('some moved, some skipped', () => {
    expect(formatQueueMoveResult(3, 2, 'TestQueue')).toBe(
      'Moved 3 to TestQueue, skipped 2 already finished',
    )
  })

  test('an unnamed queue reads as "queue"', () => {
    expect(formatQueueMoveResult(1, 1, null)).toBe('Moved 1 to queue, skipped 1 already finished')
  })
})
