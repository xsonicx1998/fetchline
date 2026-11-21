// Round 87 (10) — ONE "Download complete" dialog for every
// completion, like the R85 error-dialog queue: later
// completions queue into the open window ("N more completed"),
// Close steps through them in the same window, and the last
// Close closes it — no stacked windows.
//
// Run with: npm run test:ui

import { describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import type { DownloadRow } from '../lib/types'
import { DownloadCompleteDialog } from './DownloadCompleteDialog'

const closeMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    minimize: () => Promise.resolve(),
    hide: () => Promise.resolve(),
    close: closeMock,
    destroy: () => Promise.resolve(),
    startDragging: () => Promise.resolve(),
    isMaximized: () => Promise.resolve(false),
    onResized: () => Promise.resolve(() => {}),
  }),
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: () => Promise.resolve(),
}))

type QueuedHandler = (e: { payload: { download_id: string } }) => void
let queuedHandler: QueuedHandler | null = null
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((_event: string, handler: QueuedHandler) => {
    queuedHandler = handler
    return Promise.resolve(() => {})
  }),
}))

function makeRow(id: string, filename: string): DownloadRow {
  return {
    id,
    url: `https://example.com/${id}`,
    final_url: null,
    filename,
    save_path: `C:\\Downloads\\${filename}`,
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
    active_connections: 0,
    segment_count: 1,
  }
}

const ROWS = [makeRow('d1', 'file-a.zip'), makeRow('d2', 'file-b.zip')]

vi.mock('../lib/api', () => ({
  getSettings: () =>
    Promise.resolve({
      default_action: 'start_now',
      default_download_folder: '',
      confirm_delete: false,
      clipboard_monitoring: false,
      show_download_complete_dialog: true,
    }),
  listDownloads: () => Promise.resolve(ROWS),
  openFile: () => Promise.resolve(),
  openFileWith: () => Promise.resolve(),
  openFolder: () => Promise.resolve(),
  playCompletionSound: () => Promise.resolve(),
  updateSettings: (s: unknown) => Promise.resolve(s),
  withTimeout: (p: Promise<unknown>) => p,
}))

describe('DownloadCompleteDialog queue (Round 87)', () => {
  test('a queued completion steps through the same window, then closes', async () => {
    render(<DownloadCompleteDialog downloadId="d1" />)
    await screen.findByText('file-a.zip')
    expect(closeMock).not.toHaveBeenCalled()

    // A second download finishes while this dialog is open.
    if (!queuedHandler) throw new Error('no complete-dialog-queued listener')
    queuedHandler({ payload: { download_id: 'd2' } })
    await screen.findByText('1 more completed')
    expect(screen.getByText('Dismiss all')).toBeTruthy()

    // Close steps to the next completion instead of closing.
    fireEvent.click(screen.getByText('Close'))
    await screen.findByText('file-b.zip')
    expect(screen.queryByText('1 more completed')).toBeNull()
    expect(closeMock).not.toHaveBeenCalled()

    // The last Close closes the window.
    fireEvent.click(screen.getByText('Close'))
    expect(closeMock).toHaveBeenCalledTimes(1)
  })

  test('a duplicate queued id shows only once', async () => {
    render(<DownloadCompleteDialog downloadId="d1" />)
    await screen.findByText('file-a.zip')
    if (!queuedHandler) throw new Error('no complete-dialog-queued listener')
    queuedHandler({ payload: { download_id: 'd2' } })
    queuedHandler({ payload: { download_id: 'd2' } })
    await screen.findByText('1 more completed')
    expect(screen.queryByText('2 more completed')).toBeNull()
  })
})
