import { useEffect, useRef, useState } from 'react'
import { Globe, Square, X } from 'lucide-react'
import {
  createGrabberProject,
  getGrabberProject,
  runGrabber,
  stopGrabber,
} from '../lib/api'
import { completeActionLabel, datetimeLocalToUtcIso } from '../lib/format'
import type { GrabberProjectRow, QueueCompleteAction, QueueRow } from '../lib/types'
import { Button } from './Button'

interface SiteGrabberDialogProps {
  open: boolean
  queues: QueueRow[]
  onClose: () => void
  onAdded: () => void
}

const TYPE_OPTIONS: { value: string; label: string }[] = [
  { value: 'image', label: 'Images' },
  { value: 'document', label: 'Documents' },
  { value: 'video', label: 'Video' },
  { value: 'audio', label: 'Audio' },
  { value: 'archive', label: 'Archives' },
  { value: 'program', label: 'Programs' },
]

/** The safe default filter: images + documents only. Programs
 *  (executables) are never grabbed unless the user ticks them. */
const SAFE_DEFAULT_TYPES = ['image', 'document']

/** S06 — site grabber wizard (recon F06). Three steps: (1) project
 *  name + start address, (2) file-type filters + explore depth +
 *  page limit, (3) download vs explore-only + an optional one-time
 *  start time (the existing scheduler — no second scheduler) + the
 *  queue grabbed files go to (Milestone 4, default: main queue) +
 *  an on-completion action reusing the existing queue actions
 *  (Milestone 4 — armed only when the run actually downloaded
 *  something; shutdown / hibernate get the existing 60 s
 *  cancellable countdown). Run opens the progress view: pages
 *  explored, files matched / downloaded / failed, and the live
 *  list of matched files.
 *  The crawler itself is polite by construction (same host only,
 *  depth ≤ 5, page cap, 500 ms between pages, robots.txt honoured)
 *  — the wizard only exposes the safe knobs. */
export function SiteGrabberDialog({ open, queues, onClose, onAdded }: SiteGrabberDialogProps) {
  const [step, setStep] = useState(1)
  const [name, setName] = useState('')
  const [startUrl, setStartUrl] = useState('')
  const [types, setTypes] = useState<string[]>([...SAFE_DEFAULT_TYPES])
  const [depth, setDepth] = useState(2)
  const [maxPages, setMaxPages] = useState(200)
  const [mode, setMode] = useState<'download' | 'explore'>('download')
  const [schedule, setSchedule] = useState(false)
  const [startAt, setStartAt] = useState('')
  const [queueId, setQueueId] = useState('queue-main')
  const [onComplete, setOnComplete] = useState<QueueCompleteAction>('none')
  const [program, setProgram] = useState('')
  const [project, setProject] = useState<GrabberProjectRow | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setStep(1)
      setName('')
      setStartUrl('')
      setTypes([...SAFE_DEFAULT_TYPES])
      setDepth(2)
      setMaxPages(200)
      setMode('download')
      setSchedule(false)
      setStartAt('')
      setQueueId('queue-main')
      setOnComplete('none')
      setProgram('')
      setProject(null)
      setError(null)
      setBusy(false)
      nameRef.current?.focus()
    }
  }, [open ])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  // Poll the running project once a second while it is active.
  const running =
    project !== null &&
    (project.status === 'exploring' || project.status === 'downloading')
  useEffect(() => {
    if (!running || !project) return
    const timer = window.setInterval(() => {
      void getGrabberProject(project.id)
        .then((p) => {
          setProject(p)
          if (p.status === 'done' || p.status === 'failed' || p.status === 'stopped') {
            onAdded()
          }
        })
        .catch(() => undefined)
    }, 1000)
    return () => window.clearInterval(timer)
  }, [running, project, onAdded])

  if (!open) return null

  const urlValid = startUrl.trim() === '' || /^https?:\/\/.+/i.test(startUrl.trim())
  const step1Valid = name.trim() !== '' && urlValid && startUrl.trim() !== ''
  const step2Valid = types.length > 0 && depth >= 0 && depth <= 5 && maxPages >= 1 && maxPages <= 1000

  function toggleType(value: string) {
    setTypes((prev) =>
      prev.includes(value) ? prev.filter((t) => t !== value) : [...prev, value],
    )
  }

  async function run() {
    if (schedule && startAt.trim() === '') {
      setError('Pick a start time, or turn the schedule off.')
      return
    }
    if (onComplete === 'run_program' && program.trim() === '') {
      setError('Give the program to run when the grabber finishes.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const created = await createGrabberProject({
        name: name.trim(),
        startUrl: startUrl.trim(),
        types,
        depth,
        maxPages,
        mode,
        scheduleStartAt: schedule ? datetimeLocalToUtcIso(startAt) : null,
        queueId,
        onCompleteAction: onComplete,
        onCompleteProgram: onComplete === 'run_program' ? program.trim() : null,
      })
      setProject(created)
      // A scheduled project starts on its own when the time arrives
      // (the scheduler tick); Run now starts it at once.
      if (!schedule) {
        await runGrabber(created.id)
        setProject(await getGrabberProject(created.id))
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const finished =
    project !== null &&
    (project.status === 'done' || project.status === 'failed' || project.status === 'stopped')
  const capped =
    finished &&
    project.status === 'done' &&
    project.pages_explored >= project.filters.max_pages

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="site-grabber-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="flex max-h-full w-full max-w-2xl flex-col rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="site-grabber-title" className="flex items-center gap-2 text-lg font-semibold">
            <Globe className="h-5 w-5 text-accent" aria-hidden="true" />
            Site grabber
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

        {project === null ? (
          <div className="flex flex-col gap-4">
            <ol className="flex gap-2 text-sm" aria-label="Wizard steps">
              {['Project', 'What to grab', 'How to run it'].map((label, i) => (
                <li
                  key={label}
                  aria-current={step === i + 1 ? 'step' : undefined}
                  className={`rounded-full px-3 py-1 font-semibold ${
                    step === i + 1 ? 'bg-accent text-white' : 'bg-surface-2 text-text-muted'
                  }`}
                >
                  {i + 1}. {label}
                </li>
              ))}
            </ol>

            {step === 1 ? (
              <div className="flex flex-col gap-4">
                <div>
                  <label htmlFor="grabber-name" className="mb-1 block text-sm font-semibold">
                    Project name
                  </label>
                  <input
                    ref={nameRef}
                    id="grabber-name"
                    type="text"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="My gallery"
                    className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                  />
                </div>
                <div>
                  <label htmlFor="grabber-url" className="mb-1 block text-sm font-semibold">
                    Start address
                  </label>
                  <input
                    id="grabber-url"
                    type="url"
                    value={startUrl}
                    onChange={(e) => setStartUrl(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && step1Valid) setStep(2)
                    }}
                    placeholder="https://example.com/gallery.html"
                    aria-invalid={!urlValid || undefined}
                    className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                  />
                  <p className="mt-1 text-sm text-text-muted">
                    Only pages on this same site are followed — links to other sites are never crawled.
                  </p>
                </div>
              </div>
            ) : step === 2 ? (
              <div className="flex flex-col gap-4">
                <fieldset>
                  <legend className="mb-1 text-sm font-semibold">File types to grab</legend>
                  <div className="flex flex-wrap gap-2">
                    {TYPE_OPTIONS.map((t) => (
                      <label
                        key={t.value}
                        className="flex cursor-pointer items-center gap-2 rounded-md border border-border-input bg-bg px-3 py-2 text-sm"
                      >
                        <input
                          type="checkbox"
                          checked={types.includes(t.value)}
                          onChange={() => toggleType(t.value)}
                          className="h-4 w-4 accent-accent"
                        />
                        {t.label}
                      </label>
                    ))}
                  </div>
                  <p className="mt-1 text-sm text-text-muted">
                    Images and documents are picked by default. Programs (executables) are only
                    grabbed when you tick them.
                  </p>
                </fieldset>
                <div className="flex gap-4">
                  <div className="flex-1">
                    <label htmlFor="grabber-depth" className="mb-1 block text-sm font-semibold">
                      Explore depth
                    </label>
                    <input
                      id="grabber-depth"
                      type="number"
                      min={0}
                      max={5}
                      value={depth}
                      onChange={(e) => setDepth(Number(e.target.value))}
                      className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                    />
                    <p className="mt-1 text-sm text-text-muted">0 = the start page only, up to 5.</p>
                  </div>
                  <div className="flex-1">
                    <label htmlFor="grabber-max-pages" className="mb-1 block text-sm font-semibold">
                      Page limit
                    </label>
                    <input
                      id="grabber-max-pages"
                      type="number"
                      min={1}
                      max={1000}
                      value={maxPages}
                      onChange={(e) => setMaxPages(Number(e.target.value))}
                      className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                    />
                    <p className="mt-1 text-sm text-text-muted">The crawl stops at this many pages.</p>
                  </div>
                </div>
              </div>
            ) : (
              <div className="flex flex-col gap-4">
                <fieldset>
                  <legend className="mb-1 text-sm font-semibold">What should happen to the files?</legend>
                  <label className="flex items-center gap-2 py-1 text-sm">
                    <input
                      type="radio"
                      name="grabber-mode"
                      checked={mode === 'download'}
                      onChange={() => setMode('download')}
                      className="h-4 w-4 accent-accent"
                    />
                    Download the matched files
                  </label>
                  <label className="flex items-center gap-2 py-1 text-sm">
                    <input
                      type="radio"
                      name="grabber-mode"
                      checked={mode === 'explore'}
                      onChange={() => setMode('explore')}
                      className="h-4 w-4 accent-accent"
                    />
                    Explore only — list the matched files without downloading them
                  </label>
                </fieldset>
                <label className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={schedule}
                    onChange={(e) => setSchedule(e.target.checked)}
                    className="h-4 w-4 accent-accent"
                  />
                  Start at a scheduled time instead of now
                </label>
                {schedule ? (
                  <div>
                    <label htmlFor="grabber-start-at" className="mb-1 block text-sm font-semibold">
                      Start time
                    </label>
                    <input
                      id="grabber-start-at"
                      type="datetime-local"
                      value={startAt}
                      onChange={(e) => setStartAt(e.target.value)}
                      className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                    />
                    <p className="mt-1 text-sm text-text-muted">
                      The existing scheduler starts the project when the time arrives.
                    </p>
                  </div>
                ) : null}
                <div>
                  <label htmlFor="grabber-queue" className="mb-1 block text-sm font-semibold">
                    Queue for the grabbed files
                  </label>
                  <select
                    id="grabber-queue"
                    value={queueId}
                    onChange={(e) => setQueueId(e.target.value)}
                    className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                  >
                    {queues.map((q) => (
                      <option key={q.id} value={q.id}>
                        {q.name}
                      </option>
                    ))}
                  </select>
                  <p className="mt-1 text-sm text-text-muted">
                    Grabbed files are filed into this queue. The main queue is used when no
                    other queue is picked.
                  </p>
                </div>
                <div>
                  <label htmlFor="grabber-complete" className="mb-1 block text-sm font-semibold">
                    When the grabber finishes
                  </label>
                  <select
                    id="grabber-complete"
                    value={onComplete}
                    onChange={(e) => setOnComplete(e.target.value as QueueCompleteAction)}
                    className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                  >
                    <option value="none">Do nothing</option>
                    <option value="open_file">Open the last file</option>
                    <option value="run_program">Run a program</option>
                    <option value="shutdown">Shut down the computer</option>
                    <option value="hibernate">Hibernate the computer</option>
                  </select>
                  {onComplete === 'run_program' ? (
                    <input
                      type="text"
                      value={program}
                      onChange={(e) => setProgram(e.target.value)}
                      placeholder="C:\Tools\backup.bat"
                      aria-label="Program to run"
                      className="mt-2 h-8 w-full rounded-md border border-border-input bg-bg px-3 font-mono text-base focus:border-accent"
                    />
                  ) : null}
                  {onComplete === 'shutdown' || onComplete === 'hibernate' ? (
                    <p className="mt-1 text-sm text-text-muted">
                      You will get a 60-second countdown with a cancel button before it happens.
                    </p>
                  ) : null}
                  {onComplete !== 'none' ? (
                    <p className="mt-1 text-sm text-text-muted">
                      Only runs when the grabber actually downloaded at least one file — a run
                      that matched nothing, or explore-only, runs no action.
                    </p>
                  ) : null}
                </div>
                <p className="rounded-md border border-border bg-bg p-3 text-sm text-text-muted">
                  The grabber waits at least half a second between pages, honours the site&apos;s
                  robots.txt, and gives up on a file after 2 failed attempts. Pages that need a
                  login are reported, not bypassed.
                </p>
              </div>
            )}

            {error ? (
              <p role="alert" className="text-sm text-danger">
                {error}
              </p>
            ) : null}
            <div className="flex justify-between gap-2">
              <Button variant="ghost" onClick={() => (step === 1 ? onClose() : setStep(step - 1))}>
                {step === 1 ? 'Cancel' : 'Back'}
              </Button>
              {step < 3 ? (
                <Button
                  variant="primary"
                  disabled={step === 1 ? !step1Valid : !step2Valid}
                  onClick={() => setStep(step + 1)}
                >
                  Next
                </Button>
              ) : (
                <Button variant="primary" loading={busy} onClick={() => void run()}>
                  {schedule ? 'Save scheduled grabber' : 'Run site grabber'}
                </Button>
              )}
            </div>
          </div>
        ) : (
          <div className="flex min-h-0 flex-col gap-4">
            <div>
              <p className="text-sm font-semibold">{project.name}</p>
              <p className="truncate font-mono text-xs text-text-muted" title={project.start_url}>
                {project.start_url}
              </p>
              <p className="mt-1 text-xs text-text-muted">
                Files go to {(queues.find((q) => q.id === project.queue_id)?.name ?? 'Main download queue')}
                {project.on_complete_action !== 'none'
                  ? ` · ${completeActionLabel(project.on_complete_action)}`
                  : ''}
              </p>
            </div>
            <div className="grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
              <div className="rounded-md border border-border bg-bg p-3">
                <p className="text-xs text-text-muted">Pages explored</p>
                <p className="text-lg font-semibold">{project.pages_explored}</p>
              </div>
              <div className="rounded-md border border-border bg-bg p-3">
                <p className="text-xs text-text-muted">Files matched</p>
                <p className="text-lg font-semibold">{project.files_matched}</p>
              </div>
              <div className="rounded-md border border-border bg-bg p-3">
                <p className="text-xs text-text-muted">Downloaded</p>
                <p className="text-lg font-semibold">{project.files_downloaded}</p>
              </div>
              <div className="rounded-md border border-border bg-bg p-3">
                <p className="text-xs text-text-muted">Failed</p>
                <p className="text-lg font-semibold">{project.files_failed}</p>
              </div>
            </div>
            <p className="text-sm" aria-live="polite">
              {project.status === 'exploring' && 'Exploring the site…'}
              {project.status === 'downloading' && 'Downloading the matched files…'}
              {project.status === 'done' && `Finished — ${project.files_downloaded} downloaded, ${project.files_failed} failed.`}
              {project.status === 'failed' && `Failed — ${project.error_message ?? 'the site could not be grabbed.'}`}
              {project.status === 'stopped' && 'Stopped — the files found so far are listed below.'}
              {project.status === 'idle' && 'Waiting for its scheduled start time.'}
            </p>
            {capped ? (
              <p className="rounded-md border border-border bg-bg p-3 text-sm text-text-muted">
                Stopped at the page limit ({project.filters.max_pages} pages explored) — the site
                may have more pages than were visited.
              </p>
            ) : null}
            {project.status === 'failed' && project.error_message ? (
              <p role="alert" className="text-sm text-danger">
                {project.error_message}
              </p>
            ) : null}
            {project.matched_files.length === 0 ? (
              <p className="rounded-md border border-border bg-bg p-4 text-sm text-text-muted">
                {running ? 'No files matched yet.' : 'No files matched the filters on this site.'}
              </p>
            ) : (
              <ul className="max-h-80 min-h-0 overflow-auto rounded-md border border-border">
                {project.matched_files.map((f) => (
                  <li
                    key={f.url}
                    className="flex items-center gap-3 border-b border-border px-3 py-2 last:border-b-0"
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-semibold" title={f.url}>
                        {f.filename}
                      </span>
                      <span className="block truncate font-mono text-xs text-text-muted" title={f.url}>
                        {f.url}
                      </span>
                      {f.error ? (
                        <span className="block text-xs text-danger">{f.error}</span>
                      ) : null}
                    </span>
                    <span className="shrink-0 rounded-full bg-surface-2 px-2 py-0.5 text-xs font-semibold text-text-muted">
                      {f.kind}
                    </span>
                    <span className="w-20 shrink-0 text-right text-xs font-semibold">
                      {f.status === 'downloaded'
                        ? 'Downloaded'
                        : f.status === 'failed'
                          ? 'Failed'
                          : 'Matched'}
                    </span>
                  </li>
                ))}
              </ul>
            )}
            <div className="flex justify-end gap-2">
              {running ? (
                <Button
                  variant="secondary"
                  onClick={() => {
                    void stopGrabber(project.id).catch((e: unknown) =>
                      setError(e instanceof Error ? e.message : String(e)),
                    )
                  }}
                >
                  <Square className="h-4 w-4" aria-hidden="true" /> Stop
                </Button>
              ) : null}
              <Button
                variant="primary"
                onClick={() => {
                  onAdded()
                  onClose()
                }}
              >
                {finished ? 'Done' : 'Close'}
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
