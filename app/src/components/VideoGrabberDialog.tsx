import { useEffect, useRef, useState } from 'react'
import { Clapperboard, X } from 'lucide-react'
import { addDownload, detectPageVideos } from '../lib/api'
import { formatBytes } from '../lib/format'
import type { DetectedVideo, QueueRow } from '../lib/types'
import { Button } from './Button'

interface VideoGrabberDialogProps {
  open: boolean
  queues: QueueRow[]
  onClose: () => void
  /** The user picked a video: open the S02 add dialog prefilled
   *  with that URL (the dialog, not this panel, starts it). */
  onPick: (url: string) => void
  onAdded: () => void
}

/** S08 — video grabber panel (desktop mirror of the browser
 *  extension panel). Paste a page link, scan it for embedded
 *  video/audio, and pick a quality/format to download. Only openly
 *  downloadable streams are listed: a dead link shows its probe
 *  error instead of a download button, a live stream is labelled as
 *  a playlist file (saved as-is, never merged into one video), and
 *  a protected stream never appears as downloadable — the probe
 *  fails honestly and the panel says so. "Download" opens the S02
 *  add dialog prefilled; "Later" files the video in a queue. */
export function VideoGrabberDialog({ open, queues, onClose, onPick, onAdded }: VideoGrabberDialogProps) {
  const [pageUrl, setPageUrl] = useState('')
  const [videos, setVideos] = useState<DetectedVideo[]>([])
  const [scanned, setScanned] = useState(false)
  const [queueId, setQueueId] = useState('queue-main')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [addedNote, setAddedNote] = useState<string | null>(null)
  const urlRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) {
      setPageUrl('')
      setVideos([])
      setScanned(false)
      setQueueId('queue-main')
      setBusy(false)
      setError(null)
      setAddedNote(null)
      urlRef.current?.focus()
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

  if (!open) return null

  const pageValid = pageUrl.trim() === '' || /^https?:\/\/.+/i.test(pageUrl.trim())

  async function scan() {
    if (!pageUrl.trim() || !pageValid) {
      setError('Enter a page link starting with http:// or https://')
      return
    }
    setBusy(true)
    setError(null)
    setAddedNote(null)
    try {
      setVideos(await detectPageVideos(pageUrl.trim()))
      setScanned(true)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function addLater(video: DetectedVideo) {
    setBusy(true)
    setError(null)
    try {
      await addDownload({ url: video.url, queue: queueId })
      setAddedNote(`${video.filename} was added to the queue.`)
      onAdded()
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
      aria-labelledby="video-grabber-title"
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/50 p-4"
    >
      <div className="flex max-h-full w-full max-w-2xl flex-col rounded-lg border border-border bg-surface-raised p-6 shadow-pop">
        <div className="mb-4 flex items-start justify-between gap-4">
          <h2 id="video-grabber-title" className="flex items-center gap-2 text-lg font-semibold">
            <Clapperboard className="h-5 w-5 text-accent" aria-hidden="true" />
            Grab videos from a page
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

        <div className="flex flex-col gap-4">
          <div>
            <label htmlFor="video-grabber-page" className="mb-1 block text-sm font-semibold">
              Page link
            </label>
            <input
              ref={urlRef}
              id="video-grabber-page"
              type="url"
              value={pageUrl}
              onChange={(e) => setPageUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void scan()
              }}
              placeholder="https://example.com/watch"
              aria-invalid={!pageValid || undefined}
              className="h-8 w-full rounded-md border border-border-input bg-bg px-3 text-base focus:border-accent"
            />
            <p className="mt-1 text-sm text-text-muted">
              Only openly downloadable videos are listed. Protected streams and live
              streams that can't be saved as a file are shown as not downloadable.
            </p>
          </div>

          {error ? (
            <p role="alert" className="text-sm text-danger">
              {error}
            </p>
          ) : null}
          {addedNote ? <p className="text-sm text-text-muted">{addedNote}</p> : null}

          {!scanned ? (
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button variant="primary" loading={busy} onClick={() => void scan()}>
                Find videos
              </Button>
            </div>
          ) : videos.length === 0 ? (
            <div className="flex flex-col gap-4">
              <p className="rounded-md border border-border bg-bg p-4 text-sm text-text-muted">
                No downloadable video or audio found on this page.
              </p>
              <div className="flex justify-end gap-2">
                <Button variant="ghost" onClick={() => setScanned(false)}>
                  Back
                </Button>
                <Button variant="primary" onClick={onClose}>
                  Done
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex min-h-0 flex-col gap-4">
              <ul className="max-h-80 min-h-0 overflow-auto rounded-md border border-border">
                {videos.map((video) => (
                  <li
                    key={video.url}
                    className="flex items-center gap-3 border-b border-border px-3 py-2 last:border-b-0"
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-sm font-semibold" title={video.url}>
                        {video.filename}
                      </span>
                      <span className="block truncate font-mono text-xs text-text-muted" title={video.url}>
                        {video.url}
                      </span>
                      {video.live ? (
                        <span className="block text-xs text-text-muted">
                          Live stream — saved as its playlist file, not merged into one video.
                        </span>
                      ) : null}
                      {video.note ? (
                        <span className="block text-xs text-text-muted">{video.note}</span>
                      ) : null}
                      {video.error ? (
                        <span className="block text-xs text-danger">{video.error}</span>
                      ) : null}
                    </span>
                    <span className="shrink-0 rounded-full bg-surface-2 px-2 py-0.5 text-xs font-semibold text-text-muted">
                      {video.format} · {video.quality}
                    </span>
                    <span className="w-16 shrink-0 text-right font-mono text-xs text-text-muted">
                      {video.size !== null ? formatBytes(video.size) : '—'}
                    </span>
                    {video.downloadable ? (
                      <>
                        <Button variant="secondary" size="sm" disabled={busy} onClick={() => void addLater(video)}>
                          Later
                        </Button>
                        <Button variant="primary" size="sm" onClick={() => onPick(video.url)}>
                          Download
                        </Button>
                      </>
                    ) : (
                      <span className="w-[104px] shrink-0 text-right text-xs text-danger">
                        Not downloadable
                      </span>
                    )}
                  </li>
                ))}
              </ul>

              <div>
                <label htmlFor="video-grabber-queue" className="mb-1 block text-sm font-semibold">
                  Queue <span className="font-normal text-text-muted">(for “Later”)</span>
                </label>
                <select
                  id="video-grabber-queue"
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
              </div>

              <div className="flex justify-end gap-2">
                <Button variant="ghost" onClick={() => setScanned(false)}>
                  Back
                </Button>
                <Button variant="primary" onClick={onClose}>
                  Done
                </Button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
