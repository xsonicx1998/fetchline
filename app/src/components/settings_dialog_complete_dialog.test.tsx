// Round 87.3 — the "Download complete" dialog toggle saves
// AT ONCE.
//
// Root cause: the Settings dialog stages every setting in a
// draft and only writes it on "Save settings", so flipping
// this one checkbox and closing with X / Cancel / Escape
// silently discarded it. The R87.2 retest toggled it ON,
// closed the dialog, and the setting still read FALSE after
// a restart, with no dialog on the next finishes. The toggle
// now writes immediately (fresh re-read first, like the
// completion dialog's own "Don't show this dialog again"),
// so no Save click is needed — and a failed write reverts
// the checkbox and shows the error.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { SettingsDialog } from './SettingsDialog'
import * as api from '../lib/api'
import type { Settings } from '../lib/types'

vi.mock('../lib/api', () => ({
  deleteSiteLogin: vi.fn(),
  getProxyConfig: vi.fn(),
  getQuotaStatus: vi.fn(),
  getSettings: vi.fn(),
  getSoundSettings: vi.fn(),
  listSiteLogins: vi.fn(),
  resetQuota: vi.fn(),
  saveProxyConfig: vi.fn(),
  saveSiteLogin: vi.fn(),
  saveSoundSettings: vi.fn(),
  testAntivirusScanner: vi.fn(),
  testProxy: vi.fn(),
  testSoundFile: vi.fn(),
  updateCategory: vi.fn(),
  updateSettings: vi.fn(),
  ytdlpUpdate: vi.fn(),
  ytdlpVersion: vi.fn(),
  ytdlpVersionCached: vi.fn(),
  invalidateYtdlpVersion: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

function baseSettings(showDialog: boolean): Settings {
  return {
    default_download_folder: 'C:\\Downloads',
    start_on_startup: false,
    browser_integration: true,
    platform_downloads_enabled: true,
    browser_start_drained: true,
    clipboard_monitoring: true,
    show_progress_window: true,
    show_download_complete_dialog: showDialog,
    excluded_sites: [],
    check_updates_weekly: false,
    update_url: '',
    last_update_check_at: null,
    language: 'en',
    file_types_auto: [],
    max_connections: 8,
    connection_timeout_secs: 30,
    max_retries: 3,
    speed_limit_kbps: null,
    quota_enabled: false,
    quota_mb: null,
    quota_hours: null,
    antivirus_timeout_secs: 60,
    dialup_redial_attempts: 0,
    default_action: 'start_now',
    duplicate_action: 'rename',
  } as unknown as Settings
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.getProxyConfig).mockResolvedValue({
    id: 'proxy-1',
    name: 'Default',
    proxy_type: 'none',
    host: '',
    port: 0,
    username: null,
    use_for: [],
    bypass: [],
    enabled: false,
    has_password: false,
    auth: 'basic',
  })
  vi.mocked(api.listSiteLogins).mockResolvedValue([])
  vi.mocked(api.getSoundSettings).mockResolvedValue([])
  vi.mocked(api.getQuotaStatus).mockResolvedValue({
    enabled: false,
    limit_mb: null,
    window_hours: null,
    used_bytes: 0,
    window_start_unix: null,
    window_end_unix: null,
    reached: false,
    quota_paused_downloads: 0,
  })
  vi.mocked(api.ytdlpVersionCached).mockResolvedValue({
    version: '2026.10.26',
    path: 'binaries/yt-dlp',
    qjs: true,
    ffmpeg: true,
  })
})

function renderDialog() {
  return render(
    <SettingsDialog
      open
      categories={[]}
      onClose={() => {}}
      onSaved={() => {}}
      onCheckUpdates={() => {}}
    />,
  )
}

async function openGeneralTab() {
  renderDialog()
  await waitFor(
    () =>
      expect(
        screen.getByLabelText(/Show the .Download complete. dialog when a download finishes/),
      ).toBeTruthy(),
    { timeout: 5000 },
  )
  return screen.getByLabelText(
    /Show the .Download complete. dialog when a download finishes/,
  ) as HTMLInputElement
}

describe('Download complete dialog toggle (Round 87.3)', () => {
  test('flipping it ON writes at once, with no Save click', async () => {
    const before = baseSettings(false)
    vi.mocked(api.getSettings).mockResolvedValue(before)
    vi.mocked(api.updateSettings).mockResolvedValue(baseSettings(true))

    const checkbox = await openGeneralTab()
    expect(checkbox.checked).toBe(false)

    fireEvent.click(checkbox)

    // The write fires on the flip alone — "Save settings" is
    // never clicked in this test.
    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    const written = vi.mocked(api.updateSettings).mock.calls[0][0]
    expect(written.show_download_complete_dialog).toBe(true)
    // The checkbox follows the flip at once, and the write
    // re-read the settings fresh first (the open-time fetch
    // plus this re-read).
    expect(checkbox.checked).toBe(true)
    expect(vi.mocked(api.getSettings).mock.calls.length).toBeGreaterThanOrEqual(2)
  })

  test('flipping it OFF writes at once too', async () => {
    const before = baseSettings(true)
    vi.mocked(api.getSettings).mockResolvedValue(before)
    vi.mocked(api.updateSettings).mockResolvedValue(baseSettings(false))

    const checkbox = await openGeneralTab()
    expect(checkbox.checked).toBe(true)

    fireEvent.click(checkbox)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    const written = vi.mocked(api.updateSettings).mock.calls[0][0]
    expect(written.show_download_complete_dialog).toBe(false)
    expect(checkbox.checked).toBe(false)
  })

  test('a failed write reverts the checkbox and shows the error', async () => {
    const before = baseSettings(false)
    vi.mocked(api.getSettings).mockResolvedValue(before)
    vi.mocked(api.updateSettings).mockRejectedValue(new Error('Database is locked'))

    const checkbox = await openGeneralTab()
    fireEvent.click(checkbox)

    // Plain-DOM text check (no jest-dom dependency — see
    // sidebar_selection.test.tsx): `toHaveTextContent` does
    // not exist here and throws inside waitFor forever.
    await waitFor(
      () => expect(screen.getByRole('alert').textContent).toBe('Database is locked'),
      { timeout: 5000 },
    )
    expect(checkbox.checked).toBe(false)
  })
})
