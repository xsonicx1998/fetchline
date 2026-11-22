// Round 87.5 (C4) — "Shut down computer after download" on a
// download that ALREADY finished.
//
// The comprehensive test reported "no Shut down after download
// and no 30-second countdown": the option IS in the progress
// window's Options tab and the countdown banner with Cancel
// exists — but the completion effect only fires on a status
// FLIP into 'complete', so checking the box on a finished
// download did nothing (and the test's small files finished
// before the option could be armed mid-run). Checking either
// completion box on a finished download now acts at once:
// shut-down starts its 30 s countdown, close exits at once.
// Unchecking shut-down during a countdown stops it.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { DownloadRow, Settings } from '../lib/types'
import { ProgressWindow } from './ProgressWindow'

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

const row: DownloadRow = {
  id: 'dl-1',
  url: 'https://example.com/small.zip',
  final_url: null,
  filename: 'small.zip',
  save_path: 'C:\\Downloads\\small.zip',
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

vi.mock('../lib/api', () => ({
  cancelDownload: vi.fn(),
  exitApp: vi.fn(),
  getDownloadCompletionOptions: vi.fn(),
  getProgressSnapshot: vi.fn(),
  getSettings: vi.fn(),
  listDownloads: vi.fn(),
  openFile: vi.fn(),
  openFolder: vi.fn(),
  pauseDownload: vi.fn(),
  reloadWebviewWindow: vi.fn(),
  resumeDownload: vi.fn(),
  retryNow: vi.fn(),
  setDownloadCompletionOptions: vi.fn(),
  setDownloadSpeedLimit: vi.fn(),
  shutdownComputer: vi.fn(),
  withTimeout: (p: Promise<unknown>) => p,
}))

// The mocked module, typed loosely so the tests can set
// per-test behaviour without importing the real api.
import * as api from '../lib/api'

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.listDownloads).mockResolvedValue([row])
  vi.mocked(api.getProgressSnapshot).mockResolvedValue(null)
  vi.mocked(api.getSettings).mockResolvedValue({
    show_download_complete_dialog: false,
    speed_limit_kbps: null,
  } as unknown as Settings)
  vi.mocked(api.getDownloadCompletionOptions).mockResolvedValue({
    close_after: false,
    shutdown_after: false,
  })
  vi.mocked(api.setDownloadCompletionOptions).mockResolvedValue(undefined)
  vi.mocked(api.exitApp).mockResolvedValue(undefined)
  vi.mocked(api.shutdownComputer).mockResolvedValue(undefined)
})

async function openOptionsTab() {
  render(<ProgressWindow downloadId="dl-1" />)
  // The row polls in (500 ms interval, first tick at once).
  await waitFor(
    () => expect(screen.getByRole('tab', { name: 'Options' })).toBeTruthy(),
    { timeout: 5000 },
  )
  fireEvent.click(screen.getByRole('tab', { name: 'Options' }))
  return {
    shutdownBox: screen.getByLabelText(
      'Shut down computer after download',
    ) as HTMLInputElement,
    closeBox: screen.getByLabelText('Close program after download') as HTMLInputElement,
  }
}

describe('completion options on an already-finished download (Round 87.5)', () => {
  test('checking "Shut down computer after download" starts the 30 s countdown at once', async () => {
    const { shutdownBox } = await openOptionsTab()
    expect(shutdownBox.checked).toBe(false)

    fireEvent.click(shutdownBox)

    await waitFor(
      () => expect(vi.mocked(api.setDownloadCompletionOptions)).toHaveBeenCalledWith(
        'dl-1',
        { close_after: false, shutdown_after: true },
      ),
      { timeout: 5000 },
    )
    const banner = screen.getByRole('alertdialog')
    expect(within(banner).getByText('Shut down the computer?')).toBeTruthy()
    expect(within(banner).getByText('30s')).toBeTruthy()
    expect(within(banner).getByText('Cancel')).toBeTruthy()
    expect(within(banner).getByText('Shut down now')).toBeTruthy()
  })

  test('Cancel on the countdown clears it and unchecks the option', async () => {
    const { shutdownBox } = await openOptionsTab()
    fireEvent.click(shutdownBox)
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy(), {
      timeout: 5000,
    })

    fireEvent.click(within(screen.getByRole('alertdialog')).getByText('Cancel'))

    await waitFor(
      () => expect(screen.queryByRole('alertdialog')).toBeNull(),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.setDownloadCompletionOptions)).toHaveBeenCalledWith(
      'dl-1',
      { close_after: false, shutdown_after: false },
    )
    expect(shutdownBox.checked).toBe(false)
  })

  test('checking "Close program after download" on a finished download exits at once', async () => {
    const { closeBox } = await openOptionsTab()

    fireEvent.click(closeBox)

    await waitFor(
      () => expect(vi.mocked(api.exitApp)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.setDownloadCompletionOptions)).toHaveBeenCalledWith(
      'dl-1',
      { close_after: true, shutdown_after: false },
    )
  })

  test('unchecking shut-down during a countdown stops it', async () => {
    const { shutdownBox } = await openOptionsTab()
    fireEvent.click(shutdownBox)
    await waitFor(() => expect(screen.getByRole('alertdialog')).toBeTruthy(), {
      timeout: 5000,
    })

    // The banner covers the window, but the checkbox itself
    // must still clear the countdown when unchecked.
    fireEvent.click(shutdownBox)

    await waitFor(
      () => expect(screen.queryByRole('alertdialog')).toBeNull(),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.shutdownComputer)).not.toHaveBeenCalled()
  })
})
