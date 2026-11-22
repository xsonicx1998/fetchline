import { useEffect, useState, type MouseEvent as ReactMouseEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
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
 *  `data-tauri-drag-region="deep"` — the buttons sit OUTSIDE
 *  the drag region on purpose, so they are always clickable (a
 *  button inside a drag region can swallow the click on
 *  Windows). Double-clicking the drag area toggles maximize,
 *  like a native title bar. The maximize glyph follows the live
 *  maximized state (`isMaximized` + the resize listener).
 *  Round 86 — the region alone stalls after ~10 px on Windows
 *  when child elements (the logo img, the title span) intercept
 *  hit-testing, so a mousedown on the header also calls
 *  `startDragging()` explicitly (Jumani's fix); buttons and
 *  inputs are skipped.
 *
 *  Round 87 — the REAL cause of the R86 Add URL drag failure,
 *  proven from Tauri 2.12.5's own injected drag script
 *  (`tauri/src/window/scripts/drag.js`, added to every webview
 *  by the window plugin): `data-tauri-drag-region="true"` (what
 *  React renders for `true`) drags ONLY on a mousedown landing
 *  DIRECTLY on the element carrying the attribute
 *  (`el === composedPath[0]`). A grab on the logo img or the
 *  title span — where every user grabs — returns false, so the
 *  script never calls `preventDefault()` and never starts a
 *  drag: the mousedown begins a TEXT SELECTION instead. That is
 *  exactly the R86 retest symptom ("title-bar drag selected
 *  text"). The R86 comment above misread it as a ~10 px stall;
 *  for child clicks the native region never starts at all, and
 *  only the explicit `startDragging()` below saved the main
 *  window. The fix is `"deep"`: drag.js treats it as "any click
 *  in the subtree drags" (clickable children still block it, and
 *  our buttons sit outside the region anyway), so the native
 *  path handles the drag itself — `preventDefault()` kills the
 *  text selection and `start_dragging` fires without depending
 *  on this component's React handler at all. Double-click
 *  maximize also moves to the native path (drag.js invokes
 *  `internal_toggle_maximize` on detail === 2), so the React
 *  `onDoubleClick` is gone — keeping it would toggle maximize
 *  TWICE (native + React) and net out to nothing.
 *
 *  Round 87.1 — exactly ONE drag path: the explicit
 *  `startDragging()` fallback is REMOVED, keeping
 *  `data-tauri-drag-region="deep"` alone. Two drag starts per
 *  click (native region + explicit call) can leave the window
 *  stuck/released, and the R86 onMouseDown alone already
 *  failed to move the dialog — one path, owned by the native
 *  script, is the only behaviour the retest can judge. The
 *  `log_frontend` command stays registered in lib.rs as a
 *  diagnostic hook (other callers may use it); this component
 *  simply no longer calls it.
 *
 *  Round 87.4 — DIAGNOSIS of the R87.3 live failure (title-text
 *  drag still dead on SHIPPED), proven from code + the
 *  production build — no code-level cause is left:
 *
 *  PROVEN — the element under the title text IS the span at
 *  :224 with `data-tauri-drag-region="deep"`, and the
 *  production bundle (`npm run build`, the exact command the
 *  VPS build runs via `beforeBuildCommand`) contains all three
 *  `"deep"` attributes (parent div, logo img, title span).
 *  Tauri is lock-pinned to 2.12.5 and its drag.js was read
 *  verbatim: a left mousedown on that span MUST return true
 *  from `isDragRegion` (the span is `composedPath()[0]`, attr
 *  `'deep'` → `true`) and invoke `start_dragging`. No
 *  clickable element sits between the span and the region,
 *  and no `stopPropagation` / `stopImmediatePropagation` /
 *  `onMouseDown` handler exists anywhere in `src/` that could
 *  intercept the mousedown before drag.js's document
 *  listener — the only `preventDefault`s are the DownloadTable
 *  row keys, the CategorySidebar, and the right-click-menu
 *  suppression in main.tsx.
 *  RULED OUT — attribute stripped in the build (verified in
 *  dist); wrong Tauri/drag.js version (lock pins 2.12.5,
 *  drag.js identical in 2.12.7); a double title / wrong
 *  element grabbed (the dialog's inner header is `null` in
 *  standalone mode, AddDownloadDialog.tsx:799 — the TitleBar
 *  span is the ONLY "Add URL" text); text selection stealing
 *  the drag (global `user-select: none` on html/body,
 *  index.css:67, and drag.js `preventDefault()` would kill
 *  it anyway); an overlay on top of the span (no positioned
 *  element in the TitleBar subtree; the buttons sit to the
 *  RIGHT, outside the drag div); a stale frontend in the
 *  SHIPPED build at the pipeline level (`beforeBuildCommand`
 *  rebuilds the frontend — but the INSTALLED exe's identity
 *  was never verified on the VPS: the 87.2 and 87.3 SHIPPED
 *  exes were both exactly 19,808,768 B, so a wrong/old MSI
 *  stays a live hypothesis for the retest to close).
 *  PROVEN — why the 87.3 remote-debugging attempt failed:
 *  `tauri-plugin-single-instance` (Cargo.toml:30,
 *  lib.rs:3858) means a second launch only FOCUSES the
 *  already-running instance, so the env-var relaunch never
 *  started a new process and port 9222 never opened. The
 *  live diagnosis MUST kill the app first (see build-log.md).
 *
 *  What is left is live-only: (H1) the retest's pixel
 *  coordinates missed the span, (H2) the wrong SHIPPED
 *  binary was actually running, or (H3) a native/WebView2
 *  quirk invisible to code. The diagnosis does NOT prove the
 *  attribute route cannot work, so Round 87.1 stands: NO
 *  `startDragging()` fallback. What this round changes is
 *  observability: a mousedown hit-logger (below) records
 *  exactly what element every title-bar grab hit, in
 *  startup.log, so the next retest can PROVE the hit element
 *  even if the debug port fails again.
 *
 *  Round 87.3 — the R87.2 retest: the manual Add URL window
 *  drags by the LOGO but NOT by the TITLE TEXT (two attempts).
 *  The region div already carries "deep", which per drag.js
 *  covers the whole subtree, and the global `user-select: none`
 *  rules out text selection — so there is no code-level
 *  asymmetry between logo and title left in the parent div,
 *  and the miss must be in how the grab reaches the native
 *  script (ancestor walk vs. the element itself). The logo
 *  and the title span now carry `data-tauri-drag-region="deep"`
 *  THEMSELVES, so a mousedown on either lands directly on a
 *  drag-region element (`composedPath()[0]`) with no ancestor
 *  walk needed. Still exactly one drag path — no
 *  `startDragging()` fallback (Round 87.1 stands). The
 *  capture window shares this component, so it gains the
 *  same fix; its drag was NOT RUN in the 87.2 retest.
 *
 *  The icon is `/icon.png` — the chosen logo, copied into
 *  `app/public/` so Vite bundles it (the src-tauri icons are
 *  not in the frontend dist).
 *
 *  Round 86 — minimize to the TASKBAR (this supersedes the
 *  Round 65 hide-to-tray minimize): the minimize button does a
 *  real minimize, so the window gets a taskbar button and
 *  clicking it restores the window — the old behaviour failed
 *  the taskbar-restore check in four consecutive rounds (R76,
 *  R83, R84, R85). Only X hides to the tray (`hideOnClose` on
 *  the main window); the tray menu's Exit is the only way to
 *  quit the app. A progress window's X still closes just that
 *  window (the download keeps running); its minimize drops it
 *  to the taskbar like the main window's. */
/** Round 87.4 — diagnostic mousedown hit-logger (NOT a drag
 *  path): records what element a title-bar grab actually hit —
 *  target tag, its own drag-region attribute, and the closest
 *  ancestor's — via the `log_frontend` command, so it lands in
 *  startup.log where the retest reads it. It changes NO drag
 *  behaviour: no `preventDefault`, no `stopPropagation`, no
 *  `startDragging()` — the native drag script owns the drag,
 *  and this handler only observes (React's bubble-phase
 *  listener runs before drag.js's document listener, and it
 *  leaves the event untouched). A failed invoke is swallowed:
 *  the logger must never break the title bar. Only wired on
 *  the draggable div, so a fixed-size dialog logs nothing. */
function logTitleBarHit(e: ReactMouseEvent) {
  const target = e.target as HTMLElement | null
  if (!target) return
  const region = target.closest('[data-tauri-drag-region]')
  const message =
    `titlebar mousedown: target=${target.tagName} ` +
    `drag-region=${target.getAttribute('data-tauri-drag-region') ?? 'none'} ` +
    `closest-region=${region?.getAttribute('data-tauri-drag-region') ?? 'none'}`
  void invoke('log_frontend', { message }).catch(() => undefined)
}

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
   *  region is omitted, so the window cannot be dragged. Round 76
   *  overrides this for the complete dialog and the standalone
   *  Add URL window (both pass `true` — the user wants them
   *  movable); only the error dialog still passes `false`.
   *  Round 87 — the region is `"deep"` (see the header comment):
   *  any click in the title-bar subtree drags. */
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

  // Round 86 — a REAL minimize to the Windows taskbar (see
  // the header comment): the old `hide()` here sent the window
  // to the tray on minimize, where no taskbar button existed
  // to restore it from.
  function minimize() {
    void getCurrentWindow().minimize().catch(() => undefined)
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
          ? {
              // Round 87 — "deep", not `true`: see the header
              // comment. `true` only drags on direct clicks on
              // this div; the logo and title are children, so
              // grabbing them selected text instead of dragging.
              // "deep" drags on any click in the subtree, and
              // double-click maximize is the native path too.
              // Round 87.1 — this attribute is the ONLY drag
              // path: no `onMouseDown` → `startDragging()` —
              // two drag starts per click can leave the window
              // stuck, and the explicit call alone already
              // failed to move the dialog in R86.
              // Round 87.4 — `onMouseDown` here is the
              // hit-LOGGER only (see `logTitleBarHit`): it
              // records the grab, it never starts a drag.
              'data-tauri-drag-region': 'deep' as const,
              onMouseDown: logTitleBarHit,
            }
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
            // Round 87.3 — the logo itself is a drag-region
            // element: a grab on it lands directly on the
            // region (see the header comment).
            data-tauri-drag-region="deep"
            className="h-5 w-5 shrink-0"
          />
        ) : null}
        {/* Round 87.3 — the title span itself is a drag-region
            element: the R87.2 retest dragged the manual Add URL
            window by the logo but NOT by this text, so the grab
            must land on the region directly instead of relying
            on the parent div's "deep" ancestor walk. */}
        <span
          data-tauri-drag-region="deep"
          className="truncate text-[13.5px] font-semibold text-text"
        >
          {title}
        </span>
      </div>
      <div className="flex shrink-0 items-center">
        {showMinimize ? (
          <button
            type="button"
            onClick={minimize}
            aria-label="Minimize"
            title="Minimize"
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
