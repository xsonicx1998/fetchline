// Round 87.5 (E4 + E5) — the Add URL dialog auto-picks its
// category from the file type, and remembers a folder the user
// picks.
//
// E4: the category select used to start empty on every open
// ("Pick automatically from the file type") and never
// pre-selected, even though the engine auto-assigns on submit
// — the user could never see or confirm the category. It now
// pre-selects the category matching the file extension (the
// engine's rule, `categoryForExtension`), from the URL at once
// and again when the probe / resolve fills in the real
// filename; a hand pick always wins afterwards.
//
// E5: a folder the user picks (the "..." picker or a folder
// typed into Save As) is remembered on submit — the category's
// Save To, or the global default folder when no category is
// set — so the next dialog shows it. A folder the dialog
// filled in by itself (a category's default) is not a pick and
// is left alone. The old "Remember this path" checkbox only
// worked with a hand-picked category, so in practice the path
// was never remembered.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AddDownloadDialog } from './AddDownloadDialog'
import * as api from '../lib/api'
import type { CategoryRow, ProbeInfo, Settings } from '../lib/types'

vi.mock('../lib/api', () => ({
  addDownload: vi.fn(),
  addResolvedVideoDownload: vi.fn(),
  confirmPendingLink: vi.fn(),
  probeUrl: vi.fn(),
  setDownloadDescription: vi.fn(),
  setDownloadSpeedLimit: vi.fn(),
  setSettingField: vi.fn(),
  startDownload: vi.fn(),
  updateCategory: vi.fn(),
  ytdlpResolve: vi.fn(),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

const BUILTINS: CategoryRow[] = [
  { id: 'cat-music', name: 'Music', default_folder: '', file_extensions: ['mp3', 'flac'], is_builtin: true },
  { id: 'cat-video', name: 'Video', default_folder: '', file_extensions: ['mp4', 'mkv'], is_builtin: true },
  { id: 'cat-documents', name: 'Documents', default_folder: '', file_extensions: ['pdf', 'txt'], is_builtin: true },
  { id: 'cat-programs', name: 'Programs', default_folder: '', file_extensions: ['exe', 'msi'], is_builtin: true },
  { id: 'cat-compressed', name: 'Compressed', default_folder: '', file_extensions: ['zip', 'rar'], is_builtin: true },
]

function probeFor(filename: string): ProbeInfo {
  return {
    size: 12345,
    accept_ranges: true,
    etag: null,
    last_modified: null,
    filename,
    content_type: 'application/octet-stream',
    final_url: `https://example.com/${filename}`,
  }
}

function renderDialog(props: {
  initialUrl?: string
  categories?: CategoryRow[]
  autoFileTypes?: string[]
  captureTitle?: string | null
  captureMode?: boolean
}) {
  return render(
    <AddDownloadDialog
      open
      categories={props.categories ?? BUILTINS}
      initialUrl={props.initialUrl ?? ''}
      autoFileTypes={props.autoFileTypes ?? []}
      captureMode={props.captureMode ?? false}
      captureTitle={props.captureTitle ?? null}
      onClose={() => {}}
      onAdded={() => {}}
      onLateError={() => {}}
      onSubmitError={() => {}}
    />,
  )
}

function categorySelect(): HTMLSelectElement {
  return screen.getByLabelText('Category') as HTMLSelectElement
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.addDownload).mockResolvedValue(['dl-1', probeFor('file.zip')])
  vi.mocked(api.startDownload).mockResolvedValue(undefined)
  vi.mocked(api.updateCategory).mockResolvedValue(BUILTINS[1])
  vi.mocked(api.setSettingField).mockResolvedValue({} as unknown as Settings)
  vi.mocked(api.probeUrl).mockResolvedValue(probeFor('movie.mp4'))
})

describe('category auto-pick (Round 87.5, E4)', () => {
  test('an mp4 URL pre-selects Video at once, before the probe answers', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
  })

  test('matching is case-insensitive (MP3 → Music)', async () => {
    renderDialog({ initialUrl: 'https://example.com/SONG.MP3' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-music'), { timeout: 5000 })
  })

  test('an unknown extension falls back to Documents', async () => {
    renderDialog({ initialUrl: 'https://example.com/data.xyz' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-documents'), { timeout: 5000 })
  })

  test('a URL with no extension leaves the select alone, even after the probe', async () => {
    // The probe answers a filename with no extension either,
    // so there is never an extension to pick from.
    vi.mocked(api.probeUrl).mockResolvedValue(probeFor('download'))
    renderDialog({ initialUrl: 'https://example.com/download' })
    // The probe's size renders in cyan once it answers.
    await waitFor(() => expect(screen.getByText('12.1 KB')).toBeTruthy(), { timeout: 5000 })
    expect(categorySelect().value).toBe('')
  })

  test('file_types_auto restricts the pick to Documents', async () => {
    renderDialog({
      initialUrl: 'https://example.com/song.mp3',
      autoFileTypes: ['mp4'],
    })
    await waitFor(() => expect(categorySelect().value).toBe('cat-documents'), { timeout: 5000 })
  })

  test('a capture with a known filename auto-picks from it', async () => {
    renderDialog({ captureMode: true, captureTitle: 'song.mp3' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-music'), { timeout: 5000 })
  })

  test('a hand pick wins over a later filename change', async () => {
    // The probe is held back: it answers "movie.mp4" only
    // after the user has already picked by hand.
    let resolveProbe!: (info: ProbeInfo) => void
    vi.mocked(api.probeUrl).mockImplementation(
      () => new Promise<ProbeInfo>((resolve) => { resolveProbe = resolve }),
    )
    renderDialog({ initialUrl: 'https://example.com/movie.mp4' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })

    fireEvent.change(categorySelect(), { target: { value: 'cat-documents' } })
    expect(categorySelect().value).toBe('cat-documents')

    // Wait for the probe to actually be issued (500 ms
    // debounce) before answering it.
    await waitFor(() => expect(vi.mocked(api.probeUrl)).toHaveBeenCalled(), { timeout: 5000 })
    resolveProbe(probeFor('movie.mp4'))
    // The filename lands, but the hand pick stands — the
    // auto-pick must not yank the category back to Video.
    await waitFor(
      () =>
        expect((screen.getByLabelText('Save As') as HTMLInputElement).value).toBe('movie.mp4'),
      { timeout: 5000 },
    )
    expect(categorySelect().value).toBe('cat-documents')
  })
})

describe('remembering a picked folder (Round 87.5, E5)', () => {
  test('a folder typed into Save As is remembered on the auto-picked category', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4' })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })

    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    fireEvent.change(saveAs, { target: { value: 'D:\\Picked\\movie.mp4' } })
    expect(saveAs.value).toBe('D:\\Picked\\movie.mp4')

    fireEvent.click(screen.getByText('Start Download'))

    await waitFor(
      () => expect(vi.mocked(api.updateCategory)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateCategory)).toHaveBeenCalledWith({
      id: 'cat-video',
      folder: 'D:\\Picked',
    })
    expect(vi.mocked(api.setSettingField)).not.toHaveBeenCalled()
    // The download itself still goes through with the picked folder.
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({ folder: 'D:\\Picked', category: 'cat-video' }),
    )
  })

  test('with no category, a picked folder is remembered as the global default', async () => {
    renderDialog({ initialUrl: 'https://example.com/file.zip', categories: [] })
    expect(categorySelect().value).toBe('')

    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    fireEvent.change(saveAs, { target: { value: 'D:\\Picked\\file.zip' } })

    fireEvent.click(screen.getByText('Start Download'))

    await waitFor(
      () => expect(vi.mocked(api.setSettingField)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.setSettingField)).toHaveBeenCalledWith(
      'default_download_folder',
      'D:\\Picked',
    )
    expect(vi.mocked(api.updateCategory)).not.toHaveBeenCalled()
  })

  test('a folder the dialog filled in itself is NOT remembered', async () => {
    const withFolder: CategoryRow[] = BUILTINS.map((c) =>
      c.id === 'cat-video' ? { ...c, default_folder: 'C:\\Videos' } : c,
    )
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    // The category autofill put the folder in on its own.
    await waitFor(
      () =>
        expect((screen.getByLabelText('Save As') as HTMLInputElement).value).toBe(
          'C:\\Videos\\movie.mp4',
        ),
      { timeout: 5000 },
    )

    fireEvent.click(screen.getByText('Start Download'))

    await waitFor(
      () => expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateCategory)).not.toHaveBeenCalled()
    expect(vi.mocked(api.setSettingField)).not.toHaveBeenCalled()
  })
})
