// Round 85 — one error dialog for every failure.
//
// The R84 retest: OK "sometimes needs 2–3 clicks" to dismiss.
// Cause (verified in code): every failed download opened its OWN
// stacked OS window (label `error-<download id>`), so one OK
// click only dismissed the top copy. `open_error_dialog` now
// opens a single window and emits `error-dialog-queued` for
// later failures; this dialog queues them and OK steps through
// them in the SAME window ("N more errors" + "Dismiss all"), so
// one click is always exactly one error.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { DownloadErrorDialog } from './DownloadErrorDialog'
import * as api from '../lib/api'
import type { DownloadRow } from '../lib/types'

vi.mock('../lib/api', () => ({
  listDownloads: vi.fn(),
  playErrorSound: vi.fn(),
  withTimeout: vi.fn((promise: Promise<unknown>) => promise),
}))

const closeMock = vi.fn()
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    close: closeMock,
    isMaximized: () => Promise.resolve(false),
    onResized: () => Promise.resolve(() => {}),
  }),
}))

type QueuedHandler = (e: { payload: { download_id: string } }) => void
let queuedHandler: QueuedHandler | null = null
const unlistenMock = vi.fn()
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((_event: string, handler: QueuedHandler) => {
    queuedHandler = handler
    return Promise.resolve(unlistenMock)
  }),
}))

function row(id: string, filename: string, error_message: string): DownloadRow {
  return {
    id,
    url: `https://example.com/${filename}`,
    final_url: null,
    filename,
    save_path: `C:\\Downloads\\${filename}`,
    category_id: '',
    queue_id: null,
    size_bytes: 1000,
    downloaded_bytes: 0,
    status: 'error',
    speed_bps: 0,
    eta_seconds: null,
    etag: null,
    last_modified: null,
    error_message,
    created_at: '2026-11-21T00:00:00Z',
    completed_at: null,
    updated_at: '2026-11-21T00:00:00Z',
    grabber_project_id: null,
    speed_limit_kbps: null,
    accept_ranges: true,
    quota_paused: false,
    scan_status: null,
    scan_detail: null,
    scan_scanned_at: null,
    active_connections: 0,
    segment_count: 0,
  }
}

const ROWS = [
  row('dl-1', 'file-one.zip', 'Connection lost'),
  row('dl-2', 'file-two.zip', 'Server not found'),
  row('dl-3', 'file-three.zip', 'Not Found (404)'),
]

beforeEach(() => {
  vi.clearAllMocks()
  queuedHandler = null
  vi.mocked(api.listDownloads).mockImplementation(() =>
    Promise.resolve(ROWS),
  )
  vi.mocked(api.playErrorSound).mockResolvedValue('ok')
})

function renderDialog() {
  return render(<DownloadErrorDialog downloadId="dl-1" />)
}

async function queue(id: string) {
  expect(queuedHandler).not.toBeNull()
  await act(async () => {
    queuedHandler!({ payload: { download_id: id } })
  })
}

describe('error dialog queue', () => {
  test('shows the first error with OK focused', async () => {
    renderDialog()
    await waitFor(() => expect(screen.getByText('Connection lost')).toBeTruthy())
    expect(screen.getByText('file-one.zip')).toBeTruthy()
    // Round 70 — OK is focused on open, so Enter confirms at once.
    expect(document.activeElement?.textContent).toBe('OK')
  })

  test('queues three errors: exactly one OK click per error, then closes', async () => {
    renderDialog()
    await waitFor(() => expect(screen.getByText('Connection lost')).toBeTruthy())

    await queue('dl-2')
    await queue('dl-3')
    await waitFor(() => expect(screen.getByText('2 more errors')).toBeTruthy())

    // Click 1 dismisses error 1 and shows error 2 — the window stays open.
    fireEvent.click(screen.getByText('OK'))
    await waitFor(() => expect(screen.getByText('Server not found')).toBeTruthy())
    expect(screen.getByText('1 more error')).toBeTruthy()
    expect(closeMock).not.toHaveBeenCalled()

    // Click 2 dismisses error 2 and shows error 3.
    fireEvent.click(screen.getByText('OK'))
    await waitFor(() => expect(screen.getByText('Not Found (404)')).toBeTruthy())
    expect(screen.queryByText(/more errors?/)).toBeNull()
    expect(closeMock).not.toHaveBeenCalled()

    // Click 3 dismisses the last error and closes the window — once.
    fireEvent.click(screen.getByText('OK'))
    await waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1))
  })

  test('Dismiss all closes at once without stepping through the queue', async () => {
    renderDialog()
    await waitFor(() => expect(screen.getByText('Connection lost')).toBeTruthy())

    await queue('dl-2')
    await queue('dl-3')
    await waitFor(() => expect(screen.getByText('2 more errors')).toBeTruthy())

    fireEvent.click(screen.getByText('Dismiss all'))
    await waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1))
    // Errors 2 and 3 were never shown.
    expect(screen.queryByText('Server not found')).toBeNull()
    expect(screen.queryByText('Not Found (404)')).toBeNull()
  })

  test('Escape steps through the queue like OK, then closes', async () => {
    renderDialog()
    await waitFor(() => expect(screen.getByText('Connection lost')).toBeTruthy())

    await queue('dl-2')
    await waitFor(() => expect(screen.getByText('1 more error')).toBeTruthy())

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(screen.getByText('Server not found')).toBeTruthy())
    expect(closeMock).not.toHaveBeenCalled()

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1))
  })

  test('a duplicate queued id is ignored', async () => {
    renderDialog()
    await waitFor(() => expect(screen.getByText('Connection lost')).toBeTruthy())

    await queue('dl-2')
    await queue('dl-2')
    await waitFor(() => expect(screen.getByText('1 more error')).toBeTruthy())

    fireEvent.click(screen.getByText('OK'))
    await waitFor(() => expect(screen.getByText('Server not found')).toBeTruthy())
    // No second copy of dl-2 waits behind it.
    expect(screen.queryByText(/more errors?/)).toBeNull()
    expect(closeMock).not.toHaveBeenCalled()
  })

  test('the message variant (no row) still closes on one OK click', async () => {
    render(<DownloadErrorDialog message="Sign in to confirm you’re not a bot" title="video.mp4" onClose={closeMock} />)
    await waitFor(() =>
      expect(screen.getByText('Sign in to confirm you’re not a bot')).toBeTruthy(),
    )
    expect(screen.queryByText(/more errors?/)).toBeNull()
    fireEvent.click(screen.getByText('OK'))
    expect(closeMock).toHaveBeenCalledTimes(1)
  })
})
