// Round 88.9 — scheduler: a schedule never needs a hand-made
// queue (1), the queue name is readable on hover (4), and the
// "New queue" dialog opens above the scheduler (3).
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { AddQueueDialog } from './AddQueueDialog'
import { SchedulerDialog } from './SchedulerDialog'
import * as api from '../lib/api'
import type { QueueRow } from '../lib/types'

vi.mock('../lib/api', () => ({
  createQueue: vi.fn(),
  getQueueFiles: vi.fn(),
  removeFromQueue: vi.fn(),
  reorderQueue: vi.fn(),
  startQueue: vi.fn(),
  stopQueue: vi.fn(),
  syncQueueNow: vi.fn(),
  updateQueue: vi.fn(),
}))

function makeQueue(over: { id: string; name: string }): QueueRow {
  return {
    id: over.id,
    name: over.name,
    type: 'custom',
    is_main: false,
    schedule_type: 'none',
    start_at: null,
    stop_at: null,
    start_on_startup: false,
    on_complete_action: 'none',
    on_complete_program: null,
    sync_interval_minutes: null,
    last_sync_at: null,
    last_sync_changed: 0,
    last_sync_unchanged: 0,
    last_sync_removed: 0,
    last_sync_failed: 0,
    last_sync_skipped: 0,
    running: false,
    file_count: 0,
  }
}

function renderScheduler(queues: QueueRow[], onChanged = vi.fn()) {
  return {
    onChanged,
    ...render(
      <SchedulerDialog
        open
        queues={queues}
        initialQueueId={null}
        onClose={() => {}}
        onChanged={onChanged}
        onNewQueue={() => {}}
      />,
    ),
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.getQueueFiles).mockResolvedValue([])
})

describe('scheduler implicit queue (Round 88.9, item 1)', () => {
  test('opening with no queues creates "Schedule 1" in the background', async () => {
    const created = makeQueue({ id: 'queue-sched-1', name: 'Schedule 1' })
    let resolveCreate!: (q: QueueRow) => void
    vi.mocked(api.createQueue).mockImplementation(
      () =>
        new Promise<QueueRow>((resolve) => {
          resolveCreate = resolve
        }),
    )
    vi.mocked(api.getQueueFiles).mockResolvedValue([])
    const { onChanged, rerender } = renderScheduler([])
    // While the creation is in flight the dialog says so —
    // the user never sees the dead-end "No queues yet".
    expect(screen.getByText('Creating a schedule…')).toBeTruthy()
    expect(vi.mocked(api.createQueue)).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.createQueue)).toHaveBeenCalledWith('Schedule 1')

    resolveCreate(created)
    await waitFor(() => expect(onChanged).toHaveBeenCalled(), { timeout: 5000 })
    rerender(
      <SchedulerDialog
        open
        queues={[created]}
        initialQueueId={null}
        onClose={() => {}}
        onChanged={onChanged}
        onNewQueue={() => {}}
      />,
    )
    // The new queue is selected and its form is usable.
    await waitFor(() => expect(screen.getByText('Schedule 1')).toBeTruthy(), {
      timeout: 5000,
    })
    expect(vi.mocked(api.createQueue)).toHaveBeenCalledTimes(1)
  })

  test('opening with a queue never creates one', () => {
    vi.mocked(api.getQueueFiles).mockResolvedValue([])
    renderScheduler([makeQueue({ id: 'queue-test', name: 'TestQueue' })])
    expect(vi.mocked(api.createQueue)).not.toHaveBeenCalled()
    expect(screen.getByText('TestQueue')).toBeTruthy()
  })

  test('a failed creation shows the error and leaves "New queue" usable', async () => {
    vi.mocked(api.createQueue).mockRejectedValue(new Error('boom'))
    renderScheduler([])
    await waitFor(
      () => expect(screen.getByText('boom')).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(screen.getByText('New queue')).toBeTruthy()
  })
})

describe('scheduler queue names (Round 88.9, item 4)', () => {
  test('a truncated queue name carries the full name as a tooltip', () => {
    const longName = 'A very long queue name that does not fit the list'
    vi.mocked(api.getQueueFiles).mockResolvedValue([])
    renderScheduler([makeQueue({ id: 'queue-long', name: longName })])
    expect(screen.getByText(longName).getAttribute('title')).toBe(longName)
  })
})

describe('new queue dialog layering (Round 88.9, item 3)', () => {
  test('the New queue dialog sits above the scheduler (z-20)', () => {
    render(<AddQueueDialog open onClose={() => {}} onAdded={() => {}} />)
    const dialog = screen.getByRole('dialog')
    expect(dialog.className).toContain('z-20')
  })
})
