// Round 87.5 (D2) — ONE settings write model, no draft.
//
// Round 87.3 made only the "Download complete" dialog toggle
// write at once; every other field was still staged in a draft
// and written only on "Save settings", so a flip closed with X
// / Cancel / Escape was silently lost — and a Save after an
// immediate write from ANOTHER window (the completion dialog's
// "Don't show this dialog again") wrote the stale draft over
// it. Every Settings-row field now writes through at once
// (`saveField`: fresh re-read + updateSettings, serialized):
// toggles, radios and selects on change, text and number
// fields on blur / Enter. "Save settings" stores only the
// categories, proxy, site logins and sounds — it never writes
// the Settings row again.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { SettingsDialog } from './SettingsDialog'
import * as api from '../lib/api'
import type { CategoryRow, Settings } from '../lib/types'

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

function baseSettings(): Settings {
  return {
    default_download_folder: 'C:\\Downloads',
    start_on_startup: false,
    browser_integration: true,
    platform_downloads_enabled: true,
    browser_start_drained: true,
    clipboard_monitoring: true,
    relay_enabled: true,
    show_progress_window: true,
    show_download_complete_dialog: true,
    excluded_sites: [],
    check_updates_weekly: false,
    update_url: 'https://updates.fetchline.app/releases/latest.json',
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
    last_used_folder: null,
    antivirus_enabled: false,
    antivirus_command: null,
    antivirus_timeout_secs: 60,
    dialup_connection_name: null,
    dialup_auto_dial: false,
    dialup_hangup_after_completion: false,
    dialup_redial_attempts: 0,
    default_action: 'start_now',
    duplicate_action: 'rename',
    confirm_delete: true,
    sounds_enabled: true,
  } as unknown as Settings
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.getSettings).mockResolvedValue(baseSettings())
  vi.mocked(api.updateSettings).mockImplementation((s: Settings) => Promise.resolve(s))
  vi.mocked(api.getProxyConfig).mockResolvedValue(null)
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

function renderDialog(categories: CategoryRow[] = []) {
  return render(
    <SettingsDialog
      open
      categories={categories}
      onClose={() => {}}
      onSaved={() => {}}
      onCheckUpdates={() => {}}
    />,
  )
}

async function openTab(label: string) {
  renderDialog()
  await waitFor(
    () => expect(screen.getByRole('tab', { name: label })).toBeTruthy(),
    { timeout: 5000 },
  )
  fireEvent.click(screen.getByRole('tab', { name: label }))
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
}

describe('immediate settings writes (Round 87.5)', () => {
  test('flipping the clipboard toggle writes at once, with no Save click', async () => {
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Watch the clipboard for download links',
    ) as HTMLInputElement
    expect(checkbox.checked).toBe(true)

    fireEvent.click(checkbox)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings).mock.calls[0][0].clipboard_monitoring).toBe(false)
    expect(checkbox.checked).toBe(false)
  })

  test('flipping the progress-window toggle writes at once too', async () => {
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Show the download progress window when a download starts',
    ) as HTMLInputElement

    fireEvent.click(checkbox)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings).mock.calls[0][0].show_progress_window).toBe(false)
  })

  test('a failed toggle write reverts the checkbox and shows the error', async () => {
    vi.mocked(api.updateSettings).mockRejectedValue(new Error('Database is locked'))
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Watch the clipboard for download links',
    ) as HTMLInputElement

    fireEvent.click(checkbox)

    await waitFor(
      () => expect(screen.getByRole('alert').textContent).toBe('Database is locked'),
      { timeout: 5000 },
    )
    expect(checkbox.checked).toBe(true)
  })

  test('a radio change writes at once', async () => {
    await openTab('Downloads')
    const radio = (await screen.findByRole('radio', {
      name: /Save for later/,
    })) as HTMLInputElement

    fireEvent.click(radio)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings).mock.calls[0][0].default_action).toBe('save_for_later')
  })

  test('typing in a text field does not write until blur, then writes once', async () => {
    await openGeneralTab()
    const input = screen.getByLabelText('Default download folder') as HTMLInputElement

    fireEvent.change(input, { target: { value: 'D:\\New' } })
    expect(vi.mocked(api.updateSettings)).not.toHaveBeenCalled()

    fireEvent.blur(input)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings).mock.calls[0][0].default_download_folder).toBe('D:\\New')
  })

  test('"Save settings" never writes the Settings row again', async () => {
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Watch the clipboard for download links',
    ) as HTMLInputElement
    fireEvent.click(checkbox)
    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )

    // No categories, no proxy, no logins, no sounds staged —
    // the Save has nothing of its own to store, and must not
    // re-write the Settings row (the stale-draft clobber).
    fireEvent.click(screen.getByText('Save settings'))
    await waitFor(
      () => expect(screen.queryByText('Save settings')).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1)
  })

  test('an out-of-range number reverts on blur and shows the error', async () => {
    await openTab('Connection')
    const input = (await screen.findByLabelText(/Connection timeout/)) as HTMLInputElement
    expect(input.value).toBe('30')

    fireEvent.change(input, { target: { value: '0' } })
    fireEvent.blur(input)

    await waitFor(
      () => expect(screen.getByRole('alert').textContent).toBe(
        'The connection timeout must be between 1 and 300 seconds.',
      ),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings)).not.toHaveBeenCalled()
    expect(input.value).toBe('30')
  })

  test('a categories refresh while open does not reset the tab', async () => {
    const { rerender } = renderDialog()
    await waitFor(
      () => expect(screen.getByRole('tab', { name: 'Save To' })).toBeTruthy(),
      { timeout: 5000 },
    )
    fireEvent.click(screen.getByRole('tab', { name: 'Save To' }))
    expect(screen.getByRole('tab', { name: 'Save To' }).getAttribute('aria-selected')).toBe(
      'true',
    )

    // The main window refreshed its categories (a download was
    // added elsewhere): the dialog must not reload, reset its
    // tab, or drop in-progress edits.
    const video: CategoryRow = {
      id: 'cat-video',
      name: 'Video',
      default_folder: '',
      file_extensions: ['mp4'],
      is_builtin: true,
    }
    rerender(
      <SettingsDialog
        open
        categories={[video]}
        onClose={() => {}}
        onSaved={() => {}}
        onCheckUpdates={() => {}}
      />,
    )
    expect(screen.getByRole('tab', { name: 'Save To' }).getAttribute('aria-selected')).toBe(
      'true',
    )
    // …and the new category's folder field is there.
    expect(screen.getByLabelText('Video')).toBeTruthy()
  })
})

describe('clipboard checkbox placement (Round 87.8)', () => {
  test('the clipboard checkbox sits right under "Catch downloads from the browser"', async () => {
    await openGeneralTab()
    const clipboard = screen.getByLabelText(
      'Watch the clipboard for download links',
    ) as HTMLInputElement
    expect(clipboard.checked).toBe(true)

    // Round 87.8 — the checkbox was 7th down a long General
    // tab, below the fold, and two VPS retests missed it. It
    // now sits directly under the browser-catch section: the
    // next checkbox label after it in document order is the
    // clipboard one, with no other checkbox label between.
    const checkboxLabels = Array.from(document.querySelectorAll('label')).filter((label) =>
      label.querySelector('input[type="checkbox"]'),
    )
    const catchIdx = checkboxLabels.findIndex((label) =>
      label.textContent?.includes('Catch downloads from the browser'),
    )
    const clipboardIdx = checkboxLabels.findIndex((label) =>
      label.textContent?.includes('Watch the clipboard for download links'),
    )
    expect(catchIdx).toBeGreaterThanOrEqual(0)
    expect(clipboardIdx).toBe(catchIdx + 1)
  })
})

describe('relay master switch (Round 88)', () => {
  test('the relay checkbox renders, checked by default, right after the clipboard checkbox', async () => {
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Catch videos and files playing in the browser (relay)',
    ) as HTMLInputElement
    expect(checkbox.checked).toBe(true)

    // Round 88 — the switch lives in the browser-catch cluster
    // at the top of General, directly under the clipboard
    // checkbox (the 87.8 test pins the clipboard checkbox as
    // the one right after "Catch downloads from the browser",
    // so the relay switch goes after it, not between).
    const checkboxLabels = Array.from(document.querySelectorAll('label')).filter((label) =>
      label.querySelector('input[type="checkbox"]'),
    )
    const clipboardIdx = checkboxLabels.findIndex((label) =>
      label.textContent?.includes('Watch the clipboard for download links'),
    )
    const relayIdx = checkboxLabels.findIndex((label) =>
      label.textContent?.includes('Catch videos and files playing in the browser (relay)'),
    )
    expect(clipboardIdx).toBeGreaterThanOrEqual(0)
    expect(relayIdx).toBe(clipboardIdx + 1)
  })

  test('flipping the relay toggle writes at once, with no Save click', async () => {
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Catch videos and files playing in the browser (relay)',
    ) as HTMLInputElement

    fireEvent.click(checkbox)

    await waitFor(
      () => expect(vi.mocked(api.updateSettings)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateSettings).mock.calls[0][0].relay_enabled).toBe(false)
    expect(checkbox.checked).toBe(false)
  })

  test('the relay toggle is disabled while catching from the browser is off', async () => {
    vi.mocked(api.getSettings).mockResolvedValue({
      ...baseSettings(),
      browser_integration: false,
    })
    await openGeneralTab()
    const checkbox = screen.getByLabelText(
      'Catch videos and files playing in the browser (relay)',
    ) as HTMLInputElement
    expect(checkbox.disabled).toBe(true)
  })
})
