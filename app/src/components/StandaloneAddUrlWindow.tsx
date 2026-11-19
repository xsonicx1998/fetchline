import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  getCategories,
  getPendingLink,
  getSettings,
  resolvePendingLink,
} from '../lib/api'
import type { CategoryRow, PendingLinkRow, Settings } from '../lib/types'
import { AddDownloadDialog, filenameFromUrl } from './AddDownloadDialog'
import { Button } from './Button'
import { TitleBar } from './TitleBar'

/** Round 74 (13) — the standalone Add URL window for a browser
 *  capture: label `add-url-<pending-link-id>`, the link id from
 *  `window.__ADD_URL_ID__` (or the `#add-url=<id>` hash fallback
 *  — see main.tsx). It fetches the pending link via
 *  `get_pending_link`, then renders the SAME Add URL dialog as
 *  the main window, in capture mode (file name as the title, URL
 *  locked — Round 74 (12)).
 *
 *  FIXED SIZE by the user's decision — no minimize / maximize.
 *  Round 76 (1) — the TitleBar IS draggable now (the user
 *  overrides Round 74 (16) for this window: it can be moved),
 *  and its title is the file name (the URL's last segment),
 *  not "Add URL". X / Cancel / Escape close the window
 *  (Round 74 (14)); a close without a submit resolves the
 *  capture as rejected, so the main window never offers the
 *  same link again. A submit closes the window too — the main
 *  window's 1 s poll picks up the new row. No heartbeat:
 *  main.tsx skips it for this window like the complete /
 *  error windows. Round 76 (7) — Escape has its own listener
 *  here, not just the dialog's: while the link is still
 *  loading (or errored) the dialog is not mounted, so its
 *  Escape handler does not exist yet and Escape did nothing. */
export function StandaloneAddUrlWindow({ linkId }: { linkId: string }) {
  const [link, setLink] = useState<PendingLinkRow | null>(null)
  const [linkError, setLinkError] = useState<string | null>(null)
  const [categories, setCategories] = useState<CategoryRow[]>([])
  const [settings, setSettings] = useState<Settings | null>(null)
  const [lateError, setLateError] = useState<string | null>(null)
  /** The dialog already submitted the capture, so closing the
   *  window must not resolve it as rejected (same guard as the
   *  main window's `captureAddedRef`). */
  const submittedRef = useRef(false)
  /** Round 76 (7) — Escape is handled both here and by the
   *  dialog (its own handler calls onClose too): without this
   *  guard one press would resolve the capture as rejected
   *  twice. */
  const closingRef = useRef(false)

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', 'dark')
    let stopped = false
    void getPendingLink(linkId)
      .then((row) => {
        if (stopped) return
        // A missing row (expired or already answered elsewhere)
        // must not leave the window on "Loading…" forever.
        if (row === null) {
          setLinkError('This captured link is no longer available.')
        } else {
          setLink(row)
        }
      })
      .catch((e: unknown) => {
        if (!stopped) {
          setLinkError(e instanceof Error ? e.message : String(e))
        }
      })
    void getCategories()
      .then((rows) => {
        if (!stopped) setCategories(rows)
      })
      .catch(() => undefined)
    void getSettings()
      .then((s) => {
        if (!stopped) setSettings(s)
      })
      .catch(() => undefined)
    return () => {
      stopped = true
    }
  }, [linkId])

  // Round 76 (2) — refetch the categories whenever this window
  // regains focus: a window focused from an earlier mount holds
  // the categories from that mount, so a folder remembered since
  // (from the main window, or another capture) would never show
  // in its autofill.
  useEffect(() => {
    let stopped = false
    const onFocus = () => {
      void getCategories()
        .then((rows) => {
          if (!stopped) setCategories(rows)
        })
        .catch(() => undefined)
    }
    window.addEventListener('focus', onFocus)
    return () => {
      stopped = true
      window.removeEventListener('focus', onFocus)
    }
  }, [])

  function closeWindow() {
    void getCurrentWindow().close().catch(() => undefined)
  }

  function handleClose() {
    if (closingRef.current) return
    closingRef.current = true
    // A dismissed capture is rejected, unless the dialog already
    // submitted it — the engine already accepted it then, and a
    // reject would clobber the real outcome.
    if (link && !submittedRef.current) {
      void resolvePendingLink({
        id: link.id,
        status: 'rejected',
        error: 'Dismissed in the Add URL dialog',
      }).catch(() => undefined)
    }
    closeWindow()
  }

  // Round 76 (7) — Escape closes this window directly. The
  // dialog has its own Escape handler, but it only exists while
  // the dialog is mounted: during "Loading captured link…" (or
  // the load error) there is no dialog, so Escape did nothing.
  // `handleClose` is in the deps so the listener always sees the
  // latest link (a stale first-render closure would skip the
  // reject resolve and the capture would be re-offered).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') handleClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handleClose])

  function handleAdded() {
    submittedRef.current = true
    closeWindow()
  }

  return (
    <div className="flex h-full flex-col bg-bg text-text">
      {/* Round 76 (1) — the title is the file name (the URL's
          last segment; the pending row carries no filename), and
          the window is draggable — the user overrides Round 74
          (16) for this window. "Add URL" only while loading. */}
      <TitleBar
        title={link ? filenameFromUrl(link.url) || 'Add URL' : 'Add URL'}
        showMinimize={false}
        showMaximize={false}
        draggable
        onClose={handleClose}
      />
      <div className="h-[2px] shrink-0 bg-gradient-to-r from-[#8B20FF] to-[#F02DCD]" />
      {linkError ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <p className="text-sm text-danger">
            Could not load this captured link: {linkError}
          </p>
          <Button variant="secondary" onClick={closeWindow}>
            Close
          </Button>
        </div>
      ) : link === null ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <p className="text-sm text-text-muted">Loading captured link…</p>
        </div>
      ) : (
        <>
          {lateError ? (
            <p role="alert" className="shrink-0 px-4 pt-3 text-sm text-danger">
              {lateError}
            </p>
          ) : null}
          <div className="flex min-h-0 flex-1 flex-col">
            <AddDownloadDialog
              open
              standalone
              captureMode
              categories={categories}
              defaultAction={settings?.default_action ?? 'start_now'}
              defaultFolder={settings?.default_download_folder ?? ''}
              initialUrl={link.url}
              pendingLinkId={link.id}
              initialCookies={link.cookies}
              onClose={handleClose}
              onAdded={handleAdded}
              onLateError={(message) => setLateError(message)}
            />
          </div>
        </>
      )}
    </div>
  )
}
