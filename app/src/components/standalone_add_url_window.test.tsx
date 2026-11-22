// Round 87 — the standalone Add URL dialog's title-bar drag
// (R86 regression).
//
// The R86 retest: the dialog never moved in 3 drag attempts
// (title-bar drag selected text) while the MAIN window dragged
// fine from the same shared TitleBar. The cause was the native
// drag region: `data-tauri-drag-region="true"` only drags on a
// mousedown landing directly on the region element, so grabbing
// the title text selected it instead. The region is now "deep".
// This test renders the REAL standalone window (not just the
// TitleBar) and proves its title bar carries the deep region.
//
// Round 87.1 — exactly ONE drag path: the explicit
// `startDragging()` call is gone, keeping `"deep"` alone — a
// mousedown must NOT call `startDragging()` any more; the
// native drag script owns the drag.
//
// Run with: npm run test:ui

import { describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { getPendingLink } from '../lib/api'
import { StandaloneAddUrlWindow } from './StandaloneAddUrlWindow'

const startDraggingMock = vi.fn(() => Promise.resolve())
const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(() => Promise.resolve()),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    minimize: () => Promise.resolve(),
    hide: () => Promise.resolve(),
    close: () => Promise.resolve(),
    destroy: () => Promise.resolve(),
    startDragging: startDraggingMock,
    isMaximized: () => Promise.resolve(false),
    onResized: () => Promise.resolve(() => {}),
  }),
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}))
vi.mock('../lib/api', () => ({
  // Never resolves: the window stays on "Loading captured
  // link…" and the dialog never mounts, so this test pins the
  // title bar alone. A vi.fn so the manual-mode test can prove
  // it is never called there (Round 87.2).
  getPendingLink: vi.fn(() => new Promise(() => {})),
  getCategories: () => Promise.resolve([]),
  getSettings: () => Promise.resolve(null),
  resolvePendingLink: () => Promise.resolve(),
}))

describe('StandaloneAddUrlWindow drag (Round 87)', () => {
  test('the dialog title bar carries the deep drag region and a mousedown does not call startDragging', () => {
    render(<StandaloneAddUrlWindow linkId="11111111-2222-3333-4444-555555555555" />)
    const title = screen.getByText('Add URL')
    expect(
      title.closest('div')?.getAttribute('data-tauri-drag-region'),
    ).toBe('deep')
    fireEvent.mouseDown(title)
    expect(startDraggingMock).not.toHaveBeenCalled()
  })
})

describe('StandaloneAddUrlWindow manual mode (Round 87.2)', () => {
  test('manual mode mounts the dialog at once with an editable URL and never fetches a pending link', () => {
    vi.mocked(getPendingLink).mockClear()
    render(<StandaloneAddUrlWindow manual />)
    // No pending link is fetched — there is none.
    expect(getPendingLink).not.toHaveBeenCalled()
    // No "Loading…" — the dialog mounts immediately.
    expect(screen.queryByText('Loading captured link…')).toBeNull()
    // The same draggable title bar as a capture.
    const title = screen.getByText('Add URL')
    expect(title.closest('div')?.getAttribute('data-tauri-drag-region')).toBe('deep')
    // Round 87.3 — the title text itself is a drag-region
    // element (the R87.2 retest dragged by the logo but not by
    // this text), and so is the logo.
    expect(title.getAttribute('data-tauri-drag-region')).toBe('deep')
    const img = document.querySelector('img[alt=""]')
    if (!img) throw new Error('no logo img rendered')
    expect(img.getAttribute('data-tauri-drag-region')).toBe('deep')
    // The URL field is present, empty and editable (a capture
    // locks it — `disabled={captureMode}`).
    const urlField = screen.getByLabelText('URL') as HTMLInputElement
    expect(urlField.disabled).toBe(false)
    expect(urlField.value).toBe('')
  })

  test('mousedown on the manual window title logs nothing (Round 87.5: the 87.4 hit-logger is removed)', () => {
    // The R87.3 retest dragged this exact title twice and the
    // window never moved, with no code-level cause left — the
    // 87.4 hit-logger proved the hit element live, and the drag
    // passed 6/6 on the VPS. The logger served its purpose and
    // is gone in 87.5 (no frontend way to gate it on the
    // `debug-hooks` cargo feature), so a grab logs nothing.
    invokeMock.mockClear()
    startDraggingMock.mockClear()
    render(<StandaloneAddUrlWindow manual />)
    fireEvent.mouseDown(screen.getByText('Add URL'))
    expect(startDraggingMock).not.toHaveBeenCalled()
    expect(invokeMock).not.toHaveBeenCalled()
  })
})
