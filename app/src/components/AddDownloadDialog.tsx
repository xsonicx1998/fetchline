import { useEffect, useMemo, useRef, useState } from 'react'
import { Plus, X } from 'lucide-react'
import { open as openFolderPicker } from '@tauri-apps/plugin-dialog'
import {
  addDownload,
  addResolvedVideoDownload,
  confirmPendingLink,
  probeUrl,
  setDownloadDescription,
  setDownloadSpeedLimit,
  setSettingField,
  startDownload,
  updateCategory,
  ytdlpResolve,
} from '../lib/api'
import { categoryForExtension } from '../lib/categories'
import type { CategoryRow, ProbeInfo, QualityRow, YtdlpResolveResult } from '../lib/types'
import { Button } from './Button'

interface AddDownloadDialogProps {
  open: boolean
  categories: CategoryRow[]
  /** S09 Downloads setting: which button is the primary one. */
  defaultAction?: 'start_now' | 'save_for_later'
  /** Round 56 (§7.5): the configured default download folder,
   *  shown as the "Save to folder" placeholder so the field no
   *  longer shows a made-up path. Empty when no folder is
   *  configured — the placeholder then says so, and the engine
   *  falls back to the OS Downloads folder. */
  defaultFolder?: string
  /** Round 87.6 — the last folder the user picked outright (the
   *  "..." picker or a folder typed into Save As), across every
   *  add path. The dialog prefills from it when no category is
   *  picked yet, so a re-opened dialog shows the picked folder
   *  instead of the global default (the 87.5 E5 failure: the
   *  remember lived on the category only, and an empty dialog
   *  has no category). Empty when nothing has been picked yet. */
  lastUsedFolder?: string
  /** Round 87.5 (E4) — the Settings `file_types_auto`
   *  allow-list, for the category auto-pick: the engine's
   *  `category_for_extension_filtered` restricts auto-assign to
   *  these types when the list is non-empty, and the dialog's
   *  pick must agree with it. Empty (the default) means every
   *  type auto-assigns. */
  autoFileTypes?: string[]
  /** S08: prefill the URL field (the video panel passes the picked
   *  video's link). Empty by default — the dialog clears on open. */
  initialUrl?: string
  onClose: () => void
  /** Round 55 (defect 3a): the created download's id, so the
   *  caller can switch to "All downloads" and select the row. */
  onAdded: (id: string) => void
  /** Round 55 (defect 3a/3b): a submit that finishes after the
   *  dialog timed out or closed reports its failure here, so it
   *  lands in the main error banner instead of vanishing. */
  onLateError: (message: string) => void
  /** Round 67: open the "add a category" dialog above this one
   *  (App renders it after this dialog in the DOM, so it paints
   *  on top). */
  onAddCategory?: () => void
  /** Round 67: a browser-extension capture waiting for its
   *  confirm — the submit confirms this row (through the real
   *  add path, with its cookies) instead of adding a fresh
   *  download. */
  pendingLinkId?: string | null
  /** Round 67: the capture's browser cookies (JSON array), so
   *  the size probe works on cookie-gated links. */
  initialCookies?: string | null
  /** Round 78: a resolved video's headers (yt-dlp's
   *  `http_headers` JSON), so the size probe sends the
   *  User-Agent / Referer the stream host expects. */
  initialHeaders?: string | null
  /** Round 84: a resolved video's size in bytes (the
   *  yt-dlp format's size) — the dialog renders it directly
   *  instead of probing the stream URL, which TikTok's CDN
   *  refuses 403. Null when yt-dlp reported no size. */
  initialFileSize?: number | null
  /** Round 74 (12): capture mode — the dialog shows a captured
   *  link (extension or clipboard), not a typed URL: the file
   *  name is the title instead of "Add URL", and the URL field
   *  is locked. */
  captureMode?: boolean
  /** Round 74 (12): the captured file's name when it is already
   *  known (the clipboard offer carries one); the probe fills it
   *  in otherwise. */
  captureTitle?: string | null
  /** Round 75: a clipboard capture of a URL that is already in
   *  the download list — the dialog shows "This link is already
   *  in your list" and lets the user continue or cancel, instead
   *  of the engine swallowing the offer. */
  alreadyInList?: boolean
  /** Round 74 (13): render as the standalone Add URL window's
   *  content (fills its window, no overlay/backdrop) instead of
   *  the in-window overlay. */
  standalone?: boolean
  /** Round 69 (d): a submit that fails (404, server not found,
   *  timeout…) reports here too, so the caller can open the
   *  standalone error dialog with the real error — the inline
   *  pink text alone looked like "Start Download did nothing"
   *  in the Round 68 retest. The filename rides along for the
   *  dialog's title bar. */
  onSubmitError?: (message: string, filename: string) => void
}

/** The Save As field shows the full path (folder + filename).
 *  Split on the last separator so the filename part is the one
 *  the user edits; a bare name (no separator) is a rename — the
 *  folder stays. Exported for the standalone Add URL window
 *  (Round 76 (1)): its title bar shows the URL's last segment. */
export function filenameFromUrl(rawUrl: string): string {
  try {
    const parsed = new URL(rawUrl.trim())
    const segment = parsed.pathname.split('/').filter(Boolean).pop() ?? ''
    return decodeURIComponent(segment).trim()
  } catch {
    return ''
  }
}

function splitSaveAs(value: string): { folder: string; filename: string } {
  const idx = Math.max(value.lastIndexOf('\\'), value.lastIndexOf('/'))
  if (idx < 0) return { folder: '', filename: value }
  return { folder: value.slice(0, idx), filename: value.slice(idx + 1) }
}

function joinSaveAs(folder: string, filename: string): string {
  const f = folder.replace(/[\\/]+$/, '')
  if (!f) return filename
  return filename ? `${f}\\${filename}` : `${f}\\`
}

/** Round 87.6 — apply a Save As edit against the folder the
 *  dialog already holds. Splitting on the last separator is
 *  lossy when the edit deletes exactly that separator:
 *  `C:\...\ZipTest\slow a test.mp4` with the `\` after ZipTest
 *  deleted reads as folder `C:\...\Downloads` + filename
 *  `ZipTestslow a test.mp4` — the 87.5 retest's malformed row
 *  (and the Open With failure on the nonexistent path). When the
 *  new value still starts with the known folder and the
 *  remainder has no separator at all, the user edited the
 *  filename tail, not the folder: keep the folder and take the
 *  remainder as the name. Any other edit (a folder rename, a
 *  pasted full path, a value that no longer starts with the
 *  folder) splits on the last separator as before. Exported so
 *  the rule is locked in by tests. */
export function applySaveAsEdit(
  prevFolder: string,
  value: string,
): { folder: string; filename: string } {
  const prefix = prevFolder.replace(/[\\/]+$/, '')
  if (prefix && value.startsWith(prefix)) {
    const rest = value.slice(prefix.length)
    if (rest && !rest.includes('\\') && !rest.includes('/')) {
      return { folder: prevFolder, filename: rest }
    }
  }
  return splitSaveAs(value)
}

/** Round 79 — a pasted link that is a platform video page
 *  (YouTube / TikTok / Instagram / Facebook): the dialog
 *  resolves it with yt-dlp instead of probing the page itself
 *  (a watch-page URL probes size 0 with no ranges, and the
 *  Round 78 retest showed the manual dialog creating exactly
 *  that broken download). Mirrors the extension's
 *  `isSupportedPlatform`, with a path hint on YouTube so a
 *  channel page is not resolved. */
export function isVideoPageUrl(rawUrl: string): boolean {
  let parsed: URL
  try {
    parsed = new URL(rawUrl.trim())
  } catch {
    return false
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false
  const host = parsed.hostname.toLowerCase().replace(/^www\./, '')
  if (host === 'youtu.be') return true
  if (host === 'youtube.com' || host.endsWith('.youtube.com')) {
    const path = parsed.pathname
    return (
      path === '/watch' ||
      path.startsWith('/watch/') ||
      path.startsWith('/shorts/') ||
      path.startsWith('/live/') ||
      path.startsWith('/embed/') ||
      path.startsWith('/v/')
    )
  }
  if (host === 'tiktok.com' || host.endsWith('.tiktok.com')) return true
  if (host === 'instagram.com' || host.endsWith('.instagram.com')) return true
  if (
    host === 'facebook.com' ||
    host.endsWith('.facebook.com') ||
    host === 'fb.watch' ||
    host.endsWith('.fb.watch')
  )
    return true
  return false
}

/** Round 79 — the best quality row of a resolve: rows are one
 *  per height, sorted ascending, so the last row is the highest
 *  quality the site offers. */
export function bestQualityRow(result: YtdlpResolveResult | null): QualityRow | null {
  if (!result || result.formats.length === 0) return null
  return result.formats[result.formats.length - 1]
}

/** Round 80 — the Add URL field accepts an http(s) link. The
 *  rule itself was already correct — it accepts the Round 79
 *  retest's Instagram reel URL with and without a trailing slash
 *  (verified by direct evaluation); the retest rejection came
 *  from the typed text, not the rule (see `normalizeUrlScheme`).
 *  Exported so the rule is locked in by tests. */
export function isHttpUrl(rawUrl: string): boolean {
  return /^https?:\/\/.+/i.test(rawUrl.trim())
}

/** Round 80 — a `https;//…` scheme (a semicolon for the colon)
 *  is what an unreliable Shift key produces when a URL is typed
 *  through RDP (the Round 79 retest's Instagram rejection: the
 *  validation rule accepts the real `https://…` URL, but the
 *  typed text had `;` for `:`). Repair just that typo at input
 *  time so the field self-corrects instead of rejecting a link
 *  the user meant. Anything else passes through untouched. */
export function normalizeUrlScheme(raw: string): string {
  return raw.replace(/^(\s*https?);\/\//i, '$1://')
}

/** S02 — add a new download (components.md: Dialog, TextInput, Dropdown).
 *  URL, file name, save folder and category. "Start now" begins the
 *  download at once; "Save for later" only queues it.
 *
 *  Round 67 — redesigned to the approved mockup (IDM's Download
 *  File Info in Sonic identity): dark title bar with the real
 *  logo + "Add URL" + X, a purple→pink gradient divider, fields
 *  in IDM order (URL, Category + "+", Save As as one editable
 *  full path, "Remember this path" with the file size in cyan on
 *  the same row, a read-only folder-path field under it,
 *  Description), and Download Later / Start Download / Cancel
 *  with the default-action button as a purple→pink gradient.
 *  Tightened vertical spacing so the dialog is shorter/squarer
 *  than the old one. The per-download speed cap stays (compact
 *  row at the bottom). */
export function AddDownloadDialog({
  open,
  categories,
  defaultAction = 'start_now',
  defaultFolder = '',
  lastUsedFolder = '',
  autoFileTypes = [],
  initialUrl = '',
  onClose,
  onAdded,
  onLateError,
  onAddCategory,
  onSubmitError,
  pendingLinkId = null,
  initialCookies = null,
  initialHeaders = null,
  initialFileSize = null,
  captureMode = false,
  captureTitle = null,
  alreadyInList = false,
  standalone = false,
}: AddDownloadDialogProps) {
  const [url, setUrl] = useState('')
  const [filename, setFilename] = useState('')
  const [folder, setFolder] = useState('')
  const [category, setCategory] = useState('')
  /** Round 87.5 (E4) — the user picked a category by hand (the
   *  select, including "Pick automatically"): the auto-pick
   *  below must never override that choice. Reset on open. */
  const userPickedCategory = useRef(false)
  /** Per-download speed cap in KB/s (S12); 0 = no cap, the global cap still applies. */
  const [cap, setCap] = useState(0)
  const [description, setDescription] = useState('')
  /** Round 87.5 (E5/R77c) — the "Remember this path" checkbox
   *  is gone: it only worked with a category picked by hand,
   *  and categories never auto-picked, so in practice the path
   *  was never remembered at all. A folder the user picks (the
   *  "..." picker or a folder typed into Save As) is now
   *  remembered on submit — the category's Save To, or the
   *  global default folder when no category is set (see
   *  `submit`). */
  /** File size from the URL probe, for the cyan size on the
   *  remember-path row; null while unknown / unprobed. */
  const [fileSize, setFileSize] = useState<number | null>(null)
  /** Round 70 — the debounced probe's failure, shown muted next
   *  to the size: the probe used to fail silently (size stayed
   *  blank, no signal at all), so a dead link looked like
   *  "Start Download did nothing" in the Round 69 retest. This
   *  is advisory only — the submit re-probes and reports its own
   *  error — so it is muted text, not the pink submit error. */
  const [probeError, setProbeError] = useState<string | null>(null)
  /** Round 79 — the yt-dlp resolve for a pasted platform
   *  video page (title + quality rows), and its failure or
   *  in-flight state. `resolvedForUrl` is the URL the cached
   *  result belongs to — a resolve for one video must never
   *  be submitted for another. */
  const [resolved, setResolved] = useState<YtdlpResolveResult | null>(null)
  const [resolveError, setResolveError] = useState<string | null>(null)
  const [resolving, setResolving] = useState(false)
  const resolvedForUrl = useRef('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)
  /** Round 55: a submit that already succeeded (any attempt) —
   *  a stale attempt's later failure is noise then, not an error
   *  to report. And the sequence of submits, so a retry supersedes
   *  the attempt before it. */
  const anySucceeded = useRef(false)
  const submitSeq = useRef(0)
  // Round 67: the user typed a folder of their own (Save As edit
  // or the "..." picker) — the category autofill below must not
  // overwrite it.
  const folderEdited = useRef(false)
  // Round 87.7 — the Save As field edits a local draft string:
  // the 87.6 per-keystroke parse + rejoin mangled a select-all +
  // type (each keystroke re-prepended the folder) and a
  // backslash deletion never reached the guard with the right
  // folder, because the input was controlled by the parsed
  // state. The draft is parsed with `applySaveAsEdit` only on
  // blur / submit — see `commitSaveAsDraft`.
  const [saveAsDraft, setSaveAsDraft] = useState<string | null>(null)
  // Round 87.7 — the folder the user picked OUTRIGHT this
  // session (the "..." picker, or a Save As commit whose folder
  // differs from the one shown before). Only this is remembered
  // on the category / passed as `explicitFolder`: a folder the
  // dialog auto-filled (category default, last-used) is not a
  // pick, and the 87.6 retest proved writing it back drifted
  // the seeded folders (Video became ZipTest, Compressed and
  // Documents became the Downloads root).
  const folderExplicitRef = useRef<string | null>(null)
  // Round 67: the user typed a filename of their own — the size
  // probe must not overwrite it with the server's name.
  const userEditedFilename = useRef(false)
  // Round 67: the URL the probe last answered for — a slow probe
  // must not clobber the size of a URL the user has moved on from.
  const probedUrl = useRef('')

  useEffect(() => {
    if (open) {
      setUrl(initialUrl)
      // Round 78 — a filename from the pending row (a resolved
      // video's sanitized title) is already the right name, so
      // prefill it at once instead of waiting for the probe.
      setFilename(captureTitle ?? '')
      // Round 87.6 — prefill the last folder the user picked
      // (not blank): a re-opened dialog shows the picked folder
      // at once, instead of the global default while no category
      // is picked yet. The category autofill below still wins
      // once a category is known. In the standalone window the
      // settings can arrive after mount, so the autofill effect
      // re-applies this when `lastUsedFolder` lands.
      // `lastUsedFolder` is deliberately NOT in this effect's
      // deps: a late arrival must not re-run the whole open
      // reset (it would wipe a URL the user already typed).
      setFolder(lastUsedFolder?.trim() ?? '')
      setCategory('')
      userPickedCategory.current = false
      setCap(0)
      setDescription('')
      setFileSize(null)
      setProbeError(null)
      setResolved(null)
      setResolveError(null)
      setResolving(false)
      resolvedForUrl.current = ''
      setError(null)
      setBusy(false)
      folderEdited.current = false
      setSaveAsDraft(null)
      folderExplicitRef.current = null
      // Round 78 — a row filename must not be overwritten by
      // the probe: the probe derives its name from the URL, and
      // for a googlevideo link that is "videoplayback".
      userEditedFilename.current = captureTitle != null && captureTitle.trim() !== ''
      probedUrl.current = ''
      urlRef.current?.focus()
    }
  }, [open, initialUrl, pendingLinkId, captureTitle])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      // Round 55 (defect 3a): Escape must not close the dialog
      // while a submit is in flight — the probe error would land
      // on an unmounted component and vanish.
      if (e.key === 'Escape' && !busy) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose, busy])

  // Round 67 — probe the URL (debounced) for the file size
  // shown in cyan on the remember-path row, and prefill the
  // filename from the server when the user has not typed one.
  // A capture's cookies ride the probe, so a cookie-gated link
  // shows its size instead of failing with 401.
  // Round 71 (D5) — probe ONLY real URLs: the input must start
  // with http://, https:// or ftp:// AND pass `new URL()`
  // validation, so a bare word ("X"), a partial scheme, or two
  // URLs pasted together never reach the engine (startup.log
  // showed all three being probed). A monotonic request counter
  // discards stale responses from earlier keystrokes — the old
  // `probedUrl` string guard stays as a second check.
  const probeReqId = useRef(0)
  useEffect(() => {
    if (!open) return
    const target = url.trim()
    if (!target) {
      // Invalidate any in-flight resolve/probe for the URL that
      // was just cleared — its answer must not land on an empty
      // field (or a different URL typed next).
      probeReqId.current += 1
      setFileSize(null)
      setProbeError(null)
      setResolved(null)
      setResolveError(null)
      setResolving(false)
      return
    }
    const scheme = target.startsWith('http://')
      ? 'http://'
      : target.startsWith('https://')
        ? 'https://'
        : target.startsWith('ftp://')
          ? 'ftp://'
          : null
    if (scheme === null) return
    // Two URLs pasted together ("https://a…https://b…").
    if (target.slice(scheme.length).includes('://')) return
    try {
      new URL(target)
    } catch {
      return
    }
    // Round 84 — a yt-dlp-resolved capture (a pending row with
    // the resolver's headers and/or size) is NEVER probed:
    // yt-dlp already validated the stream, and the R83 retest
    // proved the probe itself is refused 403 by TikTok's CDN
    // on the signed URL — the probe error then blocked Start
    // Download and locked the row out. Render the pre-resolved
    // size (or nothing when yt-dlp reported none — "size
    // unknown" is tolerated, not an error) and return: no
    // probe, no error. A capture without either signal is an
    // old plain capture and still probes as before.
    const resolvedCapture =
      pendingLinkId != null && (initialHeaders != null || initialFileSize != null)
    if (resolvedCapture) {
      setFileSize(initialFileSize ?? null)
      setProbeError(null)
      return
    }
    const reqId = ++probeReqId.current
    const timer = window.setTimeout(async () => {
      // Round 79 — a platform video page resolves with yt-dlp
      // instead of probing the page itself (a watch-page URL
      // probes size 0 with no ranges, and the Round 78 retest
      // showed the manual dialog creating exactly that broken
      // download). Captures never resolve here — their row is
      // already resolved by the extension.
      if (isVideoPageUrl(target) && !pendingLinkId) {
        setResolving(true)
        try {
          const result = await ytdlpResolve(target)
          if (probeReqId.current !== reqId || probedUrl.current !== target) return
          if (result.is_live) {
            resolvedForUrl.current = ''
            setResolved(null)
            setFileSize(null)
            setResolveError("Live streams can't be downloaded")
            return
          }
          if (result.formats.length === 0) {
            // Round 80 — a resolve that succeeds with no formats
            // (a feed page, not a video — the Round 79 retest's
            // Facebook watch/ URL) used to show nothing at all:
            // no quality line, no error, so the dialog looked
            // hung. Say so plainly instead.
            // Round 87 — two different zeroes: when the resolver
            // DID find something but could use none of it, it
            // says so in `warning` (the zero-format diagnosis
            // from the engine) — that is a video the app cannot
            // download, not a feed. Only a warning-less zero is
            // "no video here".
            resolvedForUrl.current = ''
            setResolved(null)
            setFileSize(null)
            setResolveError(
              result.warning
                ? 'This video was found, but none of its formats could be downloaded.'
                : 'No downloadable video was found on this page — it may be a feed or a page, not a video.',
            )
            return
          }
          resolvedForUrl.current = target
          setResolved(result)
          setResolveError(null)
          setProbeError(null)
          const best = bestQualityRow(result)
          setFileSize(best?.filesize ?? null)
          if (!userEditedFilename.current && result.title) {
            setFilename(`${result.title}.${best?.ext ?? 'mp4'}`)
          }
        } catch (e) {
          if (probeReqId.current !== reqId || probedUrl.current !== target) return
          resolvedForUrl.current = ''
          setResolved(null)
          setFileSize(null)
          setProbeError(null)
          setResolveError(e instanceof Error ? e.message : String(e))
        } finally {
          if (probeReqId.current === reqId && probedUrl.current === target) {
            setResolving(false)
          }
        }
        return
      }
      // Not a video page: drop any stale resolve from a
      // previous URL (the quality line must not show for a
      // plain file link).
      resolvedForUrl.current = ''
      setResolved(null)
      setResolveError(null)
      try {
        // Round 78 — a resolved video's headers ride the probe,
        // so a stream host that expects its own User-Agent /
        // Referer answers instead of 403.
        const info: ProbeInfo = await probeUrl(target, initialCookies, initialHeaders)
        if (probeReqId.current !== reqId || probedUrl.current !== target) return
        setFileSize(info.size ?? null)
        setProbeError(null)
        if (!userEditedFilename.current && info.filename) {
          setFilename(info.filename)
        }
      } catch (e) {
        if (probeReqId.current !== reqId || probedUrl.current !== target) return
        setFileSize(null)
        setProbeError(e instanceof Error ? e.message : String(e))
      }
    }, 500)
    probedUrl.current = target
    return () => window.clearTimeout(timer)
  }, [open, url, initialCookies, initialHeaders, initialFileSize, pendingLinkId])

  // Round 67 — IDM behaviour: picking a category fills the
  // folder with that category's default folder (the engine
  // resolves the same way when no folder is given), unless the
  // user already typed a folder of their own.
  // Round 87.6 — with no category yet (an empty dialog, or a
  // URL with no recognizable extension), fill the folder with
  // the last folder the user picked instead of leaving it blank
  // (blank showed the global default — the 87.5 E5 failure).
  // The effect also re-runs when `lastUsedFolder` lands (the
  // standalone window's settings can arrive after mount).
  useEffect(() => {
    if (folderEdited.current) return
    const cat = categories.find((c) => c.id === category)
    if (cat && cat.default_folder.trim()) {
      setFolder(cat.default_folder.trim())
    } else if (!cat) {
      setFolder(lastUsedFolder?.trim() ?? '')
    }
  }, [category, categories, lastUsedFolder])

  // Round 87.5 (E4) — auto-pick the category from the file
  // type: when the filename (or the URL's last segment) is
  // known, pre-select the category matching its extension —
  // the same rule the engine applies on submit
  // (`category_for_extension_filtered`), so what the dialog
  // shows is what the row lands in. The user picking a
  // category by hand wins: the pick only runs until then, and
  // a file with no extension leaves the select alone instead
  // of jumping to Documents while the name is still being
  // typed. The pick runs on the URL at once (before the probe
  // answers) and again when the probe / resolve fills in the
  // real filename.
  useEffect(() => {
    if (!open || userPickedCategory.current) return
    const name = filename.trim() || filenameFromUrl(url)
    const picked = categoryForExtension(categories, name, autoFileTypes)
    if (picked && picked !== category) setCategory(picked)
  }, [open, filename, url, categories, autoFileTypes, category])

  const categoryName = useMemo(
    () => categories.find((c) => c.id === category)?.name ?? '',
    [categories, category],
  )

  /** Round 79 — the best quality of the cached resolve (the
   *  highest row), shown next to the size. */
  const bestRow = bestQualityRow(resolved)

  // Round 67 — the folder the file will land in: the typed one,
  // else the last folder the user picked, else the global
  // default, else the OS Downloads folder (what the engine falls
  // back to). Shown read-only under the remember-path row, with
  // a trailing separator.
  const effectiveFolder =
    folder.trim() || lastUsedFolder?.trim() || defaultFolder?.trim() || 'Downloads'
  const folderDisplay = `${effectiveFolder.replace(/[\\/]+$/, '')}\\`

  // Round 67 — the Save As field shows the full path; editing it
  // splits on the last separator. A bare name is a rename — the
  // folder stays (the folder field below still shows it).
  // Round 87.7 — the field edits a DRAFT string now, parsed
  // with `applySaveAsEdit` only on blur / submit. The 87.6
  // per-keystroke parse + rejoin did NOT take effect live: a
  // select-all + type re-prepended the folder on every
  // keystroke (the field showed
  // `tor\Downloads\ZipC:\Users\...\ZipTest\slow a test.mp4`),
  // and a backslash deletion read against a stale folder state
  // merged it (`...\ZipTestslow-a.mp4`). Typing now touches no
  // state at all until the commit, so the text the user typed
  // is exactly the text that gets parsed.
  const saveAsValue = saveAsDraft ?? joinSaveAs(folder, filename)
  const onSaveAsChange = (value: string) => {
    // The first keystroke locks the folder against the category
    // autofill (the old `folderEdited` guard) and locks the
    // filename against the probe prefill — the folder state
    // only moves on commit, but nothing must overwrite it
    // under the user's draft.
    folderEdited.current = true
    userEditedFilename.current = true
    setSaveAsDraft(value)
  }
  /** Parse the open draft (if any) into folder + filename with
   *  `applySaveAsEdit` and commit it to state. Returns the
   *  committed values, so submit can use them at once instead
   *  of waiting for the state update. A commit whose folder
   *  differs from the one shown before IS an outright pick and
   *  is recorded in `folderExplicitRef`; a rename, a backslash
   *  deletion, or retyping the same folder leaves the ref
   *  alone. */
  const commitSaveAsDraft = (): { folder: string; filename: string } => {
    if (saveAsDraft === null) return { folder, filename }
    const draft = saveAsDraft
    setSaveAsDraft(null)
    if (draft === '') {
      setFolder('')
      setFilename('')
      folderExplicitRef.current = null
      return { folder: '', filename: '' }
    }
    const parsed = applySaveAsEdit(folder, draft)
    const nextFolder = parsed.folder || folder
    const nextFilename = parsed.filename
    if (nextFolder !== folder) {
      folderExplicitRef.current = nextFolder
    }
    if (nextFilename !== filename) userEditedFilename.current = true
    setFolder(nextFolder)
    setFilename(nextFilename)
    return { folder: nextFolder, filename: nextFilename }
  }

  const onBrowseFolder = async () => {
    try {
      const picked = await openFolderPicker({
        directory: true,
        multiple: false,
        defaultPath: effectiveFolder || undefined,
      })
      if (typeof picked === 'string' && picked) {
        setFolder(picked)
        folderEdited.current = true
        // Round 87.7 — a "..." pick IS an outright pick.
        folderExplicitRef.current = picked
      }
    } catch {
      // The picker was cancelled or is unavailable — the user
      // can still type the folder in the Save As field.
    }
  }

  const onCategoryChange = (value: string) => {
    setCategory(value)
    // A hand pick — including back to "Pick automatically" —
    // wins over the auto-pick from now on (see above).
    userPickedCategory.current = true
  }

  /** Round 74 (12): in capture mode the file name is the title
   *  instead of "Add URL" — the probed name first, then the
   *  known capture name, then the URL's last segment. */
  const displayTitle = captureMode
    ? filename.trim() || captureTitle?.trim() || filenameFromUrl(url) || 'Add URL'
    : 'Add URL'

  if (!open) return null

  const urlValid = url.trim() === '' || isHttpUrl(url)

  const formatBytes = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
  }

  async function submit(startNow: boolean) {
    if (!url.trim() || !urlValid) {
      setError('Enter a link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    const runId = ++submitSeq.current
    const work: Promise<string> = (async () => {
      // Round 87.7 — commit any open Save As draft FIRST, so a
      // submit straight after typing uses exactly what the user
      // typed (no blur happened yet), and the committed values
      // — not the pre-edit state — go to the engine.
      const committed = commitSaveAsDraft()
      const submitFolder = committed.folder
      const submitFilename = committed.filename
      const folderValue = submitFolder.trim() || null
      const categoryValue = category || null
      // Round 87.7 — only an OUTRIGHT pick is remembered (see
      // `folderExplicitRef`): the old `folderEdited` guard fired
      // on any Save As keystroke, so a plain rename wrote the
      // auto-filled folder back onto the category — the second
      // half of the 87.6 drift, beside the engine's
      // remember-on-every-add. A failure here must not block the
      // download itself.
      const explicitFolder = folderExplicitRef.current
      if (explicitFolder) {
        try {
          if (categoryValue) {
            await updateCategory({ id: categoryValue, folder: explicitFolder })
          } else {
            await setSettingField('default_download_folder', explicitFolder)
          }
        } catch (e) {
          // Round 76 (2) — a failed remember-path save used to
          // vanish silently (`catch {}`), so a user who picked
          // a folder never knew it was not stored. Surface it
          // (main-window banner / standalone-window alert);
          // the download still starts.
          onLateError(
            `Could not remember this folder: ${
              e instanceof Error ? e.message : String(e)
            } — the download still starts.`,
          )
        }
      }
      if (pendingLinkId) {
        // Round 67 — a browser capture: confirm it through the
        // real add path with its cookies. The engine applies
        // the speed cap before the start and starts it when
        // asked. Round 84 — a failed add leaves the row
        // pending (retryable) instead of marking it rejected,
        // so the dialog shows the error and stays open for the
        // user to fix it and retry.
        const [id] = await confirmPendingLink({
          id: pendingLinkId,
          filename: submitFilename.trim() || null,
          folder: folderValue,
          explicitFolder,
          category: categoryValue,
          queue: startNow ? null : 'queue-main',
          speedLimitKbps: cap > 0 ? cap : null,
          startNow,
        })
        if (description.trim()) {
          try {
            await setDownloadDescription(id, description.trim())
          } catch {
            // Non-fatal — the download is already added.
          }
        }
        return id
      }
      if (!pendingLinkId && isVideoPageUrl(url.trim())) {
        // Round 79 — a platform video page: resolve first and
        // create the download from the RESOLVED stream URLs,
        // never from the page URL (a watch-page URL probes size
        // 0 with no ranges). The debounced probe already
        // resolved it; submit re-resolves when the cached
        // result is stale or missing (e.g. a retry after a
        // failure). A resolve failure throws — the error is
        // shown and NO download is created.
        let result = resolvedForUrl.current === url.trim() ? resolved : null
        if (!result) {
          setResolving(true)
          try {
            result = await ytdlpResolve(url.trim())
          } finally {
            setResolving(false)
          }
          resolvedForUrl.current = url.trim()
          setResolved(result)
          setResolveError(null)
        }
        if (result.is_live) {
          throw new Error("Live streams can't be downloaded")
        }
        const row = bestQualityRow(result)
        if (!row) {
          // Round 87 — same distinction as the probe above: a
          // warning means the resolver found formats it could
          // not use; without one, nothing downloadable was
          // found at all.
          throw new Error(
            result.warning
              ? 'This video was found, but none of its formats could be downloaded.'
              : 'No downloadable qualities were found for this video.',
          )
        }
        const videoFilename = submitFilename.trim() || `${result.title}.${row.ext}`
        const [id] = await addResolvedVideoDownload({
          pageUrl: url.trim(),
          row,
          filename: videoFilename || null,
          folder: folderValue,
          explicitFolder,
          category: categoryValue,
          queue: startNow ? null : 'queue-main',
          speedLimitKbps: cap > 0 ? cap : null,
          startNow,
        })
        if (description.trim()) {
          try {
            await setDownloadDescription(id, description.trim())
          } catch {
            // Non-fatal — the download is already added.
          }
        }
        return id
      }
      const [id] = await addDownload({
        url: url.trim(),
        filename: submitFilename.trim() || null,
        folder: folderValue,
        explicitFolder,
        category: categoryValue,
        // "Save for later" files the download in the main queue, so
        // the scheduler (or Start Now) can pick it up later.
        queue: startNow ? null : 'queue-main',
      })
      // Round 67 — the Description note, stored right after the
      // add. Non-fatal: the download is already added.
      if (description.trim()) {
        try {
          await setDownloadDescription(id, description.trim())
        } catch {
          // Keep going — the download still starts.
        }
      }
      // Per-download cap (S12): stored on the row before the
      // download starts, so the cap applies from the first byte.
      if (cap > 0) {
        await setDownloadSpeedLimit(id, cap)
      }
      if (startNow) {
        await startDownload(id)
      }
      return id
    })()
    // Round 55 (defect 3a/3b): the submit promise outlives the
    // dialog — a late outcome (after a timeout, or after the dialog
    // was closed) still reaches the main window: a late success adds
    // the row, a late failure shows in the main error banner instead
    // of dying on an unmounted component. `consumed` marks an
    // outcome this submit already handled itself, so it is never
    // reported twice; a failure after some submit already succeeded
    // is not reported — the download exists, the stale attempt is
    // noise.
    let consumed = false
    void work.then(
      (id) => {
        if (!consumed) onAdded(id)
      },
      (e: unknown) => {
        if (!consumed && !anySucceeded.current) {
          onLateError(e instanceof Error ? e.message : String(e))
        }
      },
    )
    // Round 55 (defect 3b): the submit must not hang silently —
    // the probe used to stall for minutes with no error. 60 s is
    // deliberately longer than the engine's 30 s total timeout on
    // the probe client (CLIENT_TOTAL_TIMEOUT in
    // app/engine/src/proxy.rs), so the probe + store + start have
    // time to settle before the dialog reports a timeout.
    // (Round 58: the old comment claimed 60 s "matches" the
    // engine timeout — it never did; the engine bound is 30 s.)
    // On timeout the dialog stays open with the error and the user
    // can retry or close; the first attempt keeps running and its
    // late outcome is reported by the handler above.
    let timedOut = false
    const timeout = new Promise<never>((_, reject) => {
      window.setTimeout(() => {
        timedOut = true
        reject(
          new Error(
            'The server took too long to respond (over 60 seconds) — check the link and try again',
          ),
        )
      }, 60_000)
    })
    try {
      const id = await Promise.race([work, timeout])
      // Superseded by a retry: leave the outcome unclaimed, so the
      // detached handler above still reports this attempt's late
      // success (its row exists and should be shown).
      if (submitSeq.current !== runId) return
      consumed = true
      anySucceeded.current = true
      setBusy(false)
      onAdded(id)
      onClose()
    } catch (e) {
      // A work failure (not a timeout) is already claimed by the
      // detached handler above — mark it consumed so it is not
      // reported twice. A timeout leaves it unclaimed: the attempt
      // is still running and its late outcome will be reported.
      if (!timedOut) consumed = true
      if (submitSeq.current !== runId) return
      setBusy(false)
      const msg = e instanceof Error ? e.message : String(e)
      setError(msg)
      // Round 69 (d) — open the standalone error dialog with the
      // real error, like IDM's popup. The inline pink text alone
      // is what made the 404 look like "Start Download did
      // nothing" in the Round 68 retest; the dialog makes the
      // failure unmistakable (and plays the error sound).
      onSubmitError?.(msg, filename.trim() || url.trim())
    }
  }

  const primaryIsLater = defaultAction === 'save_for_later'
  const primaryStyle = {
    background: 'linear-gradient(90deg, #8B20FF, #F02DCD)',
    boxShadow: '0 4px 16px rgba(240,45,205,0.40)',
  } as const

  {/* Round 74 (7): a backdrop click never closes this dialog —
      Cancel / X / Escape only (the old Round 55 handler that
      closed on backdrop click is gone). */}
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby={standalone ? undefined : 'add-download-title'}
      className={
        standalone
          ? 'flex h-full flex-col'
          : 'fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4'
      }
    >
      <div
        className={
          standalone
            ? 'flex min-h-0 w-full flex-1 flex-col overflow-hidden bg-bg text-text'
            : 'flex max-h-full w-full max-w-[620px] flex-col overflow-hidden rounded-xl border border-[#8B20FF]/45 bg-bg text-text shadow-[0_0_0_1px_rgba(0,200,255,0.12),0_24px_70px_rgba(0,0,0,0.65),0_0_44px_rgba(139,32,255,0.22)]'
        }
      >
        {/* Round 81 (double title, ex-Round 77 item a) — in the
            standalone window the file name is ALREADY the
            window's own TitleBar (StandaloneAddUrlWindow), so
            this dialog's inner header (logo + file name + X)
            showed it a second time, stacked. Hide the whole
            inner header here — title, close and divider — and
            keep it for the in-window dialog only, where it is
            the only title. */}
        {standalone ? null : (
          <>
            {/* Title bar — the real logo, "Add URL", X. */}
            <div className="flex items-center gap-2.5 px-3.5 py-3">
              <img
                src="/icon.png"
                alt="Sonic Manager logo"
                className="h-7 w-7 rounded-full shadow-[0_0_12px_rgba(139,32,255,0.6)]"
              />
              <h2 id="add-download-title" className="flex-1 truncate text-[15px] font-semibold" title={displayTitle}>
                {displayTitle}
              </h2>
              <button
                type="button"
                onClick={onClose}
                disabled={busy}
                aria-label="Close"
                className="rounded-md p-1 text-text-muted transition-colors hover:bg-[#F02DCD]/20 hover:text-text"
              >
                <X className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
            <div className="h-[2px] bg-gradient-to-r from-accent to-[#F02DCD]" />
          </>
        )}

        {/* Body — IDM field order, tightened spacing. In the
            standalone window it scrolls inside the fixed-size
            window instead of overflowing it. Round 85 — the
            in-window dialog scrolls too: it used to have no
            max-height and no scroll, so on a short (RDP) main
            window the fields ran past the viewport and Start
            Download sat below the fold with no way to reach it
            except Tab. The body is now min-h-0 + flex-1 +
            overflow-y-auto in BOTH variants, inside a box capped
            at the viewport (standalone: the window's h-full
            column; in-window: max-h-full of the padded overlay),
            and the footer below is shrink-0, so Start Download
            is always visible. No fields were removed and no
            control was shrunk — the dialog is the same height on
            a normal screen; only short viewports scroll. */}
        <div
          className={
            standalone
              ? 'flex min-h-0 flex-1 flex-col gap-[11px] overflow-y-auto px-[22px] pb-1 pt-4'
              : 'flex min-h-0 flex-1 flex-col gap-[11px] overflow-y-auto px-[22px] pb-1 pt-4'
          }
          // Round 87.8 — commit the Save As draft when the pointer
          // goes down anywhere in the fields but the Save As input
          // itself. The 87.7 blur-only commit failed live: a click
          // through the RDP canvas can fail to move DOM focus, so
          // the blur never fired and the merged draft stayed on
          // screen after the backslash-deletion rename. A mousedown
          // IS delivered even when focus is not, so this is the
          // focus-independent commit path. Clicking inside the Save
          // As field itself must not commit — the user is still
          // editing the draft there. The commit is idempotent (a
          // null draft is a no-op; committing the same draft twice
          // re-parses to the same values), so a blur firing for
          // the same move afterwards is harmless.
          onMouseDownCapture={(e) => {
            const target = e.target as HTMLElement | null
            if (target?.closest?.('#add-save-as')) return
            commitSaveAsDraft()
          }}
        >
          <div>
            <label htmlFor="add-url" className="mb-1 block text-xs font-semibold text-text-muted">
              URL
            </label>
            {captureMode && alreadyInList ? (
              <p className="mb-1 text-xs font-semibold text-warning">
                This link is already in your list — you can download it again or cancel.
              </p>
            ) : null}
            <input
              ref={urlRef}
              id="add-url"
              type="url"
              value={url}
              onChange={(e) => setUrl(normalizeUrlScheme(e.target.value))}
              placeholder="https://example.com/file.zip"
              disabled={captureMode}
              title={
                captureMode
                  ? 'Captured link — the address cannot be changed here'
                  : undefined
              }
              aria-invalid={!urlValid || undefined}
              aria-describedby={error ? 'add-error' : undefined}
              className="h-9 w-full rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          <div>
            <label
              htmlFor="add-category"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Category
            </label>
            <div className="flex gap-2">
              <select
                id="add-category"
                value={category}
                onChange={(e) => onCategoryChange(e.target.value)}
                className="h-9 min-w-0 grow rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
              >
                <option value="">Pick automatically from the file type</option>
                {categories.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
              <button
                type="button"
                onClick={() => onAddCategory?.()}
                disabled={busy}
                title="Add category"
                aria-label="Add category"
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-surface-2 text-text transition-colors hover:border-accent hover:text-accent"
              >
                <Plus className="h-4 w-4" aria-hidden="true" />
              </button>
            </div>
          </div>

          <div>
            <label
              htmlFor="add-save-as"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Save As
            </label>
            <div className="flex gap-2">
              <input
                id="add-save-as"
                type="text"
                value={saveAsValue}
                onChange={(e) => onSaveAsChange(e.target.value)}
                onBlur={() => commitSaveAsDraft()}
                placeholder="filename.ext"
                title="You can rename the file here"
                className="h-9 min-w-0 grow rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
              />
              <button
                type="button"
                onClick={() => void onBrowseFolder()}
                disabled={busy}
                title="Choose folder"
                aria-label="Choose folder"
                className="h-9 w-9 shrink-0 rounded-md border border-border bg-surface-2 text-sm font-bold leading-none text-text transition-colors hover:border-accent hover:text-accent"
              >
                …
              </button>
            </div>
          </div>

          <div className="flex items-center gap-2 text-xs text-text-muted">
            <span className="min-w-0 grow truncate">
              {category
                ? `A folder you pick is remembered for “${categoryName}”`
                : 'A folder you pick is remembered for next time'}
            </span>
            {fileSize !== null && (
              <span className="shrink-0 text-[13px] font-semibold text-info">
                {formatBytes(fileSize)}
              </span>
            )}
            {bestRow !== null && (
              <span
                className="shrink-0 text-[13px] font-semibold text-info"
                title={resolved?.warning ?? undefined}
              >
                {bestRow.label} · {bestRow.ext.toUpperCase()}
                {bestRow.needs_merge ? ' · merged' : ''}
              </span>
            )}
            {resolving && (
              <span className="shrink-0 text-[12px] text-text-muted">Resolving video…</span>
            )}
            {fileSize === null && probeError !== null && (
              <span
                className="shrink-0 truncate text-[12px] text-text-muted"
                title={probeError}
              >
                {probeError}
              </span>
            )}
            {fileSize === null && resolveError !== null && (
              <span
                className="shrink-0 truncate text-[12px] text-text-muted"
                title={resolveError}
              >
                {resolveError}
              </span>
            )}
          </div>

          {resolved?.warning ? (
            <p className="text-xs text-warning">{resolved.warning}</p>
          ) : null}

          <div>
            <input
              value={folderDisplay}
              readOnly
              title="Save folder"
              aria-label="Save folder"
              className="h-9 w-full cursor-default rounded-md border border-border-input bg-surface-2/60 px-3 text-[13px] text-text-muted focus:outline-none"
            />
          </div>

          <div>
            <label
              htmlFor="add-description"
              className="mb-1 block text-xs font-semibold text-text-muted"
            >
              Description
            </label>
            <input
              id="add-description"
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Add a description (optional)"
              className="h-9 w-full rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          <div className="flex items-center gap-3">
            <label
              htmlFor="add-cap"
              className="min-w-0 grow text-xs font-semibold text-text-muted"
            >
              Speed cap for this download (KB/s, 0 = no cap)
            </label>
            {/* Round 56 (§7.5): type="text" + inputMode="numeric",
             *  not type="number" — a number input ignores Ctrl+A /
             *  select-all in this webview, so typing appended to
             *  the old value ("0512") instead of replacing it.
             *  Focus selects the whole value, and non-digits are
             *  stripped on input, so the field still only ever
             *  holds a non-negative integer. */}
            <input
              id="add-cap"
              type="text"
              inputMode="numeric"
              value={cap}
              onChange={(e) => setCap(Math.max(0, Number(e.target.value.replace(/[^0-9]/g, '')) || 0))}
              onFocus={(e) => e.target.select()}
              className="h-9 w-32 shrink-0 rounded-md border border-border-input bg-surface px-3 text-[13px] focus:border-accent focus:outline-none"
            />
          </div>

          {error ? (
            <p id="add-error" role="alert" className="text-sm text-danger">
              {error}
            </p>
          ) : null}
        </div>

        {/* Actions — the default-action button is the gradient
         *  primary (with the default setting that is Start
         *  Download, exactly as the mockup shows). Round 85 —
         *  shrink-0 in both variants: the footer is the sticky
         *  part of the flex column, never scrolled away. */}
        <div
          className={
            standalone
              ? 'flex shrink-0 items-center justify-end gap-2.5 px-[22px] pb-[18px] pt-4'
              : 'flex shrink-0 items-center justify-end gap-2.5 px-[22px] pb-[18px] pt-4'
          }
        >
          <Button
            variant={primaryIsLater ? 'primary' : 'secondary'}
            style={primaryIsLater ? primaryStyle : undefined}
            className={primaryIsLater ? 'px-6 hover:brightness-110' : undefined}
            loading={busy}
            onClick={() => void submit(false)}
          >
            Download Later
          </Button>
          <Button
            variant={primaryIsLater ? 'secondary' : 'primary'}
            style={primaryIsLater ? primaryStyle : undefined}
            className={primaryIsLater ? 'px-6 hover:brightness-110' : undefined}
            loading={busy}
            onClick={() => void submit(true)}
          >
            Start Download
          </Button>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  )
}
