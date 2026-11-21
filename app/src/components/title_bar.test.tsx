// Round 86 — the title bar's minimize + drag.
//
// Minimize used to HIDE the window to the tray (Round 65), so no
// taskbar button existed to restore it from — the taskbar-restore
// check failed in four consecutive rounds (R76, R83, R84, R85).
// Minimize is now a REAL minimize to the Windows taskbar; only X
// hides to the tray. The drag region alone stalled after ~10 px
// on Windows when child elements (the logo img, the title span)
// intercepted hit-testing, so a mousedown on the header also
// calls `startDragging()` explicitly (Jumani's fix).
//
// Round 87 — the REAL cause of the R86 Add URL drag failure:
// `data-tauri-drag-region="true"` only drags on a mousedown
// landing DIRECTLY on the element carrying the attribute
// (Tauri 2.12.5's injected drag.js: `el === composedPath[0]`).
// A grab on the logo or the title text never started a drag and
// selected text instead. The region is now `"deep"` (any click
// in the subtree drags), and a `startDragging()` failure is
// reported to startup.log via `log_frontend` + console.error,
// never swallowed.
//
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { TitleBar } from './TitleBar'

const { minimizeMock, hideMock, closeMock, startDraggingMock, logFrontendMock } =
  vi.hoisted(() => ({
    minimizeMock: vi.fn(() => Promise.resolve()),
    hideMock: vi.fn(() => Promise.resolve()),
    closeMock: vi.fn(() => Promise.resolve()),
    startDraggingMock: vi.fn(() => Promise.resolve()),
    logFrontendMock: vi.fn(() => Promise.resolve()),
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
vi.mock('@tauri-apps/api/core', () => ({
  invoke: logFrontendMock,
}))

beforeEach(() => {
  minimizeMock.mockClear()
  hideMock.mockClear()
  closeMock.mockClear()
  startDraggingMock.mockClear()
  logFrontendMock.mockClear()
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

describe('TitleBar drag (Round 86)', () => {
  test('mousedown on the title starts the drag', () => {
    render(<TitleBar title="Sonic Manager" />)
    fireEvent.mouseDown(screen.getByText('Sonic Manager'))
    expect(startDraggingMock).toHaveBeenCalledTimes(1)
  })

  test('right-click mousedown does not start the drag', () => {
    render(<TitleBar title="Sonic Manager" />)
    fireEvent.mouseDown(screen.getByText('Sonic Manager'), { button: 2 })
    expect(startDraggingMock).not.toHaveBeenCalled()
  })

  test('mousedown on a button does not start the drag', () => {
    render(<TitleBar title="Sonic Manager" />)
    fireEvent.mouseDown(screen.getByRole('button', { name: 'Minimize' }))
    expect(startDraggingMock).not.toHaveBeenCalled()
  })

  test('a non-draggable window has no drag region', () => {
    render(<TitleBar title="Sonic Manager" draggable={false} />)
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

  test('a mousedown on the logo image starts the drag too', () => {
    render(<TitleBar title="Sonic Manager" />)
    const img = document.querySelector('img[alt=""]')
    if (!img) throw new Error('no logo img rendered')
    fireEvent.mouseDown(img)
    expect(startDraggingMock).toHaveBeenCalledTimes(1)
  })

  test('a startDragging failure is reported, never swallowed', async () => {
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      startDraggingMock.mockRejectedValueOnce(new Error('permission denied'))
      render(<TitleBar title="Sonic Manager" />)
      fireEvent.mouseDown(screen.getByText('Sonic Manager'))
      await vi.waitFor(() => {
        expect(logFrontendMock).toHaveBeenCalledWith(
          'log_frontend',
          expect.objectContaining({
            message: expect.stringContaining('startDragging failed'),
          }),
        )
      })
      expect(consoleSpy).toHaveBeenCalledWith(
        expect.stringContaining('startDragging failed'),
      )
    } finally {
      consoleSpy.mockRestore()
    }
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
    fireEvent.mouseDown(screen.getByText('movie.mp4'))
    expect(startDraggingMock).toHaveBeenCalledTimes(1)
  })
})
