import { useEffect, useRef, useState } from 'react'
import { Check, Moon, Sun } from 'lucide-react'
import type { CategoryRow, DownloadRow, QueueRow } from '../lib/types'
import logoUrl from '../assets/logo.jpg'

/** The actions the menu bar offers. Every entry wires to an existing
 *  command or dialog — the menu rearranges the app, it adds nothing. */
export interface MenuBarActions {
  add: () => void
  batch: () => void
  downloadAll: () => void
  resume: () => void
  stop: () => void
  stopAll: () => void
  deleteSelected: () => void
  deleteCompleted: () => void
  openFolder: () => void
  remove: () => void
  settings: () => void
  videoGrabber: () => void
  siteGrabber: () => void
  startQueue: () => void
  stopQueue: () => void
  syncNow: () => void
  openProgress: () => void
  moveToCategory: (categoryId: string) => void
  checkUpdates: () => void
  toggleTheme: () => void
}

interface MenuBarProps {
  actions: MenuBarActions
  /** The download the toolbar/menus act on, if one is selected. */
  selected: DownloadRow | null
  categories: CategoryRow[]
  /** The queue Start/Stop/Sync act on: the selected queue, else the
   *  main queue. Null only when the app has no queue at all. */
  queue: QueueRow | null
  hasDownloading: boolean
  hasComplete: boolean
  theme: 'light' | 'dark'
  statusText: string
}

type MenuId = 'tasks' | 'file' | 'downloads' | 'view' | 'help'

function MenuItem({
  disabled,
  onClick,
  checked,
  children,
}: {
  disabled?: boolean
  onClick: () => void
  checked?: boolean
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      onClick={onClick}
      className="flex w-full items-center justify-between gap-4 rounded px-3 py-1.5 text-left text-sm text-text hover:bg-surface-2 disabled:opacity-45"
    >
      <span>{children}</span>
      {checked ? <Check className="h-4 w-4 shrink-0 text-accent" aria-hidden="true" /> : null}
    </button>
  )
}

function MenuSeparator() {
  return <div role="separator" className="my-1 h-px bg-border" aria-hidden="true" />
}

function MenuLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="px-3 pb-1 pt-2 text-xs font-semibold uppercase text-text-muted">{children}</p>
  )
}

/** S01 — menu bar, IDM's arrangement (Tasks, File, Downloads, View,
 *  Help), Sonic Manager's actions. No Registration menu — the
 *  licence (S11) is set aside by the user's standing instruction.
 *  One menu open at a time; Escape or clicking outside closes it;
 *  hovering another top-level entry while one is open switches. */
export function MenuBar({ actions, selected, categories, queue, hasDownloading, hasComplete, theme, statusText }: MenuBarProps) {
  const [openMenu, setOpenMenu] = useState<MenuId | null>(null)
  const navRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    if (openMenu === null) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpenMenu(null)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [openMenu])

  const canResume = selected !== null && (selected.status === 'paused' || selected.status === 'error')
  const canStop = selected !== null && selected.status === 'downloading'
  const canOpenFolder = selected !== null && selected.status === 'complete'
  const canStartQueue = queue !== null && !queue.running && queue.file_count > 0
  const canStopQueue = queue !== null && queue.running

  function item(action: () => void) {
    return () => {
      setOpenMenu(null)
      action()
    }
  }

  function menuButton(id: MenuId, label: string) {
    const open = openMenu === id
    return (
      <div className="relative">
        <button
          type="button"
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => setOpenMenu(open ? null : id)}
          onMouseEnter={() => {
            if (openMenu !== null) setOpenMenu(id)
          }}
          className={`rounded px-2.5 py-1.5 text-sm font-medium ${
            open ? 'bg-surface-2 text-text' : 'text-text hover:bg-surface-2'
          }`}
        >
          {label}
        </button>
        {open ? (
          <div
            role="menu"
            aria-label={label}
            className="absolute left-0 z-20 mt-1 min-w-60 rounded-md border border-border bg-surface-raised p-1 shadow-pop"
          >
            {id === 'tasks' ? (
              <>
                <MenuItem onClick={item(actions.add)}>Add URL…</MenuItem>
                <MenuItem onClick={item(actions.batch)}>Add batch…</MenuItem>
                <MenuItem onClick={item(actions.downloadAll)}>Download all links…</MenuItem>
                <MenuSeparator />
                <MenuItem disabled={!canResume} onClick={item(actions.resume)}>
                  Resume
                </MenuItem>
                <MenuItem disabled={!canStop} onClick={item(actions.stop)}>
                  Stop
                </MenuItem>
                <MenuItem disabled={!hasDownloading} onClick={item(actions.stopAll)}>
                  Stop all
                </MenuItem>
                <MenuSeparator />
                <MenuItem disabled={!selected} onClick={item(actions.deleteSelected)}>
                  Delete
                </MenuItem>
                <MenuItem disabled={!hasComplete} onClick={item(actions.deleteCompleted)}>
                  Delete completed
                </MenuItem>
              </>
            ) : null}
            {id === 'file' ? (
              <>
                <MenuItem disabled={!canOpenFolder} onClick={item(actions.openFolder)}>
                  Open folder
                </MenuItem>
                <MenuItem disabled={!selected} onClick={item(actions.remove)}>
                  Remove selected
                </MenuItem>
                <MenuSeparator />
                <MenuItem onClick={item(actions.settings)}>Settings…</MenuItem>
              </>
            ) : null}
            {id === 'downloads' ? (
              <>
                <MenuItem onClick={item(actions.videoGrabber)}>Video grabber…</MenuItem>
                <MenuItem onClick={item(actions.siteGrabber)}>Site grabber…</MenuItem>
                <MenuSeparator />
                <MenuLabel>Move to category</MenuLabel>
                {categories.map((c) => (
                  <MenuItem
                    key={c.id}
                    disabled={!selected}
                    onClick={item(() => actions.moveToCategory(c.id))}
                  >
                    {c.name}
                  </MenuItem>
                ))}
                <MenuSeparator />
                <MenuItem disabled={!canStartQueue} onClick={item(actions.startQueue)}>
                  Start queue{queue ? ` — ${queue.name}` : ''}
                </MenuItem>
                <MenuItem disabled={!canStopQueue} onClick={item(actions.stopQueue)}>
                  Stop queue{queue ? ` — ${queue.name}` : ''}
                </MenuItem>
                <MenuItem disabled={!queue} onClick={item(actions.syncNow)}>
                  Sync queue now
                </MenuItem>
              </>
            ) : null}
            {id === 'view' ? (
              <>
                <MenuItem checked={theme === 'dark'} onClick={item(actions.toggleTheme)}>
                  Dark theme
                </MenuItem>
                <MenuSeparator />
                <MenuItem disabled={!selected} onClick={item(actions.openProgress)}>
                  Progress window
                </MenuItem>
              </>
            ) : null}
            {id === 'help' ? (
              <>
                <MenuItem onClick={item(actions.checkUpdates)}>Check for updates</MenuItem>
                <MenuItem onClick={item(actions.settings)}>Settings…</MenuItem>
              </>
            ) : null}
          </div>
        ) : null}
      </div>
    )
  }

  return (
    <nav
      ref={navRef}
      aria-label="Main menu"
      className="relative flex h-9 shrink-0 items-center gap-0.5 border-b border-border bg-surface px-2"
    >
      {openMenu !== null ? (
        <button
          type="button"
          aria-label="Close menu"
          tabIndex={-1}
          onClick={() => setOpenMenu(null)}
          className="fixed inset-0 z-10 cursor-default bg-transparent"
        />
      ) : null}
      <img
        src={logoUrl}
        alt="Sonic Manager logo"
        className="mr-1 h-7 w-7 shrink-0 rounded-md object-cover"
      />
      {menuButton('tasks', 'Tasks')}
      {menuButton('file', 'File')}
      {menuButton('downloads', 'Downloads')}
      {menuButton('view', 'View')}
      {menuButton('help', 'Help')}
      <div className="ml-auto flex items-center gap-2">
        <span className="whitespace-nowrap text-sm text-text-muted">{statusText}</span>
        <button
          type="button"
          onClick={actions.toggleTheme}
          aria-label={theme === 'light' ? 'Switch to dark theme' : 'Switch to light theme'}
          title={theme === 'light' ? 'Switch to dark theme' : 'Switch to light theme'}
          className="rounded-md p-1.5 text-text-muted hover:bg-surface-2 hover:text-text"
        >
          {theme === 'light' ? (
            <Moon className="h-4 w-4" aria-hidden="true" />
          ) : (
            <Sun className="h-4 w-4" aria-hidden="true" />
          )}
        </button>
      </div>
    </nav>
  )
}
