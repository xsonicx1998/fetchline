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
// TitleBar) and proves its title bar carries the deep region
// and starts the drag.
//
// Run with: npm run test:ui

import { describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { StandaloneAddUrlWindow } from './StandaloneAddUrlWindow'

const startDraggingMock = vi.fn(() => Promise.resolve())
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
  invoke: () => Promise.resolve(),
}))
vi.mock('../lib/api', () => ({
  // Never resolves: the window stays on "Loading captured
  // link…" and the dialog never mounts, so this test pins the
  // title bar alone.
  getPendingLink: () => new Promise(() => {}),
  getCategories: () => Promise.resolve([]),
  getSettings: () => Promise.resolve(null),
  resolvePendingLink: () => Promise.resolve(),
}))

describe('StandaloneAddUrlWindow drag (Round 87)', () => {
  test('mousedown on the dialog title bar starts the drag', () => {
    render(<StandaloneAddUrlWindow linkId="11111111-2222-3333-4444-555555555555" />)
    const title = screen.getByText('Add URL')
    expect(
      title.closest('div')?.getAttribute('data-tauri-drag-region'),
    ).toBe('deep')
    fireEvent.mouseDown(title)
    expect(startDraggingMock).toHaveBeenCalledTimes(1)
  })
})
