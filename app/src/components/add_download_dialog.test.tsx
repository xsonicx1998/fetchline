// Round 79 — manual Add URL with a video URL.
//
// The Round 78 retest: the dialog resolved the metadata (title +
// "1080p + 128kbps — merged") but the download used the raw watch
// URL — the list item was "watch", 0 B, Error, "Server ignored
// the range request". The dialog must resolve a platform video
// page and create the download from the RESOLVED stream URLs
// (never the page URL), and a resolve failure must show the real
// error and create NO download at all. Non-video URLs keep the
// plain probe + add path unchanged.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AddDownloadDialog, applySaveAsEdit, bestQualityRow, isHttpUrl, isVideoPageUrl, normalizeUrlScheme } from './AddDownloadDialog'
import * as api from '../lib/api'
import type { ProbeInfo, QualityRow, YtdlpResolveResult } from '../lib/types'

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

const WATCH_URL = 'https://www.youtube.com/watch?v=dQw4w9WgXcQ'

function row(over: Partial<QualityRow>): QualityRow {
  return {
    height: 360,
    label: '360p',
    ext: 'mp4',
    has_audio: true,
    needs_merge: false,
    filesize: 1_000_000,
    url: 'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=18',
    audio_url: null,
    audio_format_id: null,
    format_id: '18',
    headers: {},
    fragmented: false,
    ...over,
  }
}

function resolveResult(): YtdlpResolveResult {
  return {
    title: 'Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)',
    is_live: false,
    degraded: false,
    warning: null,
    formats: [
      row({}),
      row({
        height: 1080,
        label: '1080p',
        ext: 'mp4',
        has_audio: false,
        needs_merge: true,
        filesize: 50_000_000,
        url: 'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=137',
        audio_url: 'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=140',
        audio_format_id: '140',
        format_id: '137',
      }),
    ],
  }
}

const probeInfo: ProbeInfo = {
  size: 50_000_000,
  accept_ranges: true,
  etag: null,
  last_modified: null,
  filename: 'Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster).mp4',
  content_type: 'video/mp4',
  final_url: 'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=137',
}

function renderDialog(initialUrl: string, onAdded = vi.fn(), onSubmitError = vi.fn()) {
  return {
    onAdded,
    onSubmitError,
    ...render(
      <AddDownloadDialog
        open
        categories={[]}
        initialUrl={initialUrl}
        onClose={() => {}}
        onAdded={onAdded}
        onLateError={() => {}}
        onSubmitError={onSubmitError}
      />,
    ),
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.ytdlpResolve).mockResolvedValue(resolveResult())
  vi.mocked(api.addResolvedVideoDownload).mockResolvedValue(['dl-1', probeInfo])
  vi.mocked(api.addDownload).mockResolvedValue(['dl-2', probeInfo])
  vi.mocked(api.probeUrl).mockResolvedValue({
    size: 12345,
    accept_ranges: true,
    etag: null,
    last_modified: null,
    filename: 'file.zip',
    content_type: 'application/zip',
    final_url: 'https://example.com/file.zip',
  })
})

// -- routing: which URLs are platform video pages -------------------------

describe('isVideoPageUrl', () => {
  test('YouTube watch / shorts / youtu.be are video pages', () => {
    expect(isVideoPageUrl(WATCH_URL)).toBe(true)
    expect(isVideoPageUrl('https://youtu.be/dQw4w9WgXcQ')).toBe(true)
    expect(isVideoPageUrl('https://www.youtube.com/shorts/abc123')).toBe(true)
    expect(isVideoPageUrl('https://m.youtube.com/watch?v=dQw4w9WgXcQ')).toBe(true)
  })

  test('TikTok / Instagram / Facebook are video pages', () => {
    expect(isVideoPageUrl('https://www.tiktok.com/@user/video/123')).toBe(true)
    expect(isVideoPageUrl('https://www.instagram.com/reel/abc/')).toBe(true)
    expect(isVideoPageUrl('https://www.facebook.com/watch/?v=123')).toBe(true)
    expect(isVideoPageUrl('https://fb.watch/abc123/')).toBe(true)
  })

  test('channel pages, plain files and non-http URLs are not', () => {
    expect(isVideoPageUrl('https://www.youtube.com/@RickAstley')).toBe(false)
    expect(isVideoPageUrl('https://www.youtube.com/')).toBe(false)
    expect(isVideoPageUrl('https://example.com/file.zip')).toBe(false)
    expect(isVideoPageUrl('ftp://example.com/file.zip')).toBe(false)
    expect(isVideoPageUrl('not a url')).toBe(false)
  })
})

describe('bestQualityRow', () => {
  test('picks the highest row (rows are ascending)', () => {
    const best = bestQualityRow(resolveResult())
    expect(best?.format_id).toBe('137')
    expect(best?.label).toBe('1080p')
  })

  test('empty formats yield no row', () => {
    expect(bestQualityRow({ ...resolveResult(), formats: [] })).toBeNull()
    expect(bestQualityRow(null)).toBeNull()
  })
})

// -- Round 80 — URL validation --------------------------------------------
// The Round 79 retest rejected the valid Instagram reel URL
// https://www.instagram.com/reel/DdwixbhzY3_/ ("Enter a link
// starting with http:// or https://"). The rule itself was
// already correct — these URLs pass it — so the tests lock the
// rule in with the exact retest URLs, and cover the `https;//`
// scheme typo an unreliable Shift key produces when a URL is
// typed through RDP (the typed text had `;` for `:`).

describe('isHttpUrl', () => {
  test('accepts the exact Round 79 retest URLs', () => {
    expect(isHttpUrl('https://www.instagram.com/reel/DdwixbhzY3_/')).toBe(true)
    expect(isHttpUrl('https://www.instagram.com/reel/DdwixbhzY3_')).toBe(true)
    expect(isHttpUrl('https://www.tiktok.com/@sabrinacarpenter/video/7551233954129399053')).toBe(true)
    expect(isHttpUrl('https://www.facebook.com/watch/')).toBe(true)
    expect(isHttpUrl('http://127.0.0.1:55443/small.zip')).toBe(true)
  })

  test('rejects non-http URLs and the semicolon scheme typo', () => {
    expect(isHttpUrl('https;//www.instagram.com/reel/DdwixbhzY3_/')).toBe(false)
    expect(isHttpUrl('ftp://example.com/file.zip')).toBe(false)
    expect(isHttpUrl('not a url')).toBe(false)
    expect(isHttpUrl('')).toBe(false)
  })
})

describe('normalizeUrlScheme', () => {
  test('repairs only the semicolon-for-colon scheme typo', () => {
    expect(normalizeUrlScheme('https;//www.instagram.com/reel/DdwixbhzY3_/')).toBe(
      'https://www.instagram.com/reel/DdwixbhzY3_/',
    )
    expect(normalizeUrlScheme('http;//example.com/file.zip')).toBe('http://example.com/file.zip')
  })

  test('leaves good URLs untouched', () => {
    expect(normalizeUrlScheme('https://www.instagram.com/reel/DdwixbhzY3_/')).toBe(
      'https://www.instagram.com/reel/DdwixbhzY3_/',
    )
    expect(normalizeUrlScheme('https://example.com/file.zip')).toBe('https://example.com/file.zip')
    expect(normalizeUrlScheme('')).toBe('')
  })
})

// -- submit: the resolved stream URLs, never the page URL ------------------

describe('video URL submit', () => {
  test('resolves the page and adds the best row’s stream URLs', async () => {
    const { onAdded } = renderDialog(WATCH_URL)
    // The debounced resolve (500 ms) fills the filename and the
    // quality line from the best row.
    await waitFor(
      () => expect(screen.getByText('1080p · MP4 · merged')).toBeTruthy(),
      { timeout: 5000 },
    )
    expect(vi.mocked(api.ytdlpResolve)).toHaveBeenCalledWith(WATCH_URL)
    expect(vi.mocked(api.probeUrl)).not.toHaveBeenCalled()

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith('dl-1'), { timeout: 5000 })

    expect(vi.mocked(api.addResolvedVideoDownload)).toHaveBeenCalledTimes(1)
    const call = vi.mocked(api.addResolvedVideoDownload).mock.calls[0][0]
    expect(call.pageUrl).toBe(WATCH_URL)
    // The best (highest) row — the DASH video stream, not the
    // watch-page URL and not the 360p row.
    expect(call.row.format_id).toBe('137')
    expect(call.row.url).toBe(
      'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=137',
    )
    expect(call.row.audio_url).toBe(
      'https://rr1---sn.googlevideo.com/videoplayback?expire=1&id=140',
    )
    expect(call.startNow).toBe(true)
    // The plain path is never used for a video page, and the
    // engine (not the dialog) starts the download.
    expect(vi.mocked(api.addDownload)).not.toHaveBeenCalled()
    expect(vi.mocked(api.startDownload)).not.toHaveBeenCalled()
  })

  test('a resolve failure shows the real error and creates no download', async () => {
    const loginError = new Error(
      'Sign in to confirm you’re not a bot — this video needs a login. Log in to YouTube in your browser and try again.',
    )
    vi.mocked(api.ytdlpResolve).mockRejectedValue(loginError)
    const { onAdded, onSubmitError } = renderDialog(WATCH_URL)

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(() => expect(onSubmitError).toHaveBeenCalled(), { timeout: 5000 })

    expect(onSubmitError.mock.calls[0][0]).toBe(loginError.message)
    // The error is shown in the dialog too (the pink alert).
    expect(screen.getByRole('alert').textContent).toContain('not a bot')
    // No download of any kind was created — in particular no
    // size-0 download of the watch-page URL.
    expect(vi.mocked(api.addResolvedVideoDownload)).not.toHaveBeenCalled()
    expect(vi.mocked(api.addDownload)).not.toHaveBeenCalled()
    expect(onAdded).not.toHaveBeenCalled()
  })

  test('a live stream is refused with no download', async () => {
    vi.mocked(api.ytdlpResolve).mockResolvedValue({
      ...resolveResult(),
      is_live: true,
    })
    const { onAdded, onSubmitError } = renderDialog(WATCH_URL)

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(() => expect(onSubmitError).toHaveBeenCalled(), { timeout: 5000 })

    expect(onSubmitError.mock.calls[0][0]).toBe("Live streams can't be downloaded")
    expect(vi.mocked(api.addResolvedVideoDownload)).not.toHaveBeenCalled()
    expect(vi.mocked(api.addDownload)).not.toHaveBeenCalled()
    expect(onAdded).not.toHaveBeenCalled()
  })
})

// -- Round 87 — Facebook reel with height-less formats ----------------------
// The live `-J` for facebook.com/reel/2141490136612275/ lists `sd` /
// `hd` with no height and no codecs; the engine now turns them
// into rows (height 0, label = the format's own name). The
// dialog must show the reel's best row instead of "no video",
// and must tell a "formats found but unusable" zero apart from
// a feed-page zero (the engine says so in `warning`).
describe('Facebook reel resolve', () => {
  const REEL_URL = 'https://www.facebook.com/reel/2141490136612275/'

  test('a reel with height-less formats resolves and shows its best row', async () => {
    vi.mocked(api.ytdlpResolve).mockResolvedValue({
      title: 'Reel video',
      is_live: false,
      degraded: false,
      warning: null,
      formats: [
        row({
          height: 0,
          label: 'sd',
          ext: 'mp4',
          has_audio: false,
          needs_merge: false,
          filesize: null,
          url: 'https://video-ord5-2.xx.fbcdn.net/sd.mp4',
          format_id: 'sd',
        }),
        row({
          height: 0,
          label: 'hd',
          ext: 'mp4',
          has_audio: false,
          needs_merge: false,
          filesize: null,
          url: 'https://video-ord5-1.xx.fbcdn.net/hd.mp4',
          format_id: 'hd',
        }),
      ],
    })
    renderDialog(REEL_URL)
    // The debounced resolve (500 ms) fills the quality line
    // from the best row — the reel's `hd`, not a feed message.
    await waitFor(() => expect(screen.getByText('hd · MP4')).toBeTruthy(), {
      timeout: 5000,
    })
    expect(vi.mocked(api.ytdlpResolve)).toHaveBeenCalledWith(REEL_URL)
    expect(vi.mocked(api.probeUrl)).not.toHaveBeenCalled()
  })

  test('submitting the reel sends the height-0 best row’s format_id to the download start', async () => {
    // Round 87.1 (item 6, TRACE FIRST) — the R87 parser
    // synthesizes quality rows with height 0 for codec-less
    // progressive MP4s. The trace proves selection is by
    // format_id, never by height: the dialog passes the whole
    // row to `addResolvedVideoDownload` (see the video-URL
    // submit test), the extension's pick carries `format_id`
    // (video-detect.js), and the engine's
    // `add_resolved_video_download` uses `row.format_id` /
    // `row.url` directly — `select_quality` (height-keyed) is
    // only called inside `quality_rows` for height-keyed rows,
    // and the height-0 fallback rows bypass it entirely. This
    // test pins the dialog half: submitting a reel whose rows
    // are BOTH height 0 must start the download with the `hd`
    // row's format_id, not a height lookup.
    vi.mocked(api.ytdlpResolve).mockResolvedValue({
      title: 'Reel video',
      is_live: false,
      degraded: false,
      warning: null,
      formats: [
        row({
          height: 0,
          label: 'sd',
          ext: 'mp4',
          has_audio: false,
          needs_merge: false,
          filesize: null,
          url: 'https://video-ord5-2.xx.fbcdn.net/sd.mp4',
          format_id: 'sd',
        }),
        row({
          height: 0,
          label: 'hd',
          ext: 'mp4',
          has_audio: false,
          needs_merge: false,
          filesize: null,
          url: 'https://video-ord5-1.xx.fbcdn.net/hd.mp4',
          format_id: 'hd',
        }),
      ],
    })
    const { onAdded } = renderDialog(REEL_URL)
    await waitFor(() => expect(screen.getByText('hd · MP4')).toBeTruthy(), {
      timeout: 5000,
    })

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith('dl-1'), { timeout: 5000 })

    expect(vi.mocked(api.addResolvedVideoDownload)).toHaveBeenCalledTimes(1)
    const call = vi.mocked(api.addResolvedVideoDownload).mock.calls[0][0]
    // The height-0 row is selected by its format_id, not by a
    // height that does not exist.
    expect(call.row.height).toBe(0)
    expect(call.row.format_id).toBe('hd')
    expect(call.row.url).toBe('https://video-ord5-1.xx.fbcdn.net/hd.mp4')
    expect(call.row.label).toBe('hd')
  })

  test('zero formats WITH a warning says the video was found but unusable', async () => {
    vi.mocked(api.ytdlpResolve).mockResolvedValue({
      ...resolveResult(),
      formats: [],
      warning:
        'resolve returned no usable formats: yt-dlp listed 1 format(s), kept 1, quality rows 0; skipped: no URL 0, storyboard 0; protocols: https×1; top-level url: no; 12345ms',
    })
    renderDialog(REEL_URL)
    await waitFor(
      () =>
        expect(
          screen.getByText(
            'This video was found, but none of its formats could be downloaded.',
          ),
        ).toBeTruthy(),
      { timeout: 5000 },
    )
  })

  test('zero formats WITHOUT a warning still says no video was found', async () => {
    vi.mocked(api.ytdlpResolve).mockResolvedValue({
      ...resolveResult(),
      formats: [],
      warning: null,
    })
    renderDialog(REEL_URL)
    await waitFor(
      () =>
        expect(
          screen.getByText(
            'No downloadable video was found on this page — it may be a feed or a page, not a video.',
          ),
        ).toBeTruthy(),
      { timeout: 5000 },
    )
  })
})

// -- submit: non-video URLs keep the plain path ----------------------------

describe('non-video URL submit', () => {
  test('a plain file link probes and adds normally, never resolves', async () => {
    const { onAdded } = renderDialog('https://example.com/file.zip')
    await waitFor(() => expect(vi.mocked(api.probeUrl)).toHaveBeenCalled(), {
      timeout: 5000,
    })
    expect(vi.mocked(api.ytdlpResolve)).not.toHaveBeenCalled()

    fireEvent.click(screen.getByText('Start Download'))
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith('dl-2'), { timeout: 5000 })

    expect(vi.mocked(api.addDownload)).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.addDownload).mock.calls[0][0].url).toBe(
      'https://example.com/file.zip',
    )
    expect(vi.mocked(api.addResolvedVideoDownload)).not.toHaveBeenCalled()
    expect(vi.mocked(api.startDownload)).toHaveBeenCalledWith('dl-2')
  })
})

// -- Round 81 (ex-Round 77 item a) — double title ---------------------------
// The standalone Add URL window's own TitleBar already shows the
// captured file name, so the dialog's inner header showed it a
// second time, stacked (the user's 2026-11-20 screenshots). In
// standalone mode the dialog must render NO inner header; the
// in-window dialog keeps its own.
describe('standalone header', () => {
  const MOVIE_URL = 'https://s203d1.downet.net/download/1795262560/El%20Kalam.mp4'
  const MOVIE_TITLE = 'El Kalam Ala Eh Awwel Layla.2026.1080p.mp4'

  test('standalone: no inner header, the file name shows once (in the window title bar)', () => {
    render(
      <AddDownloadDialog
        open
        standalone
        captureMode
        categories={[]}
        initialUrl={MOVIE_URL}
        captureTitle={MOVIE_TITLE}
        onClose={() => {}}
        onAdded={() => {}}
        onLateError={() => {}}
      />,
    )
    expect(screen.queryByRole('heading')).toBeNull()
  })

  test('in-window: the header is the only title', () => {
    render(
      <AddDownloadDialog
        open
        captureMode
        categories={[]}
        initialUrl={MOVIE_URL}
        captureTitle={MOVIE_TITLE}
        onClose={() => {}}
        onAdded={() => {}}
        onLateError={() => {}}
      />,
    )
    expect(screen.getByRole('heading').textContent).toBe(MOVIE_TITLE)
  })
})

// -- Round 84 — a resolved capture never probes ---------------------------
// The R83 retest: the dialog probed the direct TikTok CDN URL,
// TikTok answered 403, and Start Download was rejected with
// "already decided (rejected)". yt-dlp already validated the
// stream, so a capture carrying the resolver's headers and/or
// size renders the pre-resolved size and never calls probeUrl
// (and never resolves the page either — the row is already
// resolved). No probe error is shown.
describe('resolved capture probe skip', () => {
  const CDN_URL =
    'https://v16-webapp-prime.us.tiktok.com/video/tos/useast2a/tos-useast2a-ve-0068c003/oYvA0wQwEFuQAA/video.mp4'
  const HEADERS =
    '{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.tiktok.com/"}'

  function renderResolvedCapture(initialFileSize: number | null) {
    return render(
      <AddDownloadDialog
        open
        standalone
        captureMode
        categories={[]}
        initialUrl={CDN_URL}
        pendingLinkId="pl-1"
        initialHeaders={HEADERS}
        initialFileSize={initialFileSize}
        captureTitle="TikTok video.mp4"
        onClose={() => {}}
        onAdded={() => {}}
        onLateError={() => {}}
      />,
    )
  }

  test('renders the pre-resolved size without probing', async () => {
    renderResolvedCapture(11_829_048)
    // The pre-resolved size renders (no 500 ms debounce — the
    // skip runs on mount, not in the timer).
    await waitFor(() => expect(screen.getByText('11.3 MB')).toBeTruthy(), {
      timeout: 5000,
    })
    // Past the debounce: still no probe, no resolve, no error.
    await new Promise((r) => setTimeout(r, 700))
    expect(vi.mocked(api.probeUrl)).not.toHaveBeenCalled()
    expect(vi.mocked(api.ytdlpResolve)).not.toHaveBeenCalled()
    expect(screen.queryByText(/403/)).toBeNull()
  })

  test('no size is tolerated, still without probing', async () => {
    renderResolvedCapture(null)
    // Past the debounce: no probe, no error — "size unknown"
    // is tolerated, not an error.
    await new Promise((r) => setTimeout(r, 700))
    expect(vi.mocked(api.probeUrl)).not.toHaveBeenCalled()
    expect(vi.mocked(api.ytdlpResolve)).not.toHaveBeenCalled()
    expect(screen.queryByText(/403/)).toBeNull()
  })
})

describe('Save As edits against the known folder (Round 87.6, item 1)', () => {
  const FOLDER = 'C:\\Users\\Administrator\\Downloads\\ZipTest'

  test('a deleted separator cannot fold the folder name into the filename', () => {
    // The 87.5 retest's malformed row: the `\` after ZipTest was
    // deleted during a rename edit, and the plain last-separator
    // split read folder `...\Downloads` + filename
    // `ZipTestslow a test.mp4` — which broke Open With (error 31)
    // and the row name.
    expect(
      applySaveAsEdit(FOLDER, 'C:\\Users\\Administrator\\Downloads\\ZipTestslow a test.mp4'),
    ).toEqual({ folder: FOLDER, filename: 'slow a test.mp4' })
  })

  test('a normal full path still splits on the last separator', () => {
    expect(
      applySaveAsEdit(FOLDER, 'C:\\Users\\Administrator\\Downloads\\ZipTest\\slow a test.mp4'),
    ).toEqual({ folder: FOLDER, filename: 'slow a test.mp4' })
  })

  test('renaming the folder itself still works', () => {
    expect(
      applySaveAsEdit(FOLDER, 'C:\\Users\\Administrator\\Downloads\\ZipTest2\\slow a test.mp4'),
    ).toEqual({
      folder: 'C:\\Users\\Administrator\\Downloads\\ZipTest2',
      filename: 'slow a test.mp4',
    })
  })

  test('a bare name is a rename — the folder stays', () => {
    expect(applySaveAsEdit(FOLDER, 'new name.mp4')).toEqual({
      folder: '',
      filename: 'new name.mp4',
    })
  })

  test('with no known folder, a full path splits as before', () => {
    expect(applySaveAsEdit('', 'C:\\Downloads\\file.zip')).toEqual({
      folder: 'C:\\Downloads',
      filename: 'file.zip',
    })
  })
})

// -- Round 88 — the relay source line ------------------------------------
// The Add URL dialog says where a capture came from (plan §6):
// "Caught from the browser (relay)" for a relayed row, "Caught from
// the extension" for a click/button capture. Manual Add URL and
// clipboard captures carry no source and show no line.
describe('capture source line (Round 88)', () => {
  const RELAY_URL = 'https://cdn.example.com/v/movie.mp4'

  function renderCapture(source: string | null) {
    return render(
      <AddDownloadDialog
        open
        standalone
        captureMode
        categories={[]}
        initialUrl={RELAY_URL}
        pendingLinkId="pl-1"
        captureTitle="movie.mp4"
        captureSource={source}
        onClose={() => {}}
        onAdded={() => {}}
        onLateError={() => {}}
      />,
    )
  }

  test('a relay capture shows the relay source line', () => {
    renderCapture('relay')
    expect(screen.getByText('Caught from the browser (relay)')).toBeTruthy()
  })

  test('a browser capture shows the extension source line', () => {
    renderCapture('browser')
    expect(screen.getByText('Caught from the extension')).toBeTruthy()
    expect(screen.queryByText('Caught from the browser (relay)')).toBeNull()
  })

  test('a video-button capture shows the extension source line', () => {
    renderCapture('video')
    expect(screen.getByText('Caught from the extension')).toBeTruthy()
  })

  test('no source means no source line (manual / clipboard)', () => {
    renderCapture(null)
    expect(screen.queryByText(/Caught from/)).toBeNull()
  })
})
