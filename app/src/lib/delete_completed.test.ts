// Round 76 (6) — Delete Completed.
//
// The user's report: Delete Completed "deleted more than just
// Finished rows". The filter was PROVEN correct in code
// (status === 'complete', single-row soft delete, no cascade),
// so this test pins it: the selection is exactly the complete
// rows — every other status is excluded — across every category,
// queue and filter (the whole list, not just the visible view).
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import type { DownloadRow, DownloadStatus } from './types'
import { selectCompletedDownloads } from './useSidebarSelection'

function makeDownload(over: {
  id: string
  filename: string
  category_id: string
  status: DownloadStatus
}): DownloadRow {
  return {
    id: over.id,
    url: `https://example.com/${over.id}`,
    final_url: null,
    filename: over.filename,
    save_path: `C:\\Downloads\\${over.filename}`,
    category_id: over.category_id,
    queue_id: null,
    size_bytes: 1000,
    downloaded_bytes: 1000,
    status: over.status,
    speed_bps: 0,
    eta_seconds: null,
    etag: null,
    last_modified: null,
    error_message: null,
    created_at: '2026-11-19T00:00:00Z',
    completed_at: over.status === 'complete' ? '2026-11-19T00:00:00Z' : null,
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

describe('selectCompletedDownloads (Round 76)', () => {
  test('selects only complete rows, excluding every other status', () => {
    const rows = [
      makeDownload({ id: 'c1', filename: 'done-a.zip', category_id: 'cat-compressed', status: 'complete' }),
      makeDownload({ id: 'q1', filename: 'waiting.mp4', category_id: 'cat-compressed', status: 'queued' }),
      makeDownload({ id: 'd1', filename: 'running.mp4', category_id: 'cat-compressed', status: 'downloading' }),
      makeDownload({ id: 'p1', filename: 'paused.mp4', category_id: 'cat-compressed', status: 'paused' }),
      makeDownload({ id: 'e1', filename: 'failed.mp4', category_id: 'cat-compressed', status: 'error' }),
      makeDownload({ id: 'x1', filename: 'gone.mp4', category_id: 'cat-compressed', status: 'deleted' }),
      makeDownload({ id: 'c2', filename: 'done-b.pdf', category_id: 'cat-document', status: 'complete' }),
    ]
    expect(selectCompletedDownloads(rows).map((d) => d.id)).toEqual(['c1', 'c2'])
  })

  test('scope is the whole list: complete rows in other categories are selected too', () => {
    const rows = [
      makeDownload({ id: 'c1', filename: 'done-a.zip', category_id: 'cat-compressed', status: 'complete' }),
      makeDownload({ id: 'c2', filename: 'done-b.pdf', category_id: 'cat-document', status: 'complete' }),
      makeDownload({ id: 'c3', filename: 'done-c.mp3', category_id: 'cat-music', status: 'complete' }),
      makeDownload({ id: 'd1', filename: 'running.mp4', category_id: 'cat-music', status: 'downloading' }),
    ]
    expect(selectCompletedDownloads(rows).map((d) => d.id)).toEqual(['c1', 'c2', 'c3'])
  })

  test('empty list and no complete rows select nothing', () => {
    expect(selectCompletedDownloads([])).toEqual([])
    const rows = [
      makeDownload({ id: 'd1', filename: 'running.mp4', category_id: 'cat-compressed', status: 'downloading' }),
      makeDownload({ id: 'p1', filename: 'paused.mp4', category_id: 'cat-compressed', status: 'paused' }),
    ]
    expect(selectCompletedDownloads(rows)).toEqual([])
  })
})
