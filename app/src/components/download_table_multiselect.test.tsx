// Round 87 (13b) — downloads-table multi-select.
//
// A plain click selects one row, Ctrl/Cmd-click toggles one
// row, Shift-click selects the range from the anchor, and
// dragging across rows extends the range; Delete removes ALL
// selected rows at once (in table order). Dragging selects
// ROWS, never text (13a sets `user-select: none` page-wide).
//
// Run with: npm run test:ui

import { describe, expect, test, vi } from 'vitest'
import { useState } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import { DownloadTable } from './DownloadTable'

const noop = () => {}

function makeDownload(over: { id: string; filename: string }): DownloadRow {
  return {
    id: over.id,
    url: `https://example.com/${over.id}`,
    final_url: null,
    filename: over.filename,
    save_path: `C:\\Downloads\\${over.filename}`,
    category_id: 'cat-compressed',
    queue_id: null,
    size_bytes: 1000,
    downloaded_bytes: 1000,
    status: 'complete',
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
    file_missing: false,
    active_connections: 0,
    segment_count: 1,
  }
}

const DOWNLOADS: DownloadRow[] = [
  makeDownload({ id: 'd1', filename: 'file-a.zip' }),
  makeDownload({ id: 'd2', filename: 'file-b.zip' }),
  makeDownload({ id: 'd3', filename: 'file-c.zip' }),
  makeDownload({ id: 'd4', filename: 'file-d.zip' }),
]

const CATEGORIES: CategoryRow[] = [
  { id: 'cat-compressed', name: 'Compressed', default_folder: '', file_extensions: ['zip'], is_builtin: true },
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

function rowElement(filename: string): HTMLElement {
  const el = screen.getByText(filename).closest('tr')
  if (!el) throw new Error(`no row for ${filename}`)
  return el
}

function isSelected(filename: string): boolean {
  return rowElement(filename).getAttribute('aria-selected') === 'true'
}

function renderTable(props?: {
  onSelect?: (id: string | null) => void
  onSelectionChange?: (ids: string[]) => void
  onRemove?: (d: DownloadRow) => void
  onRemoveMany?: (ds: DownloadRow[]) => void
}) {
  const onSelect = props?.onSelect ?? noop
  const onSelectionChange = props?.onSelectionChange ?? noop
  const onRemove = props?.onRemove ?? noop
  const onRemoveMany = props?.onRemoveMany ?? noop
  render(
    <DownloadTable
      downloads={DOWNLOADS}
      categories={CATEGORIES}
      queues={QUEUES}
      selectedId={null}
      onSelect={onSelect}
      onSelectionChange={onSelectionChange}
      onRemoveMany={onRemoveMany}
      onOpenProgress={noop}
      onAdd={noop}
      onOpenFile={noop}
      onOpenWith={noop}
      onOpenFolder={noop}
      onRedownload={noop}
      onResume={noop}
      onPause={noop}
      onMoveToCategory={noop}
      onRefreshAddress={noop}
      onRemove={onRemove}
      onMoveToQueue={noop}
      onDeleteFromQueue={noop}
      onShowProperties={noop}
    />,
  )
}

const fakeDataTransfer = () => ({
  setData: vi.fn(),
  effectAllowed: '',
})

describe('downloads-table multi-select (Round 87)', () => {
  test('plain click selects one row', () => {
    const onSelect = vi.fn()
    const onSelectionChange = vi.fn()
    renderTable({ onSelect, onSelectionChange })
    fireEvent.click(rowElement('file-b.zip'))
    expect(isSelected('file-b.zip')).toBe(true)
    expect(isSelected('file-a.zip')).toBe(false)
    expect(onSelect).toHaveBeenCalledWith('d2')
    expect(onSelectionChange).toHaveBeenLastCalledWith(['d2'])
  })

  test('Ctrl+click adds a row, Ctrl+click again removes it', () => {
    const onSelectionChange = vi.fn()
    renderTable({ onSelectionChange })
    fireEvent.click(rowElement('file-a.zip'))
    fireEvent.click(rowElement('file-c.zip'), { ctrlKey: true })
    expect(isSelected('file-a.zip')).toBe(true)
    expect(isSelected('file-c.zip')).toBe(true)
    expect(isSelected('file-b.zip')).toBe(false)
    expect(onSelectionChange).toHaveBeenLastCalledWith(['d1', 'd3'])
    fireEvent.click(rowElement('file-c.zip'), { ctrlKey: true })
    expect(isSelected('file-c.zip')).toBe(false)
    expect(isSelected('file-a.zip')).toBe(true)
    expect(onSelectionChange).toHaveBeenLastCalledWith(['d1'])
  })

  test('Shift+click selects the range from the anchor', () => {
    const onSelectionChange = vi.fn()
    renderTable({ onSelectionChange })
    fireEvent.click(rowElement('file-b.zip'))
    fireEvent.click(rowElement('file-d.zip'), { shiftKey: true })
    expect(isSelected('file-a.zip')).toBe(false)
    expect(isSelected('file-b.zip')).toBe(true)
    expect(isSelected('file-c.zip')).toBe(true)
    expect(isSelected('file-d.zip')).toBe(true)
    expect(onSelectionChange).toHaveBeenLastCalledWith(['d2', 'd3', 'd4'])
  })

  test('dragging across rows extends the selection', () => {
    const onSelectionChange = vi.fn()
    renderTable({ onSelectionChange })
    fireEvent.dragStart(rowElement('file-a.zip'), {
      dataTransfer: fakeDataTransfer(),
    })
    expect(isSelected('file-a.zip')).toBe(true)
    fireEvent.dragEnter(rowElement('file-c.zip'))
    expect(isSelected('file-a.zip')).toBe(true)
    expect(isSelected('file-b.zip')).toBe(true)
    expect(isSelected('file-c.zip')).toBe(true)
    expect(isSelected('file-d.zip')).toBe(false)
    expect(onSelectionChange).toHaveBeenLastCalledWith(['d1', 'd2', 'd3'])
  })

  test('Delete removes ALL selected rows at once, in table order', () => {
    const onRemoveMany = vi.fn()
    const onRemove = vi.fn()
    renderTable({ onRemoveMany, onRemove })
    fireEvent.click(rowElement('file-a.zip'))
    fireEvent.click(rowElement('file-c.zip'), { ctrlKey: true })
    fireEvent.click(rowElement('file-d.zip'), { ctrlKey: true })
    fireEvent.keyDown(rowElement('file-d.zip'), { key: 'Delete' })
    expect(onRemoveMany).toHaveBeenCalledTimes(1)
    expect(onRemoveMany.mock.calls[0][0].map((d: DownloadRow) => d.id)).toEqual([
      'd1',
      'd3',
      'd4',
    ])
    expect(onRemove).not.toHaveBeenCalled()
  })

  test('Delete with one row selected removes just that row', () => {
    const onRemoveMany = vi.fn()
    renderTable({ onRemoveMany })
    fireEvent.click(rowElement('file-b.zip'))
    fireEvent.keyDown(rowElement('file-b.zip'), { key: 'Delete' })
    expect(onRemoveMany).toHaveBeenCalledTimes(1)
    expect(onRemoveMany.mock.calls[0][0].map((d: DownloadRow) => d.id)).toEqual(['d2'])
  })

  test('right-click collapses a multi-selection to that row', () => {
    renderTable({})
    fireEvent.click(rowElement('file-a.zip'))
    fireEvent.click(rowElement('file-b.zip'), { ctrlKey: true })
    expect(isSelected('file-a.zip')).toBe(true)
    fireEvent.contextMenu(rowElement('file-c.zip'))
    expect(isSelected('file-a.zip')).toBe(false)
    expect(isSelected('file-b.zip')).toBe(false)
    expect(isSelected('file-c.zip')).toBe(true)
  })
})

describe('deselecting the last row (Round 87.1)', () => {
  // A wrapper that mirrors App: the anchor (`selectedId`) and
  // the multi-selection live in the parent, fed by the table's
  // callbacks — exactly the wiring whose staleness deleted a
  // deselected row in R87.
  function StatefulTable({
    onRemove,
    onRemoveMany,
  }: {
    onRemove: (d: DownloadRow) => void
    onRemoveMany: (ds: DownloadRow[]) => void
  }) {
    const [selectedId, setSelectedId] = useState<string | null>('d1')
    // The multi-selection itself lives in the table; the parent
    // only mirrors it (and clears the anchor when it empties),
    // so only the setter is kept here.
    const [, setSelectedIds] = useState<string[]>(['d1'])
    return (
      <DownloadTable
        downloads={DOWNLOADS}
        categories={CATEGORIES}
        queues={QUEUES}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onSelectionChange={(ids) => {
          setSelectedIds(ids)
          if (ids.length === 0) setSelectedId(null)
        }}
        onRemoveMany={onRemoveMany}
        onOpenProgress={noop}
        onAdd={noop}
        onOpenFile={noop}
        onOpenWith={noop}
        onOpenFolder={noop}
        onRedownload={noop}
        onResume={noop}
        onPause={noop}
        onMoveToCategory={noop}
        onRefreshAddress={noop}
        onRemove={onRemove}
        onMoveToQueue={noop}
        onDeleteFromQueue={noop}
        onShowProperties={noop}
      />
    )
  }

  test('Ctrl+clicking the last selected row off clears the anchor, and Delete is then a no-op', () => {
    const onRemove = vi.fn()
    const onRemoveMany = vi.fn()
    render(<StatefulTable onRemove={onRemove} onRemoveMany={onRemoveMany} />)
    expect(isSelected('file-a.zip')).toBe(true)
    // Ctrl+click the only selected row: it toggles OFF.
    fireEvent.click(rowElement('file-a.zip'), { ctrlKey: true })
    expect(isSelected('file-a.zip')).toBe(false)
    // Delete with nothing selected removes nothing at all —
    // neither the multi path nor the single-row fallback.
    fireEvent.keyDown(rowElement('file-a.zip'), { key: 'Delete' })
    expect(onRemoveMany).not.toHaveBeenCalled()
    expect(onRemove).not.toHaveBeenCalled()
    // And the row is still there to click again.
    fireEvent.click(rowElement('file-a.zip'))
    expect(isSelected('file-a.zip')).toBe(true)
  })

  test('Ctrl+clicking the last selected row off reports onSelect(null)', () => {
    const onSelect = vi.fn()
    const onSelectionChange = vi.fn()
    renderTable({ onSelect, onSelectionChange })
    fireEvent.click(rowElement('file-b.zip'))
    expect(onSelect).toHaveBeenLastCalledWith('d2')
    fireEvent.click(rowElement('file-b.zip'), { ctrlKey: true })
    expect(onSelectionChange).toHaveBeenLastCalledWith([])
    expect(onSelect).toHaveBeenLastCalledWith(null)
  })
})
