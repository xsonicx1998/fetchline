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
 *  not in the frontend dist).
 *
 *  Round 65 — minimize to tray, like IDM: the minimize button
 *  HIDES the window to the system tray instead of dropping it
 *  to the taskbar (the tray icon brings it back). The MAIN
 *  window's X hides it to the tray too (`hideOnClose`) — the
 *  tray menu's Exit is the only way to quit the app. A
 *  progress window's X still closes just that window (the
 *  download keeps running); its minimize hides it to the tray
 *  like the main window's. */
export function TitleBar({
  title,
  icon = true,
  hideOnClose = false,
  showMinimize = true,
  showMaximize = true,
  onClose,
  draggable = true,
}: {
  title: string
  icon?: boolean
  /** Round 65 — main window only: X hides to the tray instead
   *  of closing. Progress windows leave this off. */
  hideOnClose?: boolean
  /** Round 66 — the "Download complete" dialog hides both:
   *  minimize would hide the dialog to the tray (Round 65),
   *  where the tray menu could never bring it back, and a
   *  fixed-size dialog has nothing to maximize. */
  showMinimize?: boolean
  showMaximize?: boolean
  /** Round 69 (d) — when the bar is drawn inside the MAIN window
   *  (the add-time error dialog rendered as an overlay, not its
   *  own OS window), X must dismiss the overlay, not close the
   *  app's main window. Absent, X keeps its old behaviour. */
  onClose?: () => void
  /** Round 74 (16) — drag + resize apply ONLY to the main window
   *  and the active download progress window. Fixed-size dialogs
   *  (complete, error, standalone Add URL) pass `false`: the drag
   *  region is omitted, so the window cannot be dragged. */
  draggable?: boolean
}) {
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
    void getCurrentWindow().hide().catch(() => undefined)
  }

  function toggleMaximize() {
    void getCurrentWindow().toggleMaximize().catch(() => undefined)
  }

  function close() {
    if (onClose) {
      onClose()
      return
    }
    if (hideOnClose) {
      void getCurrentWindow().hide().catch(() => undefined)
    } else {
      void getCurrentWindow().close().catch(() => undefined)
    }
  }

  return (
    <div className="flex h-10 shrink-0 items-stretch justify-between border-b border-border bg-surface">
      <div
        {...(draggable
          ? { 'data-tauri-drag-region': true as const, onDoubleClick: toggleMaximize }
          : {})}
        className="flex min-w-0 flex-1 items-center gap-2 px-3"
      >
        {icon ? (
          // Round 84 — frameless: the R83 retest still showed the
          // logo in a "dark rounded square with a purple glow".
          // The pixels are innocent (public/icon.png IS the
          // transparent emblem — byte-identical to the
          // flood-fill cut, mean opaque RGB (91,51,213), and the
          // alpha map is a diagonal quills shape, not a baked
          // tile), so the box was these classes: rounded-[5px]
          // clipped the 20px artwork into a rounded square and
          // the purple glow outlined it as a tile. No rounding,
          // no glow — the graphic floats on the title bar.
          <img
            src="/icon.png"
            alt=""
            aria-hidden="true"
            className="h-5 w-5 shrink-0"
          />
        ) : null}
        <span className="truncate text-[13.5px] font-semibold text-text">{title}</span>
      </div>
      <div className="flex shrink-0 items-center">
        {showMinimize ? (
          <button
            type="button"
            onClick={minimize}
            aria-label="Minimize to tray"
            title="Minimize to tray"
            className="flex h-10 w-11 items-center justify-center text-text-muted hover:bg-surface-2 hover:text-text"
          >
            <Minus className="h-4 w-4" aria-hidden="true" />
          </button>
        ) : null}
        {showMaximize ? (
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
        ) : null}
        <button
          type="button"
          onClick={close}
          aria-label={hideOnClose ? 'Hide to tray' : 'Close'}
          title={hideOnClose ? 'Hide to tray' : 'Close'}
          className="flex h-10 w-11 items-center justify-center text-text-muted hover:bg-danger-soft hover:text-danger"
        >
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>
    </div>
  )
}
