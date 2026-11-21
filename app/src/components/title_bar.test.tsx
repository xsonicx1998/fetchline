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
// Run with: npm run test:ui

import { beforeEach, describe, expect, test, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { TitleBar } from './TitleBar'

const minimizeMock = vi.fn(() => Promise.resolve())
const hideMock = vi.fn(() => Promise.resolve())
const closeMock = vi.fn(() => Promise.resolve())
const startDraggingMock = vi.fn(() => Promise.resolve())
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
