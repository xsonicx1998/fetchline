import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import type { UpdateCheckResult } from '../lib/types'
import { noteOwnClipboardWrite } from '../lib/api'
import { Button } from './Button'

interface UpdateDialogProps {
  result: UpdateCheckResult | null
  /** A failed manual check: the message to show instead of a result. */
  checkError: string | null
  checking: boolean
  onClose: () => void
}

/** The update dialog (S09, "Quick update check"). It asks before
 *  updating — always. It never installs anything: when a newer
 *  version is available it lists the version and the changelog,
 *  and the user picks "Remind me later" or copies the installer
 *  address. There is no in-app install yet — the Tauri updater
 *  plugin and a signed release pipeline are still to be wired
 *  (see engine/src/update.rs) — so no Install button pretends
 *  otherwise. */
export function UpdateDialog({ result, checkError, checking, onClose }: UpdateDialogProps) {
  const [copied, setCopied] = useState(false)

  // Round 74 (14) — Escape closes the dialog, like the Close /
  // "Remind me later" buttons. (It had neither a backdrop close
  // — removed in Round 74 (7) — nor a key close before.)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  if (!result && !checkError && !checking) return null

  async function copyDownloadUrl(url: string) {
    try {
      await navigator.clipboard.writeText(url)
      // Round 75 — this write is OURS: record the clipboard
      // sequence right after, so the monitor does not offer the
      // installer URL back as a download. A failure to record
      // only risks one extra offer, never a lost download.
      void noteOwnClipboardWrite().catch(() => undefined)
      setCopied(true)
    } catch {
      // Clipboard unavailable (e.g. outside the desktop app):
      // the address is still shown in the readonly field below.
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="update-title"
      className="fixed inset-0 z-20 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="w-full max-w-md rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="flex items-start justify-between gap-4">
          <h2 id="update-title" className="text-lg font-semibold">
            {checking
              ? 'Checking for updates…'
              : checkError
                ? 'Could not check for updates'
                : result?.update_available
                  ? `Sonic Manager ${result.latest_version} is available`
                  : 'Sonic Manager is up to date'}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted transition-colors hover:bg-surface-2 hover:text-text"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        {checking ? (
          <p className="mt-2 text-sm text-text-muted">
            Asking the update server for the latest version…
          </p>
        ) : checkError ? (
          <div>
            <p className="mt-2 text-sm text-text-muted">{checkError}</p>
            <p className="mt-2 text-sm text-text-muted">
              Nothing was installed or changed. Sonic Manager will try again at the next weekly
              check.
            </p>
          </div>
        ) : result ? (
          <div>
            <p className="mt-2 text-sm text-text-muted">
              You have version {result.current_version}
              {result.update_available
                ? ` — version ${result.latest_version} is available.`
                : ` — that is the latest version.`}
            </p>
            {result.update_available && result.changelog.length > 0 && (
              <div className="mt-4">
                <h3 className="text-sm font-semibold">What&apos;s new in {result.latest_version}</h3>
                <ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-text-muted">
                  {result.changelog.map((change) => (
                    <li key={change}>{change}</li>
                  ))}
                </ul>
              </div>
            )}
            {result.update_available && result.changelog.length === 0 && (
              <p className="mt-4 text-sm text-text-muted">
                The update server did not list what changed in this version.
              </p>
            )}
            {result.update_available && (
              <div className="mt-4">
                <p className="text-sm text-text-muted">
                  Sonic Manager never installs an update without asking. To update, download the
                  installer from:
                </p>
                {result.download_url ? (
                  <div className="mt-2 flex gap-2">
                    <input
                      type="text"
                      readOnly
                      value={result.download_url}
                      aria-label="Installer address"
                      className="h-8 min-w-0 flex-1 rounded-md border border-border-input bg-bg px-2 font-mono text-xs text-text"
                      onFocus={(e) => e.target.select()}
                    />
                    <Button variant="secondary" size="sm" onClick={() => void copyDownloadUrl(result.download_url!)}>
                      {copied ? 'Copied' : 'Copy'}
                    </Button>
                  </div>
                ) : (
                  <p className="mt-2 text-sm text-text-muted">
                    The update server did not give an installer address — check the Sonic Manager
                    release page for version {result.latest_version}.
                  </p>
                )}
                {!result.updater_configured && (
                  <p className="mt-2 text-sm text-text-muted">
                    In-app installing is not set up yet: the updater needs a signed release
                    pipeline first. Until then, run the downloaded installer yourself.
                  </p>
                )}
              </div>
            )}
            <p className="mt-4 text-xs text-text-muted">
              Checked {new Date(result.checked_at).toLocaleString()} from {result.source_url}
            </p>
          </div>
        ) : null}

        <div className="mt-6 flex justify-end gap-2">
          {result?.update_available ? (
            <Button variant="ghost" onClick={onClose}>
              Remind me later
            </Button>
          ) : (
            <Button variant="primary" onClick={onClose}>
              Close
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
