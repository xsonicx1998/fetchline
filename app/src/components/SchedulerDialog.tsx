import { useEffect, useRef, useState } from 'react'
import { ArrowDown, ArrowUp, List, Play, Plus, Square, X } from 'lucide-react'
import {
  createQueue,
  getQueueFiles,
  removeFromQueue,
  reorderQueue,
  startQueue,
  stopQueue,
  syncQueueNow,
  updateQueue,
} from '../lib/api'
import {
  datetimeLocalToUtcIso,
  formatDateTime,
  utcIsoToDatetimeLocal,
} from '../lib/format'
import type { DownloadRow, QueueCompleteAction, QueueRow, QueueScheduleType, SyncResult } from '../lib/types'
import { Button } from './Button'

interface SchedulerDialogProps {
  open: boolean
  queues: QueueRow[]
  initialQueueId: string | null
  onClose: () => void
  onChanged: () => void
  onNewQueue: () => void
}

/** S05 — scheduler and queues (components.md: Dialog, Tabs).
 *  Queue list on the left; Schedule and Files in Queue tabs on the
 *  right. One-time schedules drain the queue once; periodic
 *  synchronisation re-probes the queue's finished files every N
 *  minutes (minimum 5) and re-downloads only the files that changed.
 *  A queue is one or the other, never both. */
export function SchedulerDialog({
  open,
  queues,
  initialQueueId,
  onClose,
  onChanged,
  onNewQueue,
}: SchedulerDialogProps) {
  const [selectedId, setSelectedId] = useState<string | null>(initialQueueId)
  const [tab, setTab] = useState<'schedule' | 'files'>('schedule')
  const [scheduleType, setScheduleType] = useState<QueueScheduleType>('none')
  const [startAt, setStartAt] = useState('')
  const [stopAt, setStopAt] = useState('')
  const [syncInterval, setSyncInterval] = useState(30)
  const [startOnStartup, setStartOnStartup] = useState(false)
  const [onComplete, setOnComplete] = useState<QueueCompleteAction>('none')
  const [program, setProgram] = useState('')
  const [files, setFiles] = useState<DownloadRow[]>([])
  const [filesBusy, setFilesBusy] = useState(false)
  const [busy, setBusy] = useState(false)
  const [syncBusy, setSyncBusy] = useState(false)
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  /** Round 88.9 (1) — the scheduler creates its queue
   *  implicitly while this is true: the dialog opens with no
   *  queues and no form, so the empty state would otherwise be
   *  a dead end ("No queues yet — create one to begin"). */
  const [creatingQueue, setCreatingQueue] = useState(false)
  /** Round 88.9 (1) — one implicit creation per open: the
   *  effect re-runs when the queue list refreshes after the
   *  creation, and must not create a second queue. */
  const autoCreatedRef = useRef(false)

  const selected = queues.find((q) => q.id === selectedId) ?? queues[0] ?? null

  useEffect(() => {
    if (open) {
      setSelectedId(initialQueueId ?? queues[0]?.id ?? null)
      setTab('schedule')
      setError(null)
      setSaved(false)
    }
    // Load the freshly opened dialog from props only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  // Round 88.9 (1) — a schedule never needs a hand-made
  // queue: opening the scheduler with no queues creates one
  // implicitly in the background ("Schedule 1", then "Schedule
  // 2", … — the first free name), selects it, and the form is
  // usable at once. The engine still schedules queues; only
  // the creation is implicit. The queue is an ordinary queue —
  // renameable and deletable like any other — so closing the
  // dialog without saving leaves a harmless empty queue behind
  // instead of a dead end.
  useEffect(() => {
    if (!open) {
      autoCreatedRef.current = false
      return
    }
    if (queues.length > 0 || autoCreatedRef.current) return
    autoCreatedRef.current = true
    let cancelled = false
    setCreatingQueue(true)
    const taken = new Set(queues.map((q) => q.name))
    let n = 1
    while (taken.has(`Schedule ${n}`)) n++
    createQueue(`Schedule ${n}`)
      .then((q) => {
        if (cancelled) return
        setSelectedId(q.id)
        onChanged()
      })
      .catch((e: unknown) => {
        if (cancelled) return
        // Let the user retry with "New queue": the guard is
        // released so a later open tries again.
        autoCreatedRef.current = false
        setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setCreatingQueue(false)
      })
    return () => {
      cancelled = true
    }
    // `queues` is read, not watched: the refresh after the
    // creation re-runs nothing (the guard stops a second one).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  // Fill the form from the selected queue. A queue stored as
  // 'one_time' with no start time has no schedule at all (that is
  // how the engine stores "none" — the schema only allows
  // 'one_time'/'periodic'), so it shows as "No schedule".
  useEffect(() => {
    if (!open || !selected) return
    setScheduleType(
      selected.schedule_type === 'one_time' && selected.start_at
        ? 'one_time'
        : selected.schedule_type === 'periodic'
          ? 'periodic'
          : 'none',
    )
    setStartAt(utcIsoToDatetimeLocal(selected.start_at))
    setStopAt(utcIsoToDatetimeLocal(selected.stop_at))
    setSyncInterval(selected.sync_interval_minutes ?? 30)
    setStartOnStartup(selected.start_on_startup)
    setOnComplete(
      (selected.on_complete_action as QueueCompleteAction) || 'none',
    )
    setProgram(selected.on_complete_program ?? '')
    setSyncResult(null)
    setError(null)
    setSaved(false)
  }, [open, selected?.id])

  // Load the files of the selected queue.
  useEffect(() => {
    if (!open || !selected) {
      setFiles([])
      return
    }
    let cancelled = false
    setFilesBusy(true)
    getQueueFiles(selected.id)
      .then((rows) => {
        if (!cancelled) setFiles(rows)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setFilesBusy(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, selected?.id])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  async function refreshFiles() {
    if (!selected) return
    try {
      setFiles(await getQueueFiles(selected.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function save() {
    if (!selected) return
    const timed = scheduleType === 'one_time' || scheduleType === 'periodic'
    const startIso = timed ? datetimeLocalToUtcIso(startAt) : null
    const stopIso = timed ? datetimeLocalToUtcIso(stopAt) : null
    if (scheduleType === 'one_time' && !startIso) {
      setError('Pick a start time for the one-time schedule.')
      return
    }
    if (scheduleType === 'periodic') {
      if (!Number.isFinite(syncInterval) || syncInterval < 5) {
        setError('Check for changes at least every 5 minutes.')
        return
      }
      if (stopIso && !startIso) {
        setError('Set a start time to use a stop time.')
        return
      }
    }
    if (startIso && stopIso && stopIso <= startIso) {
      setError('The stop time must be after the start time.')
      return
    }
    if (onComplete === 'run_program' && !program.trim()) {
      setError('Give the program to run when the queue finishes.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      await updateQueue(selected.id, {
        schedule_type: scheduleType,
        start_at: startIso,
        stop_at: stopIso,
        start_on_startup: startOnStartup,
        on_complete_action: onComplete,
        on_complete_program: onComplete === 'run_program' ? program.trim() : null,
        sync_interval_minutes: scheduleType === 'periodic' ? Math.floor(syncInterval) : null,
      })
      setSaved(true)
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function syncNow() {
    if (!selected) return
    setSyncBusy(true)
    setError(null)
    setSyncResult(null)
    try {
      const result = await syncQueueNow(selected.id)
      setSyncResult(result)
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSyncBusy(false)
    }
  }

  async function moveFile(downloadId: string, newIndex: number) {
    if (!selected) return
    try {
      await reorderQueue(selected.id, downloadId, newIndex)
      await refreshFiles()
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function removeFile(downloadId: string) {
    try {
      await removeFromQueue(downloadId)
      await refreshFiles()
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  async function runQueue(action: 'start' | 'stop') {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      if (action === 'start') await startQueue(selected.id)
      else await stopQueue(selected.id)
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="scheduler-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="flex h-full max-h-full w-full max-w-3xl flex-col overflow-hidden rounded-lg border border-border bg-surface-raised shadow-pop">
        <div className="flex shrink-0 items-start justify-between gap-4 border-b border-border px-6 py-4">
          <h2 id="scheduler-title" className="text-lg font-semibold">
            Queues and scheduler
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

        <div className="flex min-h-0 flex-1">
          {/* Queue list */}
          <div className="flex w-56 shrink-0 flex-col gap-1 overflow-y-auto border-r border-border p-2">
            {queues.map((q) => (
              <button
                key={q.id}
                type="button"
                onClick={() => setSelectedId(q.id)}
                aria-current={selected?.id === q.id || undefined}
                className={`flex items-center gap-2 rounded-md px-2 py-2 text-left text-base ${selected?.id === q.id ? 'bg-accent-soft font-semibold text-accent' : 'text-text hover:bg-surface-2'}`}
              >
                <List className="h-4 w-4 shrink-0" aria-hidden="true" />
                {/* Round 88.9 (4) — the name truncates in this
                    narrow list, so hover shows the full name. */}
                <span className="min-w-0 flex-1 truncate" title={q.name}>{q.name}</span>
                <span className="shrink-0 text-xs text-text-muted">
                  {q.file_count} file{q.file_count === 1 ? '' : 's'}
                </span>
                {q.running ? (
                  <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-xs font-semibold text-on-accent">
                    Running
                  </span>
                ) : null}
              </button>
            ))}
            <button
              type="button"
              onClick={onNewQueue}
              className="mt-1 flex items-center gap-2 rounded-md px-2 py-2 text-left text-base text-text-muted hover:bg-surface-2"
            >
              <Plus className="h-4 w-4" aria-hidden="true" />
              New queue
            </button>
          </div>

          {/* Tabs + content */}
          <div className="flex min-w-0 flex-1 flex-col">
            <div role="tablist" aria-label="Queue settings" className="flex shrink-0 gap-1 border-b border-border px-4 pt-2">
              <button
                role="tab"
                type="button"
                aria-selected={tab === 'schedule'}
                onClick={() => setTab('schedule')}
                className={`rounded-t-md px-3 py-2 text-base font-semibold ${tab === 'schedule' ? 'bg-surface-2 text-accent' : 'text-text-muted hover:bg-surface-2'}`}
              >
                Schedule
              </button>
              <button
                role="tab"
                type="button"
                aria-selected={tab === 'files'}
                onClick={() => setTab('files')}
                className={`rounded-t-md px-3 py-2 text-base font-semibold ${tab === 'files' ? 'bg-surface-2 text-accent' : 'text-text-muted hover:bg-surface-2'}`}
              >
                Files in queue ({files.length})
              </button>
            </div>

            {selected === null ? (
              <div className="p-6">
                <p className="text-base text-text-muted">
                  {creatingQueue
                    ? 'Creating a schedule…'
                    : 'No queues yet — create one to begin.'}
                </p>
                {/* Round 88.9 (1) — an implicit creation that
                    fails must say so: the error otherwise only
                    renders inside the selected-queue branches,
                    and with no queue the failure was silent. */}
                {error ? (
                  <p role="alert" className="mt-2 text-sm text-danger">
                    {error}
                  </p>
                ) : null}
              </div>
            ) : tab === 'schedule' ? (
              <div className="flex flex-col gap-4 overflow-y-auto p-6">
                <div>
                  <span className="mb-1 block text-sm font-semibold">Schedule</span>
                  <label className="flex items-center gap-2 py-1 text-base">
                    <input
                      type="radio"
                      name="schedule-type"
                      checked={scheduleType === 'none'}
                      onChange={() => setScheduleType('none')}
                    />
                    No schedule — start manually
                  </label>
                  <label className="flex items-center gap-2 py-1 text-base">
                    <input
                      type="radio"
                      name="schedule-type"
                      checked={scheduleType === 'one_time'}
                      onChange={() => setScheduleType('one_time')}
                    />
                    One time — start and stop at set times
                  </label>
                  <label className="flex items-center gap-2 py-1 text-base">
                    <input
                      type="radio"
                      name="schedule-type"
                      checked={scheduleType === 'periodic'}
                      onChange={() => setScheduleType('periodic')}
                    />
                    Repeat regularly — check the files for changes
                  </label>
                </div>

                {scheduleType === 'periodic' ? (
                  <div className="flex flex-col gap-4">
                    <div>
                      <label htmlFor="sched-interval" className="mb-1 block text-sm font-semibold">
                        Check for changes every
                      </label>
                      <div className="flex items-center gap-2">
                        <input
                          id="sched-interval"
                          type="number"
                          min={5}
                          max={10080}
                          step={1}
                          value={syncInterval}
                          onChange={(e) => setSyncInterval(Number(e.target.value))}
                          className="h-8 w-24 rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                        />
                        <span className="text-base text-text-muted">minutes (minimum 5)</span>
                      </div>
                      <p className="mt-1 text-xs text-text-muted">
                        Only files that changed on the server are downloaded again. Unchanged files are skipped.
                      </p>
                    </div>
                    <div className="grid grid-cols-2 gap-4">
                      <div>
                        <label htmlFor="sched-start" className="mb-1 block text-sm font-semibold">
                          Start at <span className="font-normal text-text-muted">(optional)</span>
                        </label>
                        <input
                          id="sched-start"
                          type="datetime-local"
                          value={startAt}
                          onChange={(e) => setStartAt(e.target.value)}
                          className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                        />
                      </div>
                      <div>
                        <label htmlFor="sched-stop" className="mb-1 block text-sm font-semibold">
                          Stop at <span className="font-normal text-text-muted">(optional)</span>
                        </label>
                        <input
                          id="sched-stop"
                          type="datetime-local"
                          value={stopAt}
                          onChange={(e) => setStopAt(e.target.value)}
                          className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                        />
                      </div>
                    </div>
                    {selected.schedule_type === 'periodic' ? (
                      <div className="rounded-md border border-border bg-surface px-3 py-2">
                        <p className="text-sm text-text-muted">
                          {selected.last_sync_at
                            ? `Last sync ${formatDateTime(selected.last_sync_at)} — ${selected.last_sync_changed} changed, ${selected.last_sync_unchanged} unchanged, ${selected.last_sync_removed} no longer on the server, ${selected.last_sync_failed} failed, ${selected.last_sync_skipped} skipped.`
                            : 'Never synced yet.'}
                        </p>
                        <div className="mt-2 flex items-center gap-2">
                          <Button variant="secondary" size="sm" loading={syncBusy} onClick={() => void syncNow()}>
                            Sync now
                          </Button>
                          {syncResult ? (
                            <p role="status" className="text-sm text-accent">
                              Synced: {syncResult.changed} changed, {syncResult.unchanged} unchanged
                              {syncResult.removed > 0 ? `, ${syncResult.removed} removed` : ''}
                              {syncResult.failed > 0 ? `, ${syncResult.failed} failed` : ''}
                              {syncResult.skipped > 0 ? `, ${syncResult.skipped} skipped` : ''}.
                            </p>
                          ) : null}
                        </div>
                        {syncResult && syncResult.files.some((f) => f.status === 'removed' || f.status === 'failed') ? (
                          <ul className="mt-2 flex flex-col gap-1">
                            {syncResult.files
                              .filter((f) => f.status === 'removed' || f.status === 'failed')
                              .map((f) => (
                                <li key={f.download_id} className="text-sm text-text-muted">
                                  {f.filename} — {f.status}
                                  {f.message ? `: ${f.message}` : ''}
                                </li>
                              ))}
                          </ul>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                ) : null}

                {scheduleType === 'one_time' ? (
                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <label htmlFor="sched-start" className="mb-1 block text-sm font-semibold">
                        Start at
                      </label>
                      <input
                        id="sched-start"
                        type="datetime-local"
                        value={startAt}
                        onChange={(e) => setStartAt(e.target.value)}
                        className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                      />
                    </div>
                    <div>
                      <label htmlFor="sched-stop" className="mb-1 block text-sm font-semibold">
                        Stop at <span className="font-normal text-text-muted">(optional)</span>
                      </label>
                      <input
                        id="sched-stop"
                        type="datetime-local"
                        value={stopAt}
                        onChange={(e) => setStopAt(e.target.value)}
                        className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
                      />
                    </div>
                  </div>
                ) : null}

                <label className="flex items-center gap-2 text-base">
                  <input
                    type="checkbox"
                    checked={startOnStartup}
                    onChange={(e) => setStartOnStartup(e.target.checked)}
                  />
                  Start this queue when Fetchline starts
                </label>

                <div>
                  <label htmlFor="sched-complete" className="mb-1 block text-sm font-semibold">
                    When the queue finishes
                  </label>
                  <select
                    id="sched-complete"
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
                    <p className="mt-1 text-xs text-text-muted">
                      You will get a 60-second countdown with a cancel button before it happens.
                    </p>
                  ) : null}
                </div>

                {selected.schedule_type === 'periodic' && selected.sync_interval_minutes ? (
                  <p className="text-sm text-text-muted">
                    Checks this queue every {selected.sync_interval_minutes} minutes
                    {selected.start_at ? ` from ${formatDateTime(selected.start_at)}` : ''}
                    {selected.stop_at ? ` until ${formatDateTime(selected.stop_at)}` : ''}.
                  </p>
                ) : null}

                {selected.schedule_type === 'one_time' && selected.start_at ? (
                  <p className="text-sm text-text-muted">
                    Currently scheduled to start {formatDateTime(selected.start_at)}
                    {selected.stop_at ? ` and stop ${formatDateTime(selected.stop_at)}` : ''}.
                  </p>
                ) : null}

                {error ? (
                  <p role="alert" className="text-sm text-danger">
                    {error}
                  </p>
                ) : null}
                {saved ? (
                  <p role="status" className="text-sm text-accent">
                    Schedule saved.
                  </p>
                ) : null}

                <div className="flex justify-end gap-2">
                  <Button variant="ghost" onClick={onClose}>
                    Close
                  </Button>
                  <Button variant="primary" loading={busy} onClick={() => void save()}>
                    Save schedule
                  </Button>
                </div>
              </div>
            ) : (
              <div className="flex flex-col gap-2 overflow-y-auto p-6">
                <div className="mb-2 flex items-center justify-between gap-2">
                  <p className="text-sm text-text-muted">
                    Files download one after another, in this order. A finished file leaves the queue.
                  </p>
                  {selected.running ? (
                    <Button variant="secondary" size="sm" loading={busy} onClick={() => void runQueue('stop')}>
                      <Square className="h-4 w-4" aria-hidden="true" /> Stop queue
                    </Button>
                  ) : (
                    <Button
                      variant="primary"
                      size="sm"
                      loading={busy}
                      disabled={files.length === 0}
                      onClick={() => void runQueue('start')}
                    >
                      <Play className="h-4 w-4" aria-hidden="true" /> Start now
                    </Button>
                  )}
                </div>
                {filesBusy ? (
                  <p className="text-base text-text-muted">Loading…</p>
                ) : files.length === 0 ? (
                  <p className="text-base text-text-muted">
                    This queue is empty. Add a link and choose “Save for later”, or drag a download here from the main list.
                  </p>
                ) : (
                  files.map((f, i) => (
                    <div
                      key={f.id}
                      className="flex items-center gap-2 rounded-md border border-border bg-surface px-3 py-2"
                    >
                      <span className="w-6 shrink-0 text-center font-mono text-sm text-text-muted">{i + 1}</span>
                      <span className="min-w-0 flex-1 truncate text-base" title={f.filename}>
                        {f.filename}
                      </span>
                      <span className="shrink-0 text-xs text-text-muted">{f.status}</span>
                      <button
                        type="button"
                        disabled={i === 0}
                        onClick={() => void moveFile(f.id, i - 1)}
                        aria-label={`Move ${f.filename} up`}
                        className="rounded-md p-1 text-text-muted hover:bg-surface-2 disabled:opacity-30"
                      >
                        <ArrowUp className="h-4 w-4" aria-hidden="true" />
                      </button>
                      <button
                        type="button"
                        disabled={i === files.length - 1}
                        onClick={() => void moveFile(f.id, i + 1)}
                        aria-label={`Move ${f.filename} down`}
                        className="rounded-md p-1 text-text-muted hover:bg-surface-2 disabled:opacity-30"
                      >
                        <ArrowDown className="h-4 w-4" aria-hidden="true" />
                      </button>
                      <button
                        type="button"
                        onClick={() => void removeFile(f.id)}
                        aria-label={`Remove ${f.filename} from the queue`}
                        className="rounded-md p-1 text-text-muted hover:bg-surface-2"
                      >
                        <X className="h-4 w-4" aria-hidden="true" />
                      </button>
                    </div>
                  ))
                )}
                {error ? (
                  <p role="alert" className="text-sm text-danger">
                    {error}
                  </p>
                ) : null}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
