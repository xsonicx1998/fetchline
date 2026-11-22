// Round 86 — the title bar's minimize + drag.
//
// Minimize used to HIDE the window to the tray (Round 65), so no
// taskbar button existed to restore it from — the taskbar-restore
// check failed in four consecutive rounds (R76, R83, R84, R85).
// Minimize is now a REAL minimize to the Windows taskbar; only X
// hides to the tray.
//
// Round 87 — the REAL cause of the R86 Add URL drag failure:
// `data-tauri-drag-region="true"` only drags on a mousedown
// landing DIRECTLY on the element carrying the attribute
// (Tauri 2.12.5's injected drag.js: `el === composedPath[0]`).
// A grab on the logo or the title text never started a drag and
// selected text instead. The region is now `"deep"` (any click
// in the subtree drags).
//
// Round 87.1 — exactly ONE drag path: the explicit
// `startDragging()` call is gone, keeping `"deep"` alone — two
// drag starts per click can leave the window stuck/released,
// and the R86 onMouseDown alone already failed to move the
// dialog. A mousedown must NOT call `startDragging()` any more;
// the native drag script owns the drag.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { TitleBar } from './TitleBar'

const { minimizeMock, hideMock, closeMock, startDraggingMock } = vi.hoisted(() => ({
  minimizeMock: vi.fn(() => Promise.resolve()),
  hideMock: vi.fn(() => Promise.resolve()),
  closeMock: vi.fn(() => Promise.resolve()),
  startDraggingMock: vi.fn(() => Promise.resolve()),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    minimize: minimizeMock,
    hide: hideMock,
    close: closeMock,
    startDragging: startDraggingMock,
    isMaximized: () => Promise.resolve(false),
    onResized: () => Promise.resolve(() => {}),
  }),
}))

beforeEach(() => {
  minimizeMock.mockClear()
  hideMock.mockClear()
  closeMock.mockClear()
  startDraggingMock.mockClear()
})

describe('TitleBar minimize (Round 86)', () => {
  test('minimize does a real minimize to the taskbar, never hide', () => {
    render(<TitleBar title="Sonic Manager" />)
    fireEvent.click(screen.getByRole('button', { name: 'Minimize' }))
    expect(minimizeMock).toHaveBeenCalledTimes(1)
    expect(hideMock).not.toHaveBeenCalled()
  })

  test('X on the main window still hides to the tray', () => {
    render(<TitleBar title="Sonic Manager" hideOnClose />)
    fireEvent.click(screen.getByRole('button', { name: 'Hide to tray' }))
    expect(hideMock).toHaveBeenCalledTimes(1)
    expect(closeMock).not.toHaveBeenCalled()
    expect(minimizeMock).not.toHaveBeenCalled()
  })

  test('X on a plain window still closes it', () => {
    render(<TitleBar title="Download complete" />)
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(closeMock).toHaveBeenCalledTimes(1)
    expect(hideMock).not.toHaveBeenCalled()
  })
})

describe('TitleBar drag (Round 87.1: exactly one path)', () => {
  test('mousedown on the title does NOT call startDragging — the "deep" region owns the drag', () => {
    render(<TitleBar title="Sonic Manager" />)
    // The native drag.js handles the drag from the attribute
    // alone; the component must not start a second drag.
    const region = screen.getByText('Sonic Manager').closest('div')
    expect(region?.getAttribute('data-tauri-drag-region')).toBe('deep')
    fireEvent.mouseDown(screen.getByText('Sonic Manager'))
    expect(startDraggingMock).not.toHaveBeenCalled()
  })

  test('mousedown on the logo image does NOT call startDragging either', () => {
    render(<TitleBar title="Sonic Manager" />)
    const img = document.querySelector('img[alt=""]')
    if (!img) throw new Error('no logo img rendered')
    fireEvent.mouseDown(img)
    expect(startDraggingMock).not.toHaveBeenCalled()
  })

  test('a non-draggable window has no drag region', () => {
    render(<TitleBar title="Sonic Manager" draggable={false} />)
    const region = screen.getByText('Sonic Manager').closest('div')
    expect(region?.getAttribute('data-tauri-drag-region')).toBeNull()
    fireEvent.mouseDown(screen.getByText('Sonic Manager'))
    expect(startDraggingMock).not.toHaveBeenCalled()
  })
})

describe('TitleBar drag region (Round 87)', () => {
  test('the drag region is "deep" so grabs on the logo and title drag', () => {
    render(<TitleBar title="Sonic Manager" />)
    // The R86 failure: `data-tauri-drag-region="true"` only drags
    // on a mousedown landing directly on the region element, so
    // grabbing the title text (a child span) selected text
    // instead of dragging. "deep" covers the whole subtree.
    const region = screen.getByText('Sonic Manager').closest('div')
    expect(region?.getAttribute('data-tauri-drag-region')).toBe('deep')
  })

  test('the title text itself carries the drag region (Round 87.3)', () => {
    // The R87.2 retest: the manual Add URL window dragged by
    // the LOGO but NOT by the TITLE TEXT (two attempts), even
    // though the parent div already carried "deep". The title
    // span now carries the attribute itself, so a grab on the
    // text lands directly on a drag-region element
    // (`composedPath()[0]`) with no ancestor walk needed —
    // and the logo img does too, for symmetry.
    render(<TitleBar title="Sonic Manager" />)
    const title = screen.getByText('Sonic Manager')
    expect(title.getAttribute('data-tauri-drag-region')).toBe('deep')
    const img = document.querySelector('img[alt=""]')
    if (!img) throw new Error('no logo img rendered')
    expect(img.getAttribute('data-tauri-drag-region')).toBe('deep')
  })

  test('the standalone Add URL dialog title bar drags (R86 regression)', () => {
    // Exactly the props StandaloneAddUrlWindow passes: no
    // minimize / maximize, draggable. The R86 retest's dialog
    // never moved in 3 attempts while the main window dragged
    // from the same component — this pins the dialog's own
    // title-bar drag path.
    render(
      <TitleBar
        title="movie.mp4"
        showMinimize={false}
        showMaximize={false}
        draggable
      />,
    )
    const region = screen.getByText('movie.mp4').closest('div')
    expect(region?.getAttribute('data-tauri-drag-region')).toBe('deep')
  })
})
