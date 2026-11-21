// Round 79 (Bug 3) — the "Update yt-dlp" button.
//
// In the Round 78 retest the button was clicked twice with no
// visible progress, status message, or version change within
// ~10 s (the ~23 MB download outlasts the observation). The
// button must now show a status IMMEDIATELY on click
// ("Checking for updates…") and must say "already the newest
// version" when the version did not change, instead of claiming
// an update.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { SettingsDialog } from './SettingsDialog'
import * as api from '../lib/api'
import type { Settings, YtdlpStatus } from '../lib/types'

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

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function ytdlpStatus(version: string | null): YtdlpStatus {
  return { version, path: 'binaries/yt-dlp', qjs: true, ffmpeg: true }
}

const settings = {
  default_download_folder: 'C:\\Downloads',
  start_on_startup: false,
  browser_integration: true,
  platform_downloads_enabled: true,
  browser_start_drained: true,
  clipboard_monitoring: true,
  show_progress_window: true,
  show_download_complete_dialog: true,
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

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.getSettings).mockResolvedValue(settings)
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
  vi.mocked(api.ytdlpVersion).mockResolvedValue(ytdlpStatus('2026.10.26'))
  vi.mocked(api.ytdlpVersionCached).mockResolvedValue(ytdlpStatus('2026.10.26'))
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

async function openVideoSection() {
  renderDialog()
  await waitFor(
    () => expect(screen.getByText(/yt-dlp 2026\.10\.26/)).toBeTruthy(),
    { timeout: 5000 },
  )
}

describe('Update yt-dlp button', () => {
  test('shows a status immediately, then “already the newest” when unchanged', async () => {
    const pending = deferred<YtdlpStatus>()
    vi.mocked(api.ytdlpUpdate).mockReturnValue(pending.promise)
    await openVideoSection()

    fireEvent.click(screen.getByText('Update yt-dlp'))
    // Immediate feedback — before the download finishes.
    expect(screen.getByText('Checking for updates…')).toBeTruthy()
    expect(screen.getByText('Updating…')).toBeTruthy()

    pending.resolve(ytdlpStatus('2026.10.26'))
    await waitFor(
      () =>
        expect(
          screen.getByText('yt-dlp is already the newest version (2026.10.26).'),
        ).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(screen.queryByText('Checking for updates…')).toBeNull()
  })

  test('reports the new version when an update lands', async () => {
    vi.mocked(api.ytdlpUpdate).mockResolvedValue(ytdlpStatus('2026.11.01'))
    await openVideoSection()

    fireEvent.click(screen.getByText('Update yt-dlp'))
    await waitFor(
      () => expect(screen.getByText('yt-dlp updated to 2026.11.01.')).toBeTruthy(),
      { timeout: 5000 },
    )
    // Round 86 — the update replaced the binary, so the
    // session-cached version must be cleared for the next open.
    expect(vi.mocked(api.invalidateYtdlpVersion)).toHaveBeenCalled()
  })

  test('a failed update shows the reason', async () => {
    vi.mocked(api.ytdlpUpdate).mockRejectedValue(
      new Error('Network error: could not reach the release server'),
    )
    await openVideoSection()

    fireEvent.click(screen.getByText('Update yt-dlp'))
    await waitFor(
      () =>
        expect(
          screen.getByText('Network error: could not reach the release server'),
        ).toBeTruthy(),
      { timeout: 5000 },
    )
  })
})

describe('Settings load does not wait for the yt-dlp version', () => {
  // Round 86 — the version fetch spawns `yt-dlp --version`
  // (~1–2 s cold start on Windows) and used to ride in the
  // dialog's Promise.all, so the whole form sat behind
  // "Loading settings..." on every open. The form must render
  // as soon as the five DB fetches resolve, and the Video
  // section fills in when the version arrives.
  test('the form renders while the version is still pending', async () => {
    const pending = deferred<YtdlpStatus>()
    vi.mocked(api.ytdlpVersionCached).mockReturnValue(pending.promise)
    renderDialog()

    await waitFor(
      () => expect(screen.getByLabelText('Default download folder')).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(screen.queryByText('Loading settings…')).toBeNull()
    // The Video section shows its pre-existing "unknown" state
    // until the version arrives — it never blocks the form.
    expect(screen.getByText('Video component status is unknown.')).toBeTruthy()
  })

  test('the video section fills in when the version arrives', async () => {
    const pending = deferred<YtdlpStatus>()
    vi.mocked(api.ytdlpVersionCached).mockReturnValue(pending.promise)
    renderDialog()

    await waitFor(
      () => expect(screen.getByLabelText('Default download folder')).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(screen.getByText('Video component status is unknown.')).toBeTruthy()

    pending.resolve(ytdlpStatus('2026.10.26'))
    await waitFor(
      () => expect(screen.getByText(/yt-dlp 2026\.10\.26/)).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(screen.queryByText('Video component status is unknown.')).toBeNull()
  })
})
