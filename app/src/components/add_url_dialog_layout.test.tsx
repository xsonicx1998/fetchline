// Round 85 — Add URL dialog fits a short viewport.
//
// The R84 retest: the dialog was "not draggable and taller
// than a 500px viewport (buttons below the fold)". What was
// actually wrong in code: the IN-WINDOW dialog had no
// max-height and no scrollable body — on a short (RDP) main
// window the fields ran past the viewport and Start Download
// sat below the fold with no way to reach it except Tab. The
// standalone window's body already scrolled, but its window
// was fixed at 560x520 with min == size and resizable(false),
// so it could not fit a ~500px viewport at all (lib.rs now:
// preferred 560x520, min 460x360, resizable).
//
// jsdom does no layout, so these tests lock the structure that
// makes the dialog fit: the box is capped at the viewport, the
// body scrolls, and the footer (Start Download) is shrink-0 —
// in BOTH variants. No fields were removed and no control was
// shrunk.
//
// Run with: npm run test:ui

import { describe, expect, test, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { AddDownloadDialog } from './AddDownloadDialog'

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

function renderDialog(standalone: boolean) {
  return render(
    <AddDownloadDialog
      open
      standalone={standalone}
      categories={[]}
      initialUrl="https://example.com/file.zip"
      onClose={() => {}}
      onAdded={() => {}}
      onLateError={() => {}}
    />,
  )
}

describe('Add URL dialog fits a short viewport', () => {
  test('in-window: box capped at the viewport, body scrolls, footer stays put', () => {
    renderDialog(false)
    const dialog = screen.getByRole('dialog')
    // The overlay is the full viewport; the box inside it is
    // capped (max-h-full of the padded overlay) and a column,
    // so the body can scroll inside it.
    const box = dialog.firstElementChild as HTMLElement
    expect(box.className).toContain('max-h-full')
    expect(box.className).toContain('flex-col')

    const urlInput = screen.getByLabelText('URL')
    const body = urlInput.closest('.overflow-y-auto') as HTMLElement | null
    expect(body).not.toBeNull()
    expect(body!.className).toContain('flex-1')

    // The footer holding Start Download never scrolls away.
    const footer = screen.getByText('Start Download').closest('div') as HTMLElement
    expect(footer.className).toContain('shrink-0')
    expect(footer.className).toContain('justify-end')
  })

  test('standalone: body scrolls inside the window, footer stays put', () => {
    renderDialog(true)
    const dialog = screen.getByRole('dialog')
    expect(dialog.className).toContain('h-full')
    expect(dialog.className).toContain('flex-col')

    const urlInput = screen.getByLabelText('URL')
    const body = urlInput.closest('.overflow-y-auto') as HTMLElement | null
    expect(body).not.toBeNull()
    expect(body!.className).toContain('flex-1')

    const footer = screen.getByText('Start Download').closest('div') as HTMLElement
    expect(footer.className).toContain('shrink-0')
    expect(footer.className).toContain('justify-end')
  })

  test('both variants keep every field and control', () => {
    for (const standalone of [false, true]) {
      const { unmount } = renderDialog(standalone)
      expect(screen.getByLabelText('URL')).toBeTruthy()
      expect(screen.getByLabelText('Category')).toBeTruthy()
      expect(screen.getByLabelText('Save As')).toBeTruthy()
      expect(screen.getByLabelText('Save folder')).toBeTruthy()
      expect(screen.getByLabelText('Description')).toBeTruthy()
      expect(screen.getByText('Start Download')).toBeTruthy()
      expect(screen.getByText('Download Later')).toBeTruthy()
      expect(screen.getByText('Cancel')).toBeTruthy()
      unmount()
    }
  })
})
