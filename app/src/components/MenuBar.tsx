import { useEffect, useState } from 'react'
import type { DownloadRow, QueueRow } from '../lib/types'
import { Button } from './Button'

/** The actions the menu bar offers. Every entry wires to an
 *  existing command, dialog or toolbar action — the menu bar
 *  rearranges the app, it adds nothing (except About, which is
 *  a local dialog). */
export interface MenuBarActions {
  add: () => void
  downloadAll: () => void
  batch: () => void
  resume: () => void
  stop: () => void
  stopAll: () => void
  deleteSelected: () => void
  deleteCompleted: () => void
  showHistory: () => void
  refresh: () => void
  startQueue: () => void
  stopQueue: () => void
  scheduler: () => void
  videoGrabber: () => void
  siteGrabber: () => void
  settings: () => void
  checkUpdates: () => void
  /** Round 74 (10) — File > Exit quits the whole app (the
   *  `exit_app` command), instead of hiding the main window to
   *  the tray like its X does. */
  exit: () => void
}

interface MenuBarProps {
  actions: MenuBarActions
  selected: DownloadRow | null
  queue: QueueRow | null
  hasDownloading: boolean
  hasComplete: boolean
}

function MenuItem({
  label,
  disabled,
  onClick,
}: {
  label: string
  disabled?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      onClick={onClick}
      className="flex w-full items-center rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2 disabled:opacity-45"
    >
      {label}
    </button>
  )
}

function MenuSeparator() {
  return <div role="separator" className="my-1 h-px bg-border" aria-hidden="true" />
}

/** Round 62 — the menu bar returns, under the custom title bar:
 *  File, Tasks, View, Downloads, Options, Help. No Registration
 *  (licence is set aside) and no "Tell a Friend" — both stay out
 *  by the user's earlier decisions. Tasks and Downloads mostly
 *  duplicate toolbar / sidebar actions on purpose: the menu bar
 *  is the discoverable path to them, the toolbar is the fast
 *  path. One menu open at a time; Escape or an outside click
 *  closes it. */
export function MenuBar({ actions, selected, queue, hasDownloading, hasComplete }: MenuBarProps) {
  const [openMenu, setOpenMenu] = useState<string | null>(null)
  const [showAbout, setShowAbout] = useState(false)

  useEffect(() => {
    if (openMenu === null) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpenMenu(null)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [openMenu])

  // Round 74 (14) — Escape closes the About dialog too (it had
  // only its Close button and a backdrop click before).
  useEffect(() => {
    if (!showAbout) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setShowAbout(false)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [showAbout])

  const canResume = selected !== null && (selected.status === 'paused' || selected.status === 'error')
  const canStop = selected !== null && selected.status === 'downloading'
  const canStartQueue = queue !== null && !queue.running && queue.file_count > 0
  const canStopQueue = queue !== null && queue.running

  function item(action: () => void) {
    return () => {
      setOpenMenu(null)
      action()
    }
  }

  const menus: { label: string; items: React.ReactNode }[] = [
    {
      label: 'File',
      items: (
        <>
          <MenuItem label="Add URL…" onClick={item(actions.add)} />
          <MenuItem label="Download All Links…" onClick={item(actions.downloadAll)} />
          <MenuItem label="Batch…" onClick={item(actions.batch)} />
          <MenuSeparator />
          <MenuItem
            label="Exit"
            onClick={() => {
              setOpenMenu(null)
              actions.exit()
            }}
          />
        </>
      ),
    },
    {
      label: 'Tasks',
      items: (
        <>
          <MenuItem label="Resume" disabled={!canResume} onClick={item(actions.resume)} />
          <MenuItem label="Pause" disabled={!canStop} onClick={item(actions.stop)} />
          <MenuItem label="Stop All" disabled={!hasDownloading} onClick={item(actions.stopAll)} />
          <MenuSeparator />
          <MenuItem label="Delete" disabled={!selected} onClick={item(actions.deleteSelected)} />
          <MenuItem
            label="Delete Completed"
            disabled={!hasComplete}
            onClick={item(actions.deleteCompleted)}
          />
        </>
      ),
    },
    {
      label: 'View',
      items: (
        <>
          <MenuItem label="Download History" onClick={item(actions.showHistory)} />
          <MenuItem label="Refresh" onClick={item(actions.refresh)} />
        </>
      ),
    },
    {
      label: 'Downloads',
      items: (
        <>
          <MenuItem label="Start Queue" disabled={!canStartQueue} onClick={item(actions.startQueue)} />
          <MenuItem label="Stop Queue" disabled={!canStopQueue} onClick={item(actions.stopQueue)} />
          <MenuItem label="Scheduler…" onClick={item(actions.scheduler)} />
          <MenuSeparator />
          <MenuItem label="Video Grabber…" onClick={item(actions.videoGrabber)} />
          <MenuItem label="Site Grabber…" onClick={item(actions.siteGrabber)} />
        </>
      ),
    },
    {
      label: 'Options',
      items: (
        <>
          <MenuItem label="Settings…" onClick={item(actions.settings)} />
        </>
      ),
    },
    {
      label: 'Help',
      items: (
        <>
          <MenuItem label="Check for Updates…" onClick={item(actions.checkUpdates)} />
          <MenuItem
            label="About Sonic Manager"
            onClick={() => {
              setOpenMenu(null)
              setShowAbout(true)
            }}
          />
        </>
      ),
    },
  ]

  return (
    <>
      <div className="relative flex shrink-0 items-center gap-0.5 border-b border-border bg-surface px-2 py-1">
        {menus.map((menu) => (
          <div key={menu.label} className="relative">
            <button
              type="button"
              aria-haspopup="menu"
              aria-expanded={openMenu === menu.label}
              onClick={() => setOpenMenu((v) => (v === menu.label ? null : menu.label))}
              onMouseEnter={() => {
                if (openMenu !== null) setOpenMenu(menu.label)
              }}
              className={`rounded px-2.5 py-1 text-[13px] text-text-muted hover:bg-surface-2 hover:text-text ${
                openMenu === menu.label ? 'bg-surface-2 text-text' : ''
              }`}
            >
              {menu.label}
            </button>
            {openMenu === menu.label ? (
              <>
                <button
                  type="button"
                  aria-label="Close menu"
                  tabIndex={-1}
                  onClick={() => setOpenMenu(null)}
                  className="fixed inset-0 z-10 cursor-default bg-transparent"
                />
                <div
                  role="menu"
                  aria-label={menu.label}
                  className="absolute left-0 z-20 mt-1 min-w-60 rounded-md border border-border bg-surface-raised p-1 shadow-pop"
                >
                  {menu.items}
                </div>
              </>
            ) : null}
          </div>
        ))}
      </div>
      {showAbout ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="about-title"
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
        >
          <div className="w-full max-w-sm rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
            <h2 id="about-title" className="text-lg font-semibold">
              Sonic Manager
            </h2>
            <p className="mt-2 text-sm text-text-muted">
              Version 0.1.0 — a fast download manager for Windows. Downloads run in up to 16
              parts, resume after interruptions, and live in queues you can schedule.
            </p>
            <p className="mt-2 text-sm text-text-muted">
              Video downloads use yt-dlp, QuickJS-ng and FFmpeg (LGPL) — see
              THIRD_PARTY_NOTICES.md next to the program for versions, licenses and sources.
            </p>
            <div className="mt-6 flex justify-end">
              <Button variant="primary" onClick={() => setShowAbout(false)}>
                Close
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  )
}
