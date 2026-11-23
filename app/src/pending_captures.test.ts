// Round 88 — the pending-capture poll's filter (plan §5 step 4).
//
// The 1 s poll offers a pending row in the Add URL dialog when its
// source is an extension capture ('browser' | 'video') or a relay
// capture ('relay'); 'download_all' rows auto-add via the drain
// and never reach the dialog, and a row already offered is never
// offered again.
//
// Run with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { isDialogCapture } from './App'
import type { PendingLinkRow } from './lib/types'

function row(source: string, id = 'row-1'): PendingLinkRow {
  return {
    id,
    url: 'https://cdn.example.com/v/movie.mp4',
    referrer: null,
    source,
    status: 'pending',
    download_id: null,
    error: null,
    cookies: null,
    created_at: '2026-11-23T00:00:00Z',
    filename: 'movie.mp4',
    headers: null,
    source_url: null,
    format_id: null,
    audio_url: null,
    fragmented: false,
  }
}

describe('pending-capture poll filter (Round 88)', () => {
  test("a 'relay' row is offered in the Add URL dialog", () => {
    expect(isDialogCapture(row('relay'), new Set())).toBe(true)
  })

  test("the old 'browser' and 'video' rows still are", () => {
    expect(isDialogCapture(row('browser'), new Set())).toBe(true)
    expect(isDialogCapture(row('video'), new Set())).toBe(true)
  })

  test("a 'download_all' row is not offered (the drain auto-adds it)", () => {
    expect(isDialogCapture(row('download_all'), new Set())).toBe(false)
  })

  test('a row already offered is not offered again', () => {
    expect(isDialogCapture(row('relay'), new Set(['row-1']))).toBe(false)
    expect(isDialogCapture(row('browser', 'row-2'), new Set(['row-1']))).toBe(true)
  })
})
