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
import { AddDownloadDialog, bestQualityRow, isVideoPageUrl } from './AddDownloadDialog'
import * as api from '../lib/api'
import type { ProbeInfo, QualityRow, YtdlpResolveResult } from '../lib/types'

vi.mock('../lib/api', () => ({
  addDownload: vi.fn(),
  addResolvedVideoDownload: vi.fn(),
  confirmPendingLink: vi.fn(),
  probeUrl: vi.fn(),
  setDownloadDescription: vi.fn(),
  setDownloadSpeedLimit: vi.fn(),
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
