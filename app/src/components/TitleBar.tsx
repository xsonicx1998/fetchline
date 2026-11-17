import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Copy, Minus, Square, X } from 'lucide-react'

/** Round 62 — the custom title bar for the frameless windows
 *  (main window + progress window: `decorations: false` in
 *  tauri.conf.json / the progress-window builder). The native
 *  Windows frame is hidden, so this draws the dark title bar:
 *  the new logo + title on the left, real minimize / maximize /
 *  close buttons on the right.
 *
 *  Dragging: only the left area (icon + title) carries
 *  `data-tauri-drag-region` — the buttons sit OUTSIDE the drag
 *  region on purpose, so they are always clickable (a button
 *  inside a drag region can swallow the click on Windows).
 *  Double-clicking the drag area toggles maximize, like a native
 *  title bar. The maximize glyph follows the live maximized
 *  state (`isMaximized` + the resize listener).
 *
 *  The icon is `/icon.png` — the chosen logo, copied into
 *  `app/public/` so Vite bundles it (the src-tauri icons are
 *  not in the frontend dist). */
export function TitleBar({ title, icon = true }: { title: string; icon?: boolean }) {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void getCurrentWindow()
      .isMaximized()
      .then((m) => setMaximized(m))
      .catch(() => undefined)
    void getCurrentWindow()
      .onResized(() => {
        void getCurrentWindow()
          .isMaximized()
          .then((m) => setMaximized(m))
          .catch(() => undefined)
      })
      .then((fn) => {
        unlisten = fn
      })
      .catch(() => undefined)
    return () => {
      unlisten?.()
    }
  }, [])

  function minimize() {
    void getCurrentWindow().minimize().catch(() => undefined)
  }

  function toggleMaximize() {
    void getCurrentWindow().toggleMaximize().catch(() => undefined)
  }

  function close() {
    void getCurrentWindow().close().catch(() => undefined)
  }

  return (
    <div className="flex h-10 shrink-0 items-stretch justify-between border-b border-border bg-surface">
      <div
        data-tauri-drag-region
        onDoubleClick={toggleMaximize}
        className="flex min-w-0 flex-1 items-center gap-2 px-3"
      >
        {icon ? (
          <img
            src="/icon.png"
            alt=""
            aria-hidden="true"
            className="h-5 w-5 shrink-0 rounded-[5px] shadow-[0_0_10px_rgba(139,32,255,0.8)]"
          />
        ) : null}
        <span className="truncate text-[13.5px] font-semibold text-text">{title}</span>
      </div>
      <div className="flex shrink-0 items-center">
        <button
          type="button"
          onClick={minimize}
          aria-label="Minimize"
          title="Minimize"
          className="flex h-10 w-11 items-center justify-center text-text-muted hover:bg-surface-2 hover:text-text"
        >
          <Minus className="h-4 w-4" aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={toggleMaximize}
          aria-label={maximized ? 'Restore' : 'Maximize'}
          title={maximized ? 'Restore' : 'Maximize'}
          className="flex h-10 w-11 items-center justify-center text-text-muted hover:bg-surface-2 hover:text-text"
        >
          {maximized ? (
            <Copy className="h-3.5 w-3.5" aria-hidden="true" />
          ) : (
            <Square className="h-3.5 w-3.5" aria-hidden="true" />
          )}
        </button>
        <button
          type="button"
          onClick={close}
          aria-label="Close"
          title="Close"
          className="flex h-10 w-11 items-center justify-center text-text-muted hover:bg-danger-soft hover:text-danger"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>
    </div>
  )
}
