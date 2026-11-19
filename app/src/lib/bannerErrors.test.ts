// Round 75, item 6 — banner dismissals keyed per (source,
// message) and the failed-capture no-retry set. Pure logic; run
// with: npm run test:ui

import { describe, expect, test } from 'vitest'
import { BannerDismissals, FailedCaptures } from './bannerErrors'

describe('BannerDismissals (Round 75)', () => {
  test('a dismissed (source, message) does not show again', () => {
    const d = new BannerDismissals()
    expect(d.shouldShow('capture', 'missing required key linkId')).toBe(true)
    d.dismiss('capture', 'missing required key linkId')
    expect(d.shouldShow('capture', 'missing required key linkId')).toBe(false)
  })

  test('a different message or a different source still shows', () => {
    const d = new BannerDismissals()
    d.dismiss('capture', 'missing required key linkId')
    expect(d.shouldShow('capture', 'Backend did not respond')).toBe(true)
    expect(d.shouldShow('poll:downloads', 'missing required key linkId')).toBe(true)
  })

  test('a successful poll clears only its own source', () => {
    const d = new BannerDismissals()
    d.dismiss('poll:downloads', 'Backend did not respond')
    d.dismiss('capture', 'missing required key linkId')
    // The downloads poll succeeded: its dismissal is forgotten,
    // so a later failure with the same text shows again.
    d.clearForSource('poll:downloads')
    expect(d.shouldShow('poll:downloads', 'Backend did not respond')).toBe(true)
    // The capture dismissal survives the unrelated poll — the
    // auto-open failure must NOT be un-dismissed by it.
    expect(d.shouldShow('capture', 'missing required key linkId')).toBe(false)
  })
})

describe('FailedCaptures (Round 75)', () => {
  test('a failed auto-open is not retried until Retry clears it', () => {
    const failed = new FailedCaptures()
    expect(failed.has('link-1')).toBe(false)
    failed.markFailed('link-1')
    expect(failed.has('link-1')).toBe(true)
    // The other queued captures are unaffected.
    expect(failed.has('link-2')).toBe(false)
    failed.clear('link-1')
    expect(failed.has('link-1')).toBe(false)
  })

  test('seven stuck captures queue one at a time; one failure does not block the rest', () => {
    // The Round 74 linkId fallout: 7 pending rows waited for the
    // Add URL window. A failure of one must not spam-retried, and
    // must not stop the other six from opening.
    const failed = new FailedCaptures()
    const queue = ['link-1', 'link-2', 'link-3', 'link-4', 'link-5', 'link-6', 'link-7']
    failed.markFailed('link-3')
    const toOpen = queue.filter((id) => !failed.has(id))
    expect(toOpen).toEqual(['link-1', 'link-2', 'link-4', 'link-5', 'link-6', 'link-7'])
    expect(toOpen).toHaveLength(6)
  })
})
