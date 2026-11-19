// Round 75, item 2 — sidebar selection.
//
// The bug: a category click called THREE App handlers
// (onSelectCategory + onSelectQueue(null) + onSelectFilter('all')).
// Each App handler clears the other two selections, and React
// batches the three setStates with the last write winning, so
// selectedCategory ended null and the click did nothing. The fix
// (CategorySidebar.tsx) calls exactly ONE handler per click.
//
// This test renders the REAL CategorySidebar wired to the REAL App
// selection functions (useSidebarSelection, shared with App.tsx)
// and the REAL visible-rows computation (selectVisibleRows), then
// clicks through the tree the way the user did on his PC.
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { useEffect, useState } from 'react'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { selectVisibleRows, useSidebarSelection } from '../lib/useSidebarSelection'
import { CategorySidebar } from './CategorySidebar'

const noop = () => {}

function makeDownload(over: {
  id: string
  filename: string
  category_id: string
  status?: DownloadRow['status']
}): DownloadRow {
  return {
    url: `https://example.com/${over.id}`,
    final_url: null,
    save_path: `C:\\Downloads\\${over.filename}`,
    queue_id: null,
    size_bytes: 1000,
    downloaded_bytes: 1000,
    status: over.status ?? 'complete',
    speed_bps: 0,
    eta_seconds: null,
    etag: null,
    last_modified: null,
    error_message: null,
    created_at: '2026-11-19T00:00:00Z',
    completed_at: '2026-11-19T00:00:00Z',
    updated_at: '2026-11-19T00:00:00Z',
    grabber_project_id: null,
    speed_limit_kbps: null,
    accept_ranges: true,
    quota_paused: false,
    scan_status: null,
    scan_detail: null,
    scan_scanned_at: null,
    active_connections: 0,
    segment_count: 1,
    ...over,
  }
}

const CATEGORIES: CategoryRow[] = [
  { id: 'cat-compressed', name: 'Compressed', default_folder: '', file_extensions: ['zip'], is_builtin: true },
  { id: 'cat-document', name: 'Document', default_folder: '', file_extensions: ['pdf'], is_builtin: true },
]

const DOWNLOADS: DownloadRow[] = [
  makeDownload({ id: 'd1', filename: 'archive-a.zip', category_id: 'cat-compressed' }),
  makeDownload({ id: 'd2', filename: 'archive-b.zip', category_id: 'cat-compressed' }),
  makeDownload({ id: 'd3', filename: 'manual.pdf', category_id: 'cat-document' }),
  makeDownload({ id: 'd4', filename: 'song.mp3', category_id: 'cat-music' }),
]

const QUEUES: QueueRow[] = [
  {
    id: 'queue-main', name: 'Main download queue', type: 'main', is_main: true,
    schedule_type: 'none', start_at: null, stop_at: null, start_on_startup: false,
    on_complete_action: 'none', on_complete_program: null, sync_interval_minutes: null,
    last_sync_at: null, last_sync_changed: 0, last_sync_unchanged: 0, last_sync_removed: 0,
    last_sync_failed: 0, last_sync_skipped: 0, running: false, file_count: 0,
  },
  {
    id: 'queue-test', name: 'TestQueue', type: 'custom', is_main: false,
    schedule_type: 'none', start_at: null, stop_at: null, start_on_startup: false,
    on_complete_action: 'none', on_complete_program: null, sync_interval_minutes: null,
    last_sync_at: null, last_sync_changed: 0, last_sync_unchanged: 0, last_sync_removed: 0,
    last_sync_failed: 0, last_sync_skipped: 0, running: false, file_count: 2,
  },
]

// The files a queue holds (loaded by the app when the queue is
// selected, via getQueueFiles). Deliberately different filenames
// from DOWNLOADS, so the test can tell queue files apart from the
// full list.
const QUEUE_FILES: Record<string, DownloadRow[]> = {
  'queue-test': [
    makeDownload({ id: 'q1', filename: 'queued-a.mp4', category_id: 'cat-compressed' }),
    makeDownload({ id: 'q2', filename: 'queued-b.mp4', category_id: 'cat-document' }),
  ],
}

interface CallLog {
  category: unknown[][]
  queue: unknown[][]
  filter: unknown[][]
}

interface HarnessProps {
  calls?: CallLog
}

/** The App's sidebar + table, stripped to the selection chain:
 *  the real sidebar, the real selection hook, the real
 *  visible-rows computation, and the App's queue-files effect
 *  (load the selected queue's files). */
function SidebarHarness({ calls }: HarnessProps) {
  const selection = useSidebarSelection()
  const [queueFiles, setQueueFiles] = useState<DownloadRow[]>([])

  useEffect(() => {
    if (selection.selectedQueue === null) {
      setQueueFiles([])
      return
    }
    setQueueFiles(QUEUE_FILES[selection.selectedQueue] ?? [])
  }, [selection.selectedQueue])

  const visible = selectVisibleRows({
    downloads: DOWNLOADS,
    queueFiles,
    selectedCategory: selection.selectedCategory,
    selectedQueue: selection.selectedQueue,
    selectedFilter: selection.selectedFilter,
    selectedGrabberId: selection.selectedGrabberId,
    searchQuery: '',
  })

  const onSelectCategory = (id: string | null) => {
    calls?.category.push([id])
    selection.selectCategory(id)
  }
  const onSelectQueue = (id: string | null) => {
    calls?.queue.push([id])
    selection.selectQueue(id)
  }
  const onSelectFilter = (filter: Parameters<typeof selection.selectFilter>[0]) => {
    calls?.filter.push([filter])
    selection.selectFilter(filter)
  }

  return (
    <div>
      <CategorySidebar
        categories={CATEGORIES}
        downloads={DOWNLOADS}
        queues={QUEUES}
        grabberProjects={[]}
        selectedCategory={selection.selectedCategory}
        selectedQueue={selection.selectedQueue}
        selectedFilter={selection.selectedFilter}
        selectedGrabberId={selection.selectedGrabberId}
        onSelectCategory={onSelectCategory}
        onSelectQueue={onSelectQueue}
        onSelectFilter={onSelectFilter}
        onAddCategory={noop}
        onAddQueue={noop}
        onDeleteCategory={noop}
        onOpenScheduler={noop}
        onRunGrabber={noop}
        onNewGrabber={noop}
        onSyncQueue={noop}
        onDropDownload={noop}
      />
      <ul aria-label="visible downloads">
        {visible.map((d) => (
          <li key={d.id}>{d.filename}</li>
        ))}
      </ul>
    </div>
  )
}

function visibleFilenames(): string[] {
  const list = screen.getByLabelText('visible downloads')
  return within(list)
    .getAllByRole('listitem')
    .map((li) => li.textContent ?? '')
}

function rowButton(label: string): HTMLElement {
  const el = screen.getByText(label).closest('button')
  if (!el) throw new Error(`no button for row ${label}`)
  return el
}

/** Plain-DOM aria-current check (no jest-dom dependency). */
function isCurrent(label: string): boolean {
  return rowButton(label).getAttribute('aria-current') === 'true'
}

describe('sidebar selection (Round 75)', () => {
  test('clicking a category moves aria-current to it and filters the table by category_id', () => {
    render(<SidebarHarness />)
    // Start: everything, All downloads current.
    expect(isCurrent('All downloads')).toBe(true)
    expect(visibleFilenames()).toEqual(['archive-a.zip', 'archive-b.zip', 'manual.pdf', 'song.mp3'])

    fireEvent.click(rowButton('Compressed'))

    // The bug this test pins: before the fix the batched triple
    // call left selectedCategory null, so aria-current stayed on
    // All downloads and the table stayed full.
    expect(isCurrent('Compressed')).toBe(true)
    expect(isCurrent('All downloads')).toBe(false)
    expect(visibleFilenames()).toEqual(['archive-a.zip', 'archive-b.zip'])
  })

  test('clicking a queue shows its files and marks the queue current', async () => {
    render(<SidebarHarness />)
    fireEvent.click(rowButton('TestQueue'))

    await waitFor(() => {
      expect(isCurrent('TestQueue')).toBe(true)
    })
    expect(visibleFilenames()).toEqual(['queued-a.mp4', 'queued-b.mp4'])
    expect(isCurrent('All downloads')).toBe(false)
  })

  test('clicking All downloads shows everything again', () => {
    render(<SidebarHarness />)
    fireEvent.click(rowButton('Compressed'))
    expect(visibleFilenames()).toEqual(['archive-a.zip', 'archive-b.zip'])

    fireEvent.click(rowButton('All downloads'))

    expect(isCurrent('All downloads')).toBe(true)
    expect(visibleFilenames()).toEqual(['archive-a.zip', 'archive-b.zip', 'manual.pdf', 'song.mp3'])
  })

  test('each click calls exactly ONE selection handler', () => {
    const calls: CallLog = { category: [], queue: [], filter: [] }
    render(<SidebarHarness calls={calls} />)

    fireEvent.click(rowButton('Compressed'))
    expect(calls.category).toEqual([['cat-compressed']])
    expect(calls.queue).toEqual([])
    expect(calls.filter).toEqual([])

    fireEvent.click(rowButton('TestQueue'))
    expect(calls.queue).toEqual([['queue-test']])
    expect(calls.category).toEqual([['cat-compressed']])
    expect(calls.filter).toEqual([])

    fireEvent.click(rowButton('All downloads'))
    expect(calls.filter).toEqual([['all']])
    expect(calls.category).toEqual([['cat-compressed']])
    expect(calls.queue).toEqual([['queue-test']])
  })
})
