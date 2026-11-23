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
  lastUsedFolder?: string
}) {
  return render(
    <AddDownloadDialog
      open
      categories={props.categories ?? BUILTINS}
      initialUrl={props.initialUrl ?? ''}
      autoFileTypes={props.autoFileTypes ?? []}
      captureMode={props.captureMode ?? false}
      captureTitle={props.captureTitle ?? null}
      lastUsedFolder={props.lastUsedFolder ?? ''}
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

describe('last-used folder prefill + separator-loss guard (Round 87.6, items 1–2)', () => {
  const ZIPTEST = 'C:\\Users\\Administrator\\Downloads\\ZipTest'

  test('an empty dialog shows the last-used folder, not the global default', async () => {
    renderDialog({ lastUsedFolder: ZIPTEST })
    // The read-only folder field under Save As shows the picked
    // folder at once — no URL pasted, no category picked yet.
    // (Save As itself shows the same `folder\` while the filename
    // is empty, so the folder field is read by its label.)
    await waitFor(
      () =>
        expect(screen.getByLabelText('Save folder') as HTMLInputElement).toBeTruthy(),
      { timeout: 5000 },
    )
    expect((screen.getByLabelText('Save folder') as HTMLInputElement).value).toBe(
      `${ZIPTEST}\\`,
    )
    expect(categorySelect().value).toBe('')
  })

  test('the last-used folder survives a category pick whose own folder is empty', async () => {
    renderDialog({
      initialUrl: 'https://example.com/movie.mp4',
      lastUsedFolder: ZIPTEST,
    })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    // Video's own folder is empty in BUILTINS, so the prefilled
    // folder stands — Save As shows the full picked path.
    await waitFor(
      () =>
        expect((screen.getByLabelText('Save As') as HTMLInputElement).value).toBe(
          `${ZIPTEST}\\movie.mp4`,
        ),
      { timeout: 5000 },
    )
  })

  test('deleting the separator in Save As keeps the folder and the filename apart', async () => {
    const withFolder: CategoryRow[] = BUILTINS.map((c) =>
      c.id === 'cat-video' ? { ...c, default_folder: ZIPTEST } : c,
    )
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    // A rename edit that deletes the `\` between the folder and
    // the filename: the 87.5 retest's malformed row.
    // Round 87.7 — the field holds a draft until blur, so the
    // typed text shows exactly as typed (no per-keystroke
    // re-parse), and the commit splits it apart.
    fireEvent.change(saveAs, {
      target: { value: 'C:\\Users\\Administrator\\Downloads\\ZipTestslow a test.mp4' },
    })
    expect(saveAs.value).toBe(
      'C:\\Users\\Administrator\\Downloads\\ZipTestslow a test.mp4',
    )
    fireEvent.blur(saveAs)
    expect(saveAs.value).toBe(`${ZIPTEST}\\slow a test.mp4`)

    fireEvent.click(screen.getByText('Start Download'))

    await waitFor(
      () => expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    // Round 87.7 — a rename is NOT an outright folder pick: the
    // auto-filled folder is not written back onto the category
    // (the 87.6 drift: Video became ZipTest), so the category
    // remember does not fire — and the download goes to the
    // real folder with the real name, with no explicit folder.
    expect(vi.mocked(api.updateCategory)).not.toHaveBeenCalled()
    expect(vi.mocked(api.setSettingField)).not.toHaveBeenCalled()
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({
        folder: ZIPTEST,
        filename: 'slow a test.mp4',
        explicitFolder: null,
      }),
    )
  })
})

describe('Save As draft field (Round 87.7, items 1–2)', () => {
  const ZIPTEST = 'C:\\Users\\Administrator\\Downloads\\ZipTest'
  const withFolder: CategoryRow[] = BUILTINS.map((c) =>
    c.id === 'cat-video' ? { ...c, default_folder: ZIPTEST } : c,
  )

  /** Type `text` into the Save As field one character at a
   *  time, the way a real select-all + type does it — each
   *  keystroke is its own change event. The 87.6 per-keystroke
   *  parse + rejoin mangled this: every keystroke re-prepended
   *  the folder, so the field ended up
   *  `tor\Downloads\ZipC:\Users\...\ZipTest\slow a test.mp4`. */
  function typeCharByChar(input: HTMLInputElement, text: string) {
    for (let i = 1; i <= text.length; i++) {
      fireEvent.change(input, { target: { value: text.slice(0, i) } })
    }
  }

  test('select-all + type a full path replaces cleanly, keystroke by keystroke', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    const typed = `${ZIPTEST}\\slow a test.mp4`
    typeCharByChar(saveAs, typed)
    // The field shows exactly what was typed at every step —
    // the folder is never re-prepended mid-typing.
    expect(saveAs.value).toBe(typed)
    fireEvent.blur(saveAs)
    expect(saveAs.value).toBe(typed)

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(
      () => expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    // Same folder as before, so no explicit pick — and no
    // category write for a retype of the same folder.
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({
        folder: ZIPTEST,
        filename: 'slow a test.mp4',
        explicitFolder: null,
      }),
    )
    expect(vi.mocked(api.updateCategory)).not.toHaveBeenCalled()
  })

  test('typing a path in a DIFFERENT folder is an outright pick and is remembered', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    const typed = 'D:\\Picked\\movie.mp4'
    typeCharByChar(saveAs, typed)
    expect(saveAs.value).toBe(typed)
    fireEvent.blur(saveAs)
    expect(saveAs.value).toBe(typed)

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(
      () => expect(vi.mocked(api.updateCategory)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.updateCategory)).toHaveBeenCalledWith({
      id: 'cat-video',
      folder: 'D:\\Picked',
    })
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({
        folder: 'D:\\Picked',
        filename: 'movie.mp4',
        explicitFolder: 'D:\\Picked',
      }),
    )
  })

  test('a bare rename keeps the folder and remembers nothing', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    typeCharByChar(saveAs, 'renamed clip.mp4')
    expect(saveAs.value).toBe('renamed clip.mp4')
    fireEvent.blur(saveAs)
    expect(saveAs.value).toBe(`${ZIPTEST}\\renamed clip.mp4`)

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(
      () => expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({
        folder: ZIPTEST,
        filename: 'renamed clip.mp4',
        explicitFolder: null,
      }),
    )
    expect(vi.mocked(api.updateCategory)).not.toHaveBeenCalled()
    expect(vi.mocked(api.setSettingField)).not.toHaveBeenCalled()
  })

  test('submit straight after typing commits the draft without a blur', async () => {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    // No blur: type the new name and hit Start Download at once.
    typeCharByChar(saveAs, 'D:\\Picked\\final name.mp4')
    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(
      () => expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.addDownload)).toHaveBeenCalledWith(
      expect.objectContaining({
        folder: 'D:\\Picked',
        filename: 'final name.mp4',
        explicitFolder: 'D:\\Picked',
      }),
    )
  })
})

describe('Save As commit on mousedown (Round 87.8)', () => {
  const ZIPTEST = 'C:\\Users\\Administrator\\Downloads\\ZipTest'
  const withFolder: CategoryRow[] = BUILTINS.map((c) =>
    c.id === 'cat-video' ? { ...c, default_folder: ZIPTEST } : c,
  )

  /** Open the dialog on a video in ZipTest, then make the
   *  87.7-retest rename edit: delete the `\` between the folder
   *  and the filename, so the field holds the merged draft. */
  async function openDialogWithMergedDraft(): Promise<HTMLInputElement> {
    renderDialog({ initialUrl: 'https://example.com/movie.mp4', categories: withFolder })
    await waitFor(() => expect(categorySelect().value).toBe('cat-video'), { timeout: 5000 })
    const saveAs = screen.getByLabelText('Save As') as HTMLInputElement
    await waitFor(() => expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`), { timeout: 5000 })

    fireEvent.change(saveAs, {
      target: { value: 'C:\\Users\\Administrator\\Downloads\\ZipTestmovie.mp4' },
    })
    expect(saveAs.value).toBe(
      'C:\\Users\\Administrator\\Downloads\\ZipTestmovie.mp4',
    )
    return saveAs
  }

  test('a mousedown on Description commits the draft — no blur needed', async () => {
    const saveAs = await openDialogWithMergedDraft()

    // Round 87.8 — the live failure: the click into
    // Description never delivered a blur (RDP canvas), so the
    // draft stayed merged. A mousedown on the Description
    // field commits the draft even with no blur at all — the
    // test fires mouseDown only, deliberately NOT blur.
    fireEvent.mouseDown(screen.getByLabelText('Description'))
    expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`)
  })

  test('a mousedown inside the Save As field itself does NOT commit', async () => {
    const saveAs = await openDialogWithMergedDraft()

    fireEvent.mouseDown(saveAs)
    expect(saveAs.value).toBe(
      'C:\\Users\\Administrator\\Downloads\\ZipTestmovie.mp4',
    )

    // The blur path still works afterwards, as before.
    fireEvent.blur(saveAs)
    expect(saveAs.value).toBe(`${ZIPTEST}\\movie.mp4`)
  })
})
