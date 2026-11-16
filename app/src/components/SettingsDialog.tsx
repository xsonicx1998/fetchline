import { useEffect, useState } from 'react'
import { X } from 'lucide-react'
import {
  deleteSiteLogin,
  getProxyConfig,
  getQuotaStatus,
  getSettings,
  getSoundSettings,
  listSiteLogins,
  resetQuota,
  saveProxyConfig,
  saveSiteLogin,
  saveSoundSettings,
  testAntivirusScanner,
  testProxy,
  testSoundFile,
  updateCategory,
  updateSettings,
} from '../lib/api'
import type {
  CategoryRow,
  ProxyConfigInput,
  ProxyConfigRow,
  QuotaStatus,
  Settings,
  SiteLoginRow,
  SoundSetting,
} from '../lib/types'
import { Button } from './Button'

type Tab = 'general' | 'filetypes' | 'connection' | 'saveto' | 'downloads' | 'proxy' | 'logins' | 'dialup' | 'sounds'

const TABS: { id: Tab; label: string }[] = [
  { id: 'general', label: 'General' },
  { id: 'filetypes', label: 'File Types' },
  { id: 'connection', label: 'Connection' },
  { id: 'saveto', label: 'Save To' },
  { id: 'downloads', label: 'Downloads' },
  { id: 'proxy', label: 'Proxy' },
  { id: 'logins', label: 'Site logins' },
  { id: 'dialup', label: 'Dial-Up' },
  { id: 'sounds', label: 'Sounds' },
]

/** One staged site-login edit: the password starts empty (keep the
 *  stored one) and is only sent when the user types one. */
interface LoginDraft {
  host: string
  username: string
  password: string
  passwordTouched: boolean
}

const SOUND_LABELS: Record<string, string> = {
  download_complete: 'Download complete',
  download_failed: 'Download failed',
  queue_finished: 'Queue finished',
}

interface SettingsDialogProps {
  open: boolean
  categories: CategoryRow[]
  onClose: () => void
  /** The parent reloads settings and categories after a save. */
  onSaved: () => void
  /** Manual update check (was Help > Check for updates in the menu
   *  bar; the menu bar is gone in Round 45). */
  onCheckUpdates: () => void
}

function emptyProxy(): ProxyConfigInput {
  return {
    proxy_type: 'http',
    host: '',
    port: 8080,
    username: null,
    password: null,
    use_for: [],
    bypass: [],
    enabled: false,
    auth: 'negotiate-auto',
  }
}

/** S09 — Options dialog, all 9 tabs (components.md: Dialog, Tabs).
 *  General, File Types, Connection, Save To, Downloads, Proxy,
 *  Site logins, Dial-Up and Sounds. Everything is staged in a
 *  draft and saved at once; the engine validates and applies the
 *  save live (no restart). Passwords are never read back: the
 *  dialog only knows whether one is stored, and re-typing it is
 *  needed to change it. Site-login passwords go through the same
 *  protection as proxy passwords (DPAPI on Windows,
 *  base64-obfuscated on this Linux build — see build-log.md).
 *  Dial-Up is legacy: only "hang up after completion" has an
 *  engine behaviour; auto-dial/redial are stored, and Windows
 *  dial-up (RAS) execution itself is unproven on this VM. Sounds
 *  play on Windows only; the Test button validates the WAV file
 *  here and reports that plainly. */
export function SettingsDialog({ open, categories, onClose, onSaved, onCheckUpdates }: SettingsDialogProps) {
  const [tab, setTab] = useState<Tab>('general')
  const [settings, setSettings] = useState<Settings | null>(null)
  const [loading, setLoading] = useState(false)
  const [catEdits, setCatEdits] = useState<Record<string, { folder: string; extensions: string }>>({})
  const [proxy, setProxy] = useState<ProxyConfigInput>(emptyProxy())
  const [proxyRow, setProxyRow] = useState<ProxyConfigRow | null>(null)
  const [password, setPassword] = useState('')
  const [passwordTouched, setPasswordTouched] = useState(false)
  const [testUrl, setTestUrl] = useState('http://example.com/')
  const [testResult, setTestResult] = useState<string | null>(null)
  const [testing, setTesting] = useState(false)
  const [logins, setLogins] = useState<SiteLoginRow[]>([])
  const [loginEdits, setLoginEdits] = useState<Record<string, LoginDraft>>({})
  const [deletedLoginIds, setDeletedLoginIds] = useState<string[]>([])
  const [newLogin, setNewLogin] = useState<LoginDraft>({ host: '', username: '', password: '', passwordTouched: false })
  const [showLoginPasswords, setShowLoginPasswords] = useState<Record<string, boolean>>({})
  const [sounds, setSounds] = useState<SoundSetting[]>([])
  const [soundEdits, setSoundEdits] = useState<Record<string, { enabled: boolean; file: string }>>({})
  const [testingSound, setTestingSound] = useState<string | null>(null)
  const [soundTestResult, setSoundTestResult] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** Download quota (S12): the live window, loaded with the rest
   *  of the settings when the dialog opens. */
  const [quotaStatus, setQuotaStatus] = useState<QuotaStatus | null>(null)
  const [resettingQuota, setResettingQuota] = useState(false)
  /** Antivirus hook (S14): the Test button's result. */
  const [avTestResult, setAvTestResult] = useState<string | null>(null)
  const [testingAv, setTestingAv] = useState(false)

  useEffect(() => {
    if (!open) return
    setTab('general')
    setError(null)
    setTestResult(null)
    setSoundTestResult(null)
    setAvTestResult(null)
    setPassword('')
    setPasswordTouched(false)
    setDeletedLoginIds([])
    setNewLogin({ host: '', username: '', password: '', passwordTouched: false })
    setShowLoginPasswords({})
    setLoading(true)
    const edits: Record<string, { folder: string; extensions: string }> = {}
    for (const c of categories) {
      edits[c.id] = { folder: c.default_folder, extensions: c.file_extensions.join(', ') }
    }
    setCatEdits(edits)
    void Promise.all([getSettings(), getProxyConfig(), listSiteLogins(), getSoundSettings(), getQuotaStatus()])
      .then(([s, p, loginsList, soundList, quota]) => {
        setSettings(s)
        setQuotaStatus(quota)
        setLogins(loginsList)
        const loginDrafts: Record<string, LoginDraft> = {}
        for (const l of loginsList) {
          loginDrafts[l.id] = { host: l.host, username: l.username, password: '', passwordTouched: false }
        }
        setLoginEdits(loginDrafts)
        setSounds(soundList)
        const soundDrafts: Record<string, { enabled: boolean; file: string }> = {}
        for (const snd of soundList) {
          soundDrafts[snd.event] = { enabled: snd.enabled, file: snd.file ?? '' }
        }
        setSoundEdits(soundDrafts)
        setProxyRow(p)
        if (p) {
          setProxy({
            proxy_type: p.proxy_type,
            host: p.host,
            port: p.port,
            username: p.username,
            password: null,
            use_for: p.use_for,
            bypass: p.bypass,
            enabled: p.enabled,
            auth: p.auth,
          })
        } else {
          setProxy(emptyProxy())
        }
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false))
  }, [open, categories])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  function set<T extends keyof Settings>(key: T, value: Settings[T]) {
    setSettings((prev) => (prev ? { ...prev, [key]: value } : prev))
  }

  function validate(): string | null {
    if (!settings) return 'Settings are still loading.'
    if (settings.max_connections < 1 || settings.max_connections > 16) {
      return 'Connections per download must be between 1 and 16.'
    }
    if (settings.connection_timeout_secs < 1 || settings.connection_timeout_secs > 300) {
      return 'The connection timeout must be between 1 and 300 seconds.'
    }
    if (settings.max_retries < 0 || settings.max_retries > 10) {
      return 'Retries must be between 0 and 10.'
    }
    if ((settings.speed_limit_kbps ?? 0) < 0) {
      return 'The speed limit cannot be negative — use 0 for unlimited.'
    }
    // Same bound as the engine (BUG-008, MAX_SPEED_LIMIT_KBPS):
    // larger values overflow the bytes-per-second conversion.
    if ((settings.speed_limit_kbps ?? 0) > 10_000_000) {
      return 'The speed limit cannot be more than 10,000,000 KB/s.'
    }
    // Download quota (S12): same bounds as the engine
    // (MIN/MAX_QUOTA_MB, MIN/MAX_QUOTA_HOURS).
    if (settings.quota_enabled && (settings.quota_mb == null || settings.quota_hours == null)) {
      return 'Turn the quota on with both a size in MB and a window in hours.'
    }
    if (settings.quota_mb != null && (settings.quota_mb < 1 || settings.quota_mb > 1_000_000)) {
      return 'The download quota must be between 1 and 1,000,000 MB.'
    }
    if (settings.quota_hours != null && (settings.quota_hours < 1 || settings.quota_hours > 720)) {
      return 'The quota window must be between 1 and 720 hours.'
    }
    // Antivirus hook (S14): same bounds as the engine
    // (MIN/MAX_SCAN_TIMEOUT_SECS, 5–600 s).
    if (settings.antivirus_timeout_secs < 5 || settings.antivirus_timeout_secs > 600) {
      return 'The antivirus scan timeout must be between 5 and 600 seconds.'
    }
    if (proxy.enabled) {
      if (!proxy.host.trim()) return 'Give the proxy a host.'
      if (proxy.port < 1 || proxy.port > 65535) return 'The proxy port must be between 1 and 65535.'
    }
    if (settings.dialup_redial_attempts < 0 || settings.dialup_redial_attempts > 10) {
      return 'Redial attempts must be between 0 and 10.'
    }
    for (const l of logins) {
      const draft = loginEdits[l.id]
      if (!draft) continue
      if (!draft.host.trim()) return 'Give every site login a host.'
      if (!draft.username.trim()) return 'Give every site login a username.'
    }
    if (newLogin.host.trim() || newLogin.username.trim() || newLogin.password) {
      if (!newLogin.host.trim()) return 'Give the new site login a host.'
      if (!newLogin.username.trim()) return 'Give the new site login a username.'
      if (!newLogin.password) return 'Give the new site login a password.'
    }
    for (const snd of sounds) {
      const draft = soundEdits[snd.event]
      if (!draft) continue
      const file = draft.file.trim()
      if (file && !file.toLowerCase().endsWith('.wav')) {
        return `The sound for “${SOUND_LABELS[snd.event] ?? snd.event}” must be a WAV file.`
      }
    }
    return null
  }

  async function save() {
    const problem = validate()
    if (problem) {
      setError(problem)
      return
    }
    if (!settings) return
    setBusy(true)
    setError(null)
    try {
      await updateSettings(settings)
      for (const c of categories) {
        const edit = catEdits[c.id]
        if (!edit) continue
        const folder = edit.folder.trim()
        const exts = edit.extensions
          .split(',')
          .map((e) => e.trim().replace(/^\./, '').toLowerCase())
          .filter((e) => e.length > 0)
        const folderChanged = folder !== c.default_folder
        const extChanged =
          JSON.stringify([...exts].sort()) !== JSON.stringify([...c.file_extensions].sort())
        if (!folderChanged && !extChanged) continue
        if (extChanged && exts.length === 0) {
          throw new Error(`“${c.name}” needs at least one file type.`)
        }
        await updateCategory({
          id: c.id,
          folder: folderChanged ? folder : null,
          extensions: extChanged ? exts : null,
        })
      }
      // Only save a proxy when one exists or the user enabled one —
      // otherwise an empty, disabled form would fail validation.
      if (proxyRow !== null || proxy.enabled) {
        await saveProxyConfig({
          ...proxy,
          password: passwordTouched ? password : null,
        })
      }
      // Site logins: deletions first, then changed rows, then the
      // new login (if the user started one). A row whose password
      // field is empty keeps the stored password.
      for (const id of deletedLoginIds) {
        await deleteSiteLogin(id)
      }
      for (const l of logins) {
        const draft = loginEdits[l.id]
        if (!draft) continue
        const hostChanged = draft.host.trim().toLowerCase() !== l.host
        const userChanged = draft.username.trim() !== l.username
        if (!hostChanged && !userChanged && !draft.passwordTouched) continue
        await saveSiteLogin({
          id: l.id,
          host: draft.host.trim(),
          username: draft.username.trim(),
          password: draft.passwordTouched ? draft.password : null,
          auth_type: 'basic',
        })
      }
      if (newLogin.host.trim()) {
        await saveSiteLogin({
          id: null,
          host: newLogin.host.trim(),
          username: newLogin.username.trim(),
          password: newLogin.password,
          auth_type: 'basic',
        })
      }
      // Sounds: all three events, as staged.
      if (sounds.length > 0) {
        await saveSoundSettings(
          sounds.map((snd) => ({
            event: snd.event,
            enabled: soundEdits[snd.event]?.enabled ?? snd.enabled,
            file: soundEdits[snd.event]?.file.trim() ? soundEdits[snd.event].file.trim() : null,
          })),
        )
      }
      onSaved()
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function runTest() {
    setTesting(true)
    setTestResult(null)
    setError(null)
    try {
      const status = await testProxy(
        { ...proxy, password: passwordTouched ? password : null },
        testUrl.trim() || 'http://example.com/',
      )
      setTestResult(`OK — the proxy answered with status ${status}.`)
    } catch (e) {
      setTestResult(`Failed — ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setTesting(false)
    }
  }

  async function runSoundTest(event: string) {
    const file = soundEdits[event]?.file.trim() ?? ''
    if (!file) {
      setSoundTestResult('Pick a WAV file first.')
      return
    }
    setTestingSound(event)
    setSoundTestResult(null)
    try {
      await testSoundFile(file)
      setSoundTestResult(`Played “${file}”.`)
    } catch (e) {
      setSoundTestResult(`Could not play it — ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setTestingSound(null)
    }
  }

  /** Antivirus hook (S14): run the command typed in the dialog
   *  against a small throwaway file and report what the scanner
   *  said. The command does not have to be saved first. */
  async function runAvTest() {
    const command = settings?.antivirus_command ?? ''
    if (!command.trim()) {
      setAvTestResult('Enter a scanner command first.')
      return
    }
    setTestingAv(true)
    setAvTestResult(null)
    try {
      const result = await testAntivirusScanner(command, settings?.antivirus_timeout_secs ?? 60)
      if (result.status === 'clean') {
        setAvTestResult('Clean — the scanner reported no threat in the test file.')
      } else if (result.status === 'infected') {
        setAvTestResult(`Threat found — ${result.detail ?? 'the scanner reported a threat.'}`)
      } else if (result.status === 'timed_out') {
        setAvTestResult(`Timed out — ${result.detail ?? 'the scanner did not finish in time.'}`)
      } else {
        setAvTestResult(`Failed — ${result.detail ?? 'the scan could not run.'}`)
      }
    } catch (e) {
      setAvTestResult(`Failed — ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setTestingAv(false)
    }
  }

  const inputCls =
    'h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent'
  const labelCls = 'mb-1 block text-sm font-semibold'
  const hintCls = 'mt-1 text-xs text-text-muted'

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="flex max-h-full w-full max-w-2xl flex-col rounded-lg border border-border bg-surface-raised shadow-pop">
        <div className="flex shrink-0 items-start justify-between gap-4 px-6 pt-6">
          <h2 id="settings-title" className="text-lg font-semibold">
            Settings
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-text-muted hover:bg-surface-2"
          >
            <X className="h-5 w-5" aria-hidden="true" />
          </button>
        </div>

        <div role="tablist" aria-label="Settings sections" className="flex shrink-0 gap-1 border-b border-border px-4 pt-2">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={tab === t.id}
              onClick={() => setTab(t.id)}
              className={`rounded-t-md px-3 py-2 text-base font-semibold ${
                tab === t.id ? 'bg-surface-2 text-accent' : 'text-text-muted hover:bg-surface-2'
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>

        <div className="min-h-0 flex-1 overflow-auto px-6 py-4">
          {loading ? (
            <p className="text-sm text-text-muted">Loading settings…</p>
          ) : !settings ? (
            <p role="alert" className="text-sm text-danger">
              {error ?? 'Settings could not be loaded.'}
            </p>
          ) : tab === 'general' ? (
            <div className="flex flex-col gap-4">
              <div>
                <label htmlFor="set-folder" className={labelCls}>
                  Default download folder
                </label>
                <input
                  id="set-folder"
                  type="text"
                  value={settings.default_download_folder}
                  onChange={(e) => set('default_download_folder', e.target.value)}
                  placeholder="Leave empty for your Downloads folder"
                  className={`${inputCls} font-mono`}
                />
                <p className={hintCls}>
                  Used when a download has no folder of its own. A category&apos;s folder (Save To tab) wins
                  over this one.
                </p>
              </div>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.start_on_startup}
                  onChange={(e) => set('start_on_startup', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Start Sonic Manager when Windows starts
              </label>
              <p className={hintCls}>
                Stored for now — registering the app in Windows startup is part of the installed
                version, not this build.
              </p>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.browser_integration}
                  onChange={(e) => set('browser_integration', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Catch downloads from the browser
              </label>
              <p className={hintCls}>
                Takes effect once the browser extension is installed. When it is off, the
                extension catches nothing and drained links are rejected.
              </p>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.browser_start_drained}
                  onChange={(e) => set('browser_start_drained', e.target.checked)}
                  disabled={!settings.browser_integration}
                  className="h-4 w-4 accent-accent"
                />
                Start browser-caught downloads at once
              </label>
              <p className={hintCls}>
                On (the default), a link the browser hands over starts downloading as soon as
                Sonic Manager picks it up. Off leaves it queued in the download list, to start by
                hand. Only applies while catching downloads from the browser is on.
              </p>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.show_progress_window}
                  onChange={(e) => set('show_progress_window', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Show the download progress window when a download starts
              </label>
              <p className={hintCls}>
                On (the default), each download that starts opens its own small window with its
                speed, time left and parts. Closing the window never stops the download, and
                double-clicking a download opens its window again. Off leaves the table as the
                only progress view.
              </p>
              <div>
                <label htmlFor="set-excluded" className={labelCls}>
                  Excluded sites
                </label>
                <input
                  id="set-excluded"
                  type="text"
                  value={settings.excluded_sites.join(', ')}
                  onChange={(e) =>
                    set(
                      'excluded_sites',
                      e.target.value
                        .split(',')
                        .map((t) => t.trim().toLowerCase())
                        .filter((t) => t.length > 0),
                    )
                  }
                  placeholder="example.com, ads.example.net"
                  className={`${inputCls} font-mono`}
                />
                <p className={hintCls}>
                  Sonic Manager will not download from these sites, or their subdomains. Downloads,
                  batches and Download All all honour this list.
                </p>
              </div>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.check_updates_weekly}
                  onChange={(e) => set('check_updates_weekly', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Check for updates weekly
              </label>
              <p className={hintCls}>
                When on, Sonic Manager checks the update server once a week (on the scheduler&apos;s
                regular tick) and asks before updating — nothing is ever installed
                automatically. Turning it off stops the automatic check; the button below
                still works.
              </p>
              <div>
                <label htmlFor="set-update-url" className={labelCls}>
                  Update server address
                </label>
                <input
                  id="set-update-url"
                  type="text"
                  value={settings.update_url}
                  onChange={(e) => set('update_url', e.target.value)}
                  placeholder="https://updates.fetchline.app/releases/latest.json"
                  className={`${inputCls} font-mono`}
                />
                <p className={hintCls}>
                  Sonic Manager&apos;s own release server — the check fetches a version list from
                  here. The address above is a placeholder: replace it with the real release
                  server before shipping. Last checked:{' '}
                  {settings.last_update_check_at
                    ? new Date(settings.last_update_check_at).toLocaleString()
                    : 'never'}
                  .
                </p>
                <button
                  type="button"
                  onClick={onCheckUpdates}
                  className="mt-2 inline-flex h-9 items-center rounded-md bg-accent px-3 text-sm font-semibold text-on-accent shadow-glow hover:bg-accent-hover"
                >
                  Check for updates now
                </button>
              </div>
              <div>
                <label htmlFor="set-language" className={labelCls}>
                  Language
                </label>
                <select
                  id="set-language"
                  value={settings.language}
                  onChange={(e) => set('language', e.target.value)}
                  className={inputCls}
                >
                  <option value="en">English</option>
                </select>
                <p className={hintCls}>More languages are planned for a later version.</p>
              </div>
            </div>
          ) : tab === 'filetypes' ? (
            <div className="flex flex-col gap-4">
              <p className="text-sm text-text-muted">
                File types decide which category a new download lands in. Edit a category&apos;s list and
                new downloads with those types are filed there automatically.
              </p>
              {categories.map((c) => (
                <div key={c.id}>
                  <label htmlFor={`set-ext-${c.id}`} className={labelCls}>
                    {c.name}
                  </label>
                  <input
                    id={`set-ext-${c.id}`}
                    type="text"
                    value={catEdits[c.id]?.extensions ?? c.file_extensions.join(', ')}
                    onChange={(e) =>
                      setCatEdits((prev) => ({
                        ...prev,
                        [c.id]: { folder: prev[c.id]?.folder ?? c.default_folder, extensions: e.target.value },
                      }))
                    }
                    placeholder="mp4, mkv"
                    className={`${inputCls} font-mono`}
                  />
                </div>
              ))}
              <div>
                <label htmlFor="set-auto-types" className={labelCls}>
                  Auto-assign file types
                </label>
                <input
                  id="set-auto-types"
                  type="text"
                  value={settings.file_types_auto.join(', ')}
                  onChange={(e) =>
                    set(
                      'file_types_auto',
                      e.target.value
                        .split(',')
                        .map((t) => t.trim().replace(/^\./, '').toLowerCase())
                        .filter((t) => t.length > 0),
                    )
                  }
                  placeholder="Leave empty to auto-assign every file type"
                  className={`${inputCls} font-mono`}
                />
                <p className={hintCls}>
                  Only these file types are filed into a category automatically. Leave empty to
                  auto-assign every type; anything else goes to Documents.
                </p>
              </div>
            </div>
          ) : tab === 'connection' ? (
            <div className="flex flex-col gap-4">
              <div>
                <label htmlFor="set-connections" className={labelCls}>
                  Connections per download <span className="font-normal text-text-muted">(1–16)</span>
                </label>
                <input
                  id="set-connections"
                  type="number"
                  min={1}
                  max={16}
                  value={settings.max_connections}
                  onChange={(e) => set('max_connections', Number(e.target.value) || 0)}
                  className={inputCls}
                />
                <p className={hintCls}>
                  How many parts a file is split into. Applies to the next download you start.
                </p>
              </div>
              <div>
                <label htmlFor="set-timeout" className={labelCls}>
                  Connection timeout <span className="font-normal text-text-muted">(seconds, 1–300)</span>
                </label>
                <input
                  id="set-timeout"
                  type="number"
                  min={1}
                  max={300}
                  value={settings.connection_timeout_secs}
                  onChange={(e) => set('connection_timeout_secs', Number(e.target.value) || 0)}
                  className={inputCls}
                />
              </div>
              <div>
                <label htmlFor="set-retries" className={labelCls}>
                  Retries per part <span className="font-normal text-text-muted">(0–10)</span>
                </label>
                <input
                  id="set-retries"
                  type="number"
                  min={0}
                  max={10}
                  value={settings.max_retries}
                  onChange={(e) => set('max_retries', Number(e.target.value) || 0)}
                  className={inputCls}
                />
                <p className={hintCls}>How many times a failed part is retried before the download errors.</p>
              </div>
              <div>
                <label htmlFor="set-speed" className={labelCls}>
                  Speed limit <span className="font-normal text-text-muted">(KB/s, 0 = unlimited, max 10,000,000)</span>
                </label>
                <input
                  id="set-speed"
                  type="number"
                  min={0}
                  max={10000000}
                  value={settings.speed_limit_kbps ?? 0}
                  onChange={(e) => {
                    const v = Number(e.target.value) || 0
                    set('speed_limit_kbps', v === 0 ? null : v)
                  }}
                  className={inputCls}
                />
                <p className={hintCls}>
                  Caps the app&apos;s total download speed. Changing it takes effect on running downloads
                  too, and the current cap shows in the status bar.
                </p>
              </div>
              <fieldset className="rounded-md border border-border p-3">
                <legend className="px-1 text-sm font-semibold">Download quota</legend>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="checkbox"
                    checked={settings.quota_enabled}
                    onChange={(e) => set('quota_enabled', e.target.checked)}
                    className="h-4 w-4 accent-accent"
                  />
                  Limit downloads to a quota
                </label>
                <div className="mt-3 flex gap-3">
                  <div className="flex-1">
                    <label htmlFor="set-quota-mb" className={labelCls}>
                      Quota <span className="font-normal text-text-muted">(MB, 1–1,000,000)</span>
                    </label>
                    <input
                      id="set-quota-mb"
                      type="number"
                      min={1}
                      max={1000000}
                      disabled={!settings.quota_enabled}
                      value={settings.quota_mb ?? ''}
                      onChange={(e) => {
                        const v = Number(e.target.value)
                        set('quota_mb', e.target.value === '' || Number.isNaN(v) ? null : v)
                      }}
                      className={inputCls}
                    />
                  </div>
                  <div className="flex-1">
                    <label htmlFor="set-quota-hours" className={labelCls}>
                      Every <span className="font-normal text-text-muted">(hours, 1–720)</span>
                    </label>
                    <input
                      id="set-quota-hours"
                      type="number"
                      min={1}
                      max={720}
                      disabled={!settings.quota_enabled}
                      value={settings.quota_hours ?? ''}
                      onChange={(e) => {
                        const v = Number(e.target.value)
                        set('quota_hours', e.target.value === '' || Number.isNaN(v) ? null : v)
                      }}
                      className={inputCls}
                    />
                  </div>
                </div>
                <p className={hintCls}>
                  When the quota is reached the queue pauses and no new download starts. The window
                  starts with the first download after the quota is turned on, and rolls over on its
                  own — downloads it paused then resume.
                </p>
                {quotaStatus?.enabled && quotaStatus.limit_mb != null ? (
                  <div className="mt-2 text-sm">
                    <p>
                      Used {(quotaStatus.used_bytes / 1_048_576).toFixed(2)} of {quotaStatus.limit_mb} MB
                      {quotaStatus.window_end_unix
                        ? ` · window ends ${new Date(quotaStatus.window_end_unix * 1000).toLocaleString()}`
                        : ' · window not started yet'}
                      {quotaStatus.reached ? ' · quota reached — downloads are paused' : ''}
                      {quotaStatus.quota_paused_downloads > 0
                        ? ` · ${quotaStatus.quota_paused_downloads} download${quotaStatus.quota_paused_downloads === 1 ? '' : 's'} paused by the quota`
                        : ''}
                    </p>
                    <Button
                      type="button"
                      disabled={resettingQuota}
                      onClick={() => {
                        setResettingQuota(true)
                        setError(null)
                        resetQuota()
                          .then((q) => setQuotaStatus(q))
                          .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
                          .finally(() => setResettingQuota(false))
                      }}
                    >
                      {resettingQuota ? 'Resetting…' : 'Reset quota & resume now'}
                    </Button>
                    <p className={hintCls}>
                      Reset starts a fresh window at once and resumes the downloads the quota paused.
                    </p>
                  </div>
                ) : null}
              </fieldset>
            </div>
          ) : tab === 'saveto' ? (
            <div className="flex flex-col gap-4">
              <p className="text-sm text-text-muted">
                Where each category&apos;s finished files go. A new download in a category uses that
                folder unless you pick another one when adding it.
              </p>
              {categories.map((c) => (
                <div key={c.id}>
                  <label htmlFor={`set-folder-${c.id}`} className={labelCls}>
                    {c.name}
                  </label>
                  <input
                    id={`set-folder-${c.id}`}
                    type="text"
                    value={catEdits[c.id]?.folder ?? c.default_folder}
                    onChange={(e) =>
                      setCatEdits((prev) => ({
                        ...prev,
                        [c.id]: { folder: e.target.value, extensions: prev[c.id]?.extensions ?? c.file_extensions.join(', ') },
                      }))
                    }
                    placeholder="C:\Users\you\Downloads\Video"
                    className={`${inputCls} font-mono`}
                  />
                </div>
              ))}
            </div>
          ) : tab === 'downloads' ? (
            <div className="flex flex-col gap-4">
              <fieldset>
                <legend className="mb-1 text-sm font-semibold">When a download is added</legend>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="radio"
                    name="default-action"
                    checked={settings.default_action === 'start_now'}
                    onChange={() => set('default_action', 'start_now')}
                    className="h-4 w-4 accent-accent"
                  />
                  Start downloading right away
                </label>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="radio"
                    name="default-action"
                    checked={settings.default_action === 'save_for_later'}
                    onChange={() => set('default_action', 'save_for_later')}
                    className="h-4 w-4 accent-accent"
                  />
                  Save for later (file it in the main queue)
                </label>
              </fieldset>
              <fieldset>
                <legend className="mb-1 text-sm font-semibold">When the file already exists</legend>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="radio"
                    name="duplicate-action"
                    checked={settings.duplicate_action === 'rename'}
                    onChange={() => set('duplicate_action', 'rename')}
                    className="h-4 w-4 accent-accent"
                  />
                  Keep both — save as “name (2)”
                </label>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="radio"
                    name="duplicate-action"
                    checked={settings.duplicate_action === 'overwrite'}
                    onChange={() => set('duplicate_action', 'overwrite')}
                    className="h-4 w-4 accent-accent"
                  />
                  Replace the existing file
                </label>
              </fieldset>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.confirm_delete}
                  onChange={(e) => set('confirm_delete', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Ask before removing a download
              </label>
              <fieldset>
                <legend className="mb-1 text-sm font-semibold">Antivirus scan on completion</legend>
                <label className="flex items-center gap-2 text-base">
                  <input
                    type="checkbox"
                    checked={settings.antivirus_enabled}
                    onChange={(e) => set('antivirus_enabled', e.target.checked)}
                    className="h-4 w-4 accent-accent"
                  />
                  Scan finished downloads with an antivirus scanner
                </label>
                <div className="mt-2">
                  <label htmlFor="set-antivirus" className={labelCls}>
                    Scanner command <span className="font-normal text-text-muted">(optional)</span>
                  </label>
                  <input
                    id="set-antivirus"
                    type="text"
                    value={settings.antivirus_command ?? ''}
                    onChange={(e) => set('antivirus_command', e.target.value.trim() === '' ? null : e.target.value)}
                    placeholder={'MpCmdRun.exe -Scan -ScanType 3 -File "{file}"'}
                    disabled={!settings.antivirus_enabled}
                    className={`${inputCls} font-mono`}
                  />
                  <p className={hintCls}>
                    Runs on a file when its download finishes. <code>{'{file}'}</code> is replaced
                    with the finished file&apos;s path; without it, the path is added at the end.
                    Leave empty to skip scanning.
                  </p>
                </div>
                <div className="mt-2">
                  <label htmlFor="set-antivirus-timeout" className={labelCls}>
                    Scan timeout (seconds)
                  </label>
                  <input
                    id="set-antivirus-timeout"
                    type="number"
                    min={5}
                    max={600}
                    value={settings.antivirus_timeout_secs}
                    onChange={(e) => set('antivirus_timeout_secs', Number(e.target.value) || 0)}
                    disabled={!settings.antivirus_enabled}
                    className={inputCls}
                  />
                  <p className={hintCls}>
                    A scan that runs longer is stopped and reported as timed out — the app never
                    waits on a scanner forever.
                  </p>
                </div>
                <div className="mt-2 flex items-center gap-2">
                  <Button variant="secondary" onClick={() => void runAvTest()} disabled={testingAv || !settings.antivirus_enabled}>
                    Test scanner
                  </Button>
                  {avTestResult ? (
                    <p role="status" className="text-sm text-text-muted">
                      {avTestResult}
                    </p>
                  ) : null}
                </div>
                <p className={hintCls}>
                  Sonic Manager does not include a scanner — use one already installed on your
                  computer, for example Windows Defender. A file the scanner flags is never
                  opened automatically; the completion dialog offers Delete or Quarantine
                  instead.
                </p>
              </fieldset>
            </div>
          ) : tab === 'proxy' ? (
            <div className="flex flex-col gap-4">
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={proxy.enabled}
                  onChange={(e) => setProxy((prev) => ({ ...prev, enabled: e.target.checked }))}
                  className="h-4 w-4 accent-accent"
                />
                Use a proxy for downloads
              </label>
              <div>
                <label htmlFor="set-proxy-type" className={labelCls}>
                  Type
                </label>
                <select
                  id="set-proxy-type"
                  value={proxy.proxy_type}
                  onChange={(e) =>
                    setProxy((prev) => ({
                      ...prev,
                      proxy_type: e.target.value,
                      // Kerberos and NTLM are HTTP-proxy auth only;
                      // SOCKS5 and FTP proxies use the
                      // username/password (Basic).
                      auth: e.target.value === 'http' ? prev.auth : 'basic',
                    }))
                  }
                  className={inputCls}
                  disabled={!proxy.enabled}
                >
                  <option value="http">HTTP</option>
                  <option value="socks">SOCKS5</option>
                  <option value="ftp">FTP</option>
                </select>
                {proxy.proxy_type === 'ftp' ? (
                  <p className={hintCls}>
                    FTP proxies route ftp:// downloads (with the proxy
                    username and password, then user@host for the FTP
                    server). FTP through an HTTP proxy is not supported.
                  </p>
                ) : null}
              </div>
              <div>
                <label htmlFor="set-proxy-auth" className={labelCls}>
                  Authentication
                </label>
                <select
                  id="set-proxy-auth"
                  value={proxy.auth ?? 'negotiate-auto'}
                  onChange={(e) => setProxy((prev) => ({ ...prev, auth: e.target.value }))}
                  className={inputCls}
                  disabled={!proxy.enabled}
                >
                  {proxy.proxy_type === 'http' ? (
                    <>
                      <option value="negotiate-auto">Automatic (Kerberos, then NTLM, then Basic)</option>
                      <option value="kerberos">Kerberos (Negotiate / SPNEGO)</option>
                      <option value="ntlm">NTLM</option>
                      <option value="basic">Basic</option>
                    </>
                  ) : (
                    <option value="basic">Basic</option>
                  )}
                </select>
                {proxy.proxy_type === 'http' ? (
                  <p className={hintCls}>
                    Kerberos uses your Windows sign-in (or your Kerberos
                    ticket on Linux), not the username and password above —
                    those are only for NTLM and Basic. Automatic tries
                    Kerberos first, then NTLM, then Basic, in that order;
                    a proxy that offers only Negotiate never falls back to
                    Basic. Kerberos works for http:// downloads only.
                  </p>
                ) : (
                  <p className={hintCls}>
                    SOCKS5 and FTP proxies authenticate with the username
                    and password above (Basic).
                  </p>
                )}
              </div>
              <div className="flex gap-4">
                <div className="flex-1">
                  <label htmlFor="set-proxy-host" className={labelCls}>
                    Host
                  </label>
                  <input
                    id="set-proxy-host"
                    type="text"
                    value={proxy.host}
                    onChange={(e) => setProxy((prev) => ({ ...prev, host: e.target.value }))}
                    placeholder="proxy.example.com"
                    className={inputCls}
                    disabled={!proxy.enabled}
                  />
                </div>
                <div className="w-28 shrink-0">
                  <label htmlFor="set-proxy-port" className={labelCls}>
                    Port
                  </label>
                  <input
                    id="set-proxy-port"
                    type="number"
                    min={1}
                    max={65535}
                    value={proxy.port}
                    onChange={(e) => setProxy((prev) => ({ ...prev, port: Number(e.target.value) || 0 }))}
                    className={inputCls}
                    disabled={!proxy.enabled}
                  />
                </div>
              </div>
              <div className="flex gap-4">
                <div className="flex-1">
                  <label htmlFor="set-proxy-user" className={labelCls}>
                    Username <span className="font-normal text-text-muted">(optional)</span>
                  </label>
                  <input
                    id="set-proxy-user"
                    type="text"
                    value={proxy.username ?? ''}
                    onChange={(e) =>
                      setProxy((prev) => ({
                        ...prev,
                        username: e.target.value.trim() === '' ? null : e.target.value,
                      }))
                    }
                    className={inputCls}
                    disabled={!proxy.enabled}
                  />
                </div>
                <div className="flex-1">
                  <label htmlFor="set-proxy-pass" className={labelCls}>
                    Password <span className="font-normal text-text-muted">(optional)</span>
                  </label>
                  <input
                    id="set-proxy-pass"
                    type="password"
                    value={password}
                    onChange={(e) => {
                      setPassword(e.target.value)
                      setPasswordTouched(true)
                    }}
                    placeholder={proxyRow?.has_password ? 'Password saved' : ''}
                    className={inputCls}
                    disabled={!proxy.enabled}
                  />
                  {proxyRow?.has_password && !passwordTouched ? (
                    <p className={hintCls}>
                      A password is saved. Leave this empty to keep it; type a new one to change it.
                      To test with the saved password, type it again below.
                    </p>
                  ) : null}
                </div>
              </div>
              <div>
                <label htmlFor="set-proxy-usefor" className={labelCls}>
                  Use the proxy for
                </label>
                <select
                  id="set-proxy-usefor"
                  value={proxy.use_for.length === 0 ? 'all' : proxy.use_for[0]}
                  onChange={(e) =>
                    setProxy((prev) => ({
                      ...prev,
                      use_for: e.target.value === 'all' ? [] : [e.target.value],
                    }))
                  }
                  className={inputCls}
                  disabled={!proxy.enabled}
                >
                  <option value="all">All traffic</option>
                  <option value="http">HTTP only</option>
                  <option value="https">HTTPS only</option>
                </select>
              </div>
              <div>
                <label htmlFor="set-proxy-bypass" className={labelCls}>
                  Bypass the proxy for <span className="font-normal text-text-muted">(comma-separated hosts)</span>
                </label>
                <input
                  id="set-proxy-bypass"
                  type="text"
                  value={proxy.bypass.join(', ')}
                  onChange={(e) =>
                    setProxy((prev) => ({
                      ...prev,
                      bypass: e.target.value
                        .split(',')
                        .map((h) => h.trim().toLowerCase())
                        .filter((h) => h.length > 0),
                    }))
                  }
                  placeholder="localhost, 127.0.0.1"
                  className={`${inputCls} font-mono`}
                  disabled={!proxy.enabled}
                />
              </div>
              <div className="rounded-md border border-border bg-surface-2 p-4">
                <label htmlFor="set-proxy-test-url" className={labelCls}>
                  Test the proxy
                </label>
                <div className="flex gap-2">
                  <input
                    id="set-proxy-test-url"
                    type="text"
                    value={testUrl}
                    onChange={(e) => setTestUrl(e.target.value)}
                    className={`${inputCls} font-mono`}
                    disabled={!proxy.enabled}
                  />
                  <Button variant="secondary" loading={testing} disabled={!proxy.enabled} onClick={() => void runTest()}>
                    Test
                  </Button>
                </div>
                {testResult ? (
                  <p role="status" className="mt-2 text-sm text-text">
                    {testResult}
                  </p>
                ) : null}
              </div>
            </div>
          ) : tab === 'logins' ? (
            <div className="flex flex-col gap-4">
              <p className="text-sm text-text-muted">
                Saved sign-ins for sites that ask for a username and password. When a download
                from one of these sites starts, Sonic Manager signs in with the saved login (HTTP
                Basic sign-in only). Passwords are never shown here — only whether one is saved.
              </p>
              {logins.length === 0 ? (
                <p className="text-sm text-text-muted">No site logins saved yet.</p>
              ) : (
                logins.map((l) => {
                  const draft = loginEdits[l.id] ?? { host: l.host, username: l.username, password: '', passwordTouched: false }
                  const showPassword = showLoginPasswords[l.id] ?? false
                  return (
                    <div key={l.id} className="rounded-md border border-border bg-surface-2 p-4">
                      <div className="flex gap-4">
                        <div className="flex-1">
                          <label htmlFor={`set-login-host-${l.id}`} className={labelCls}>
                            Site
                          </label>
                          <input
                            id={`set-login-host-${l.id}`}
                            type="text"
                            value={draft.host}
                            onChange={(e) =>
                              setLoginEdits((prev) => ({
                                ...prev,
                                [l.id]: { ...draft, host: e.target.value },
                              }))
                            }
                            placeholder="example.com"
                            className={`${inputCls} font-mono`}
                          />
                        </div>
                        <div className="flex-1">
                          <label htmlFor={`set-login-user-${l.id}`} className={labelCls}>
                            Username
                          </label>
                          <input
                            id={`set-login-user-${l.id}`}
                            type="text"
                            value={draft.username}
                            onChange={(e) =>
                              setLoginEdits((prev) => ({
                                ...prev,
                                [l.id]: { ...draft, username: e.target.value },
                              }))
                            }
                            className={inputCls}
                          />
                        </div>
                      </div>
                      <div className="mt-4 flex gap-2">
                        <div className="flex-1">
                          <label htmlFor={`set-login-pass-${l.id}`} className={labelCls}>
                            Password {l.has_password ? <span className="font-normal text-text-muted">(saved — leave empty to keep it)</span> : null}
                          </label>
                          <input
                            id={`set-login-pass-${l.id}`}
                            type={showPassword ? 'text' : 'password'}
                            value={draft.password}
                            onChange={(e) =>
                              setLoginEdits((prev) => ({
                                ...prev,
                                [l.id]: { ...draft, password: e.target.value, passwordTouched: true },
                              }))
                            }
                            className={inputCls}
                          />
                        </div>
                        <div className="flex shrink-0 items-end gap-2">
                          <Button
                            variant="secondary"
                            onClick={() =>
                              setShowLoginPasswords((prev) => ({ ...prev, [l.id]: !showPassword }))
                            }
                          >
                            {showPassword ? 'Hide' : 'Show'}
                          </Button>
                          <Button
                            variant="ghost"
                            onClick={() => {
                              setDeletedLoginIds((prev) => [...prev, l.id])
                              setLogins((prev) => prev.filter((row) => row.id !== l.id))
                            }}
                          >
                            Delete
                          </Button>
                        </div>
                      </div>
                    </div>
                  )
                })
              )}
              <div className="rounded-md border border-border bg-surface-2 p-4">
                <p className="mb-2 text-sm font-semibold">Add a site login</p>
                <div className="flex gap-4">
                  <div className="flex-1">
                    <label htmlFor="set-login-new-host" className={labelCls}>
                      Site
                    </label>
                    <input
                      id="set-login-new-host"
                      type="text"
                      value={newLogin.host}
                      onChange={(e) => setNewLogin((prev) => ({ ...prev, host: e.target.value }))}
                      placeholder="example.com"
                      className={`${inputCls} font-mono`}
                    />
                  </div>
                  <div className="flex-1">
                    <label htmlFor="set-login-new-user" className={labelCls}>
                      Username
                    </label>
                    <input
                      id="set-login-new-user"
                      type="text"
                      value={newLogin.username}
                      onChange={(e) => setNewLogin((prev) => ({ ...prev, username: e.target.value }))}
                      className={inputCls}
                    />
                  </div>
                  <div className="flex-1">
                    <label htmlFor="set-login-new-pass" className={labelCls}>
                      Password
                    </label>
                    <input
                      id="set-login-new-pass"
                      type={showLoginPasswords['new'] ? 'text' : 'password'}
                      value={newLogin.password}
                      onChange={(e) =>
                        setNewLogin((prev) => ({ ...prev, password: e.target.value, passwordTouched: true }))
                      }
                      className={inputCls}
                    />
                  </div>
                </div>
                <p className={hintCls}>
                  The password is protected before it is stored (Windows DPAPI on the installed
                  app; on this Linux build it is only obfuscated, not encrypted — see the build
                  notes). It is never shown in plain text unless you press Show, and never logged.
                </p>
              </div>
            </div>
          ) : tab === 'dialup' ? (
            <div className="flex flex-col gap-4">
              <p className="text-sm text-text-muted">
                Dial-up settings, from when downloads ran over a phone line. Most of this is
                legacy on modern Windows: the connection itself is managed by Windows, and
                Sonic Manager never dials or hangs up on its own here — the one part that still does
                something is hanging up after a queue finishes, which uses the same confirmation
                as the other completion actions.
              </p>
              <div>
                <label htmlFor="set-dialup-name" className={labelCls}>
                  Dial-up connection name
                </label>
                <input
                  id="set-dialup-name"
                  type="text"
                  value={settings.dialup_connection_name ?? ''}
                  onChange={(e) =>
                    set('dialup_connection_name', e.target.value.trim() === '' ? null : e.target.value)
                  }
                  placeholder="My Connection"
                  className={inputCls}
                />
                <p className={hintCls}>
                  The Windows dial-up connection to use. Leave empty if you have none.
                </p>
              </div>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.dialup_auto_dial}
                  onChange={(e) => set('dialup_auto_dial', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Dial automatically when a download starts
              </label>
              <p className={hintCls}>
                Stored only: dialling happens through Windows (RAS) where a dial-up connection
                exists, and that has never been run on this machine — Sonic Manager does not dial here.
              </p>
              <label className="flex items-center gap-2 text-base">
                <input
                  type="checkbox"
                  checked={settings.dialup_hangup_after_completion}
                  onChange={(e) => set('dialup_hangup_after_completion', e.target.checked)}
                  className="h-4 w-4 accent-accent"
                />
                Hang up after a queue finishes
              </label>
              <p className={hintCls}>
                When a queue drains cleanly and has no other completion action, Sonic Manager offers
                to hang up the connection above (on Windows, with the usual confirmation). A queue
                with its own action keeps that action instead, and a queue that downloaded nothing
                hangs up nothing.
              </p>
              <div>
                <label htmlFor="set-dialup-redial" className={labelCls}>
                  Redial attempts <span className="font-normal text-text-muted">(0–10, 0 = do not redial)</span>
                </label>
                <input
                  id="set-dialup-redial"
                  type="number"
                  min={0}
                  max={10}
                  value={settings.dialup_redial_attempts}
                  onChange={(e) => set('dialup_redial_attempts', Number(e.target.value) || 0)}
                  className={inputCls}
                />
                <p className={hintCls}>
                  How many times to redial a dropped dial-up connection. This is not the download
                  retry count — a failed download part still retries as set on the Connection tab.
                </p>
              </div>
            </div>
          ) : (
            <div className="flex flex-col gap-4">
              <p className="text-sm text-text-muted">
                A sound for each event. Pick a WAV file per event and turn it on; the sound plays
                when the event happens. Sounds play on Windows only — on this Linux build the Test
                button checks the file and says so instead of playing anything.
              </p>
              {sounds.map((snd) => {
                const draft = soundEdits[snd.event] ?? { enabled: snd.enabled, file: snd.file ?? '' }
                return (
                  <div key={snd.event} className="rounded-md border border-border bg-surface-2 p-4">
                    <label className="flex items-center gap-2 text-base">
                      <input
                        type="checkbox"
                        checked={draft.enabled}
                        onChange={(e) =>
                          setSoundEdits((prev) => ({
                            ...prev,
                            [snd.event]: { ...draft, enabled: e.target.checked },
                          }))
                        }
                        className="h-4 w-4 accent-accent"
                      />
                      {SOUND_LABELS[snd.event] ?? snd.event}
                    </label>
                    <div className="mt-2 flex gap-2">
                      <input
                        type="text"
                        aria-label={`Sound file for ${SOUND_LABELS[snd.event] ?? snd.event}`}
                        value={draft.file}
                        onChange={(e) =>
                          setSoundEdits((prev) => ({
                            ...prev,
                            [snd.event]: { ...draft, file: e.target.value },
                          }))
                        }
                        placeholder="C:\Sounds\done.wav"
                        className={`${inputCls} font-mono`}
                        disabled={!draft.enabled}
                      />
                      <Button
                        variant="secondary"
                        loading={testingSound === snd.event}
                        disabled={!draft.enabled || !draft.file.trim()}
                        onClick={() => void runSoundTest(snd.event)}
                      >
                        Test
                      </Button>
                    </div>
                  </div>
                )
              })}
              {soundTestResult ? (
                <p role="status" className="text-sm text-text">
                  {soundTestResult}
                </p>
              ) : null}
              <p className={hintCls}>
                There is no file picker here yet — type or paste the WAV file&apos;s path. Only WAV
                files are accepted.
              </p>
            </div>
          )}

          {error ? (
            <p role="alert" className="mt-4 text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        <div className="flex shrink-0 justify-end gap-2 border-t border-border px-6 py-4">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} disabled={loading || !settings} onClick={() => void save()}>
            Save settings
          </Button>
        </div>
      </div>
    </div>
  )
}
