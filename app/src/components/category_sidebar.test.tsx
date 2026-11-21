// Round 87 (13c) — the "Main download queue" is deleted from
// the sidebar: only the user's own queues show (plus "New
// queue"). The engine still seeds it (replica/schema.sql) as
// the internal default bucket, but no UI list offers it.
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { render, screen } from '@testing-library/react'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { CategorySidebar } from './CategorySidebar'

const noop = () => {}

const QUEUES: QueueRow[] = [
  {
    id: 'queue-main', name: 'Main download queue', type: 'main', is_main: true,
    schedule_type: 'none', start_at: null, stop_at: null, start_on_startup: false,
    on_complete_action: 'none', on_complete_program: null, sync_interval_minutes: null,
    last_sync_at: null, last_sync_changed: 0, last_sync_unchanged: 0, last_sync_removed: 0,
    last_sync_failed: 0, last_sync_skipped: 0, running: false, file_count: 3,
  },
  {
    id: 'queue-test', name: 'TestQueue', type: 'custom', is_main: false,
    schedule_type: 'none', start_at: null, stop_at: null, start_on_startup: false,
    on_complete_action: 'none', on_complete_program: null, sync_interval_minutes: null,
    last_sync_at: null, last_sync_changed: 0, last_sync_unchanged: 0, last_sync_removed: 0,
    last_sync_failed: 0, last_sync_skipped: 0, running: false, file_count: 2,
  },
]

const CATEGORIES: CategoryRow[] = [
  { id: 'cat-compressed', name: 'Compressed', default_folder: '', file_extensions: ['zip'], is_builtin: true },
]

const DOWNLOADS: DownloadRow[] = []

describe('sidebar queues (Round 87)', () => {
  test('the main queue is not shown, custom queues still are', () => {
    render(
      <CategorySidebar
        categories={CATEGORIES}
        downloads={DOWNLOADS}
        queues={QUEUES}
        grabberProjects={[]}
        selectedCategory={null}
        selectedQueue={null}
        selectedFilter="all"
        selectedGrabberId={null}
        onSelectCategory={noop}
        onSelectQueue={noop}
        onSelectFilter={noop}
        onAddCategory={noop}
        onAddQueue={noop}
        onDeleteCategory={noop}
        onOpenScheduler={noop}
        onRunGrabber={noop}
        onNewGrabber={noop}
        onSyncQueue={noop}
        onDropDownload={noop}
      />,
    )
    expect(screen.queryByText('Main download queue')).toBeNull()
    expect(screen.getByText('TestQueue')).toBeTruthy()
    // "New queue" is the only way to add one now.
    expect(screen.getByText('New queue')).toBeTruthy()
  })

  test('with only the main queue, the Queues section is just "New queue"', () => {
    render(
      <CategorySidebar
        categories={CATEGORIES}
        downloads={DOWNLOADS}
        queues={QUEUES.filter((q) => q.is_main)}
        grabberProjects={[]}
        selectedCategory={null}
        selectedQueue={null}
        selectedFilter="all"
        selectedGrabberId={null}
        onSelectCategory={noop}
        onSelectQueue={noop}
        onSelectFilter={noop}
        onAddCategory={noop}
        onAddQueue={noop}
        onDeleteCategory={noop}
        onOpenScheduler={noop}
        onRunGrabber={noop}
        onNewGrabber={noop}
        onSyncQueue={noop}
        onDropDownload={noop}
      />,
    )
    expect(screen.queryByText('Main download queue')).toBeNull()
    expect(screen.getByText('New queue')).toBeTruthy()
  })
})
