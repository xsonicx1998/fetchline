import addUrlSvg from '../assets/icons/add-url.svg?raw'
import categoryFolderSvg from '../assets/icons/category-folder.svg?raw'
import deleteCompletedSvg from '../assets/icons/delete-completed.svg?raw'
import deleteSvg from '../assets/icons/delete.svg?raw'
import downloadSvg from '../assets/icons/download.svg?raw'
import finishedSvg from '../assets/icons/finished.svg?raw'
import grabberSvg from '../assets/icons/grabber.svg?raw'
import lightningSvg from '../assets/icons/lightning.svg?raw'
import linkSvg from '../assets/icons/link.svg?raw'
import optionsSvg from '../assets/icons/options.svg?raw'
import queueSvg from '../assets/icons/queue.svg?raw'
import resumeSvg from '../assets/icons/resume.svg?raw'
import schedulerSvg from '../assets/icons/scheduler.svg?raw'
import startQueueSvg from '../assets/icons/start-queue.svg?raw'
import stopAllSvg from '../assets/icons/stop-all.svg?raw'
import stopQueueSvg from '../assets/icons/stop-queue.svg?raw'
import stopSvg from '../assets/icons/stop.svg?raw'
import unfinishedSvg from '../assets/icons/unfinished.svg?raw'

/** The user's asset pack (Round 45): 18 SVG line icons, 24×24,
 *  stroke 2.0 (was 1.8 — thickened Round 48 so the lines read
 *  clearly at 32 px instead of looking faded), `currentColor` — so
 *  the wrapping element's text colour IS the icon colour. They are
 *  inlined (not <img>): as an image, `currentColor` would fall
 *  back to black and the pack's per-action colours (cyan resume,
 *  pink stop…) would be lost.
 *  The SVGs carry width/height 256 attributes; the `.pack-icon`
 *  rule in index.css sizes the inner <svg> to its wrapper. The
 *  strings are static local assets, so innerHTML is safe here. */
const ICONS = {
  'add-url': addUrlSvg,
  'category-folder': categoryFolderSvg,
  'delete-completed': deleteCompletedSvg,
  delete: deleteSvg,
  download: downloadSvg,
  finished: finishedSvg,
  grabber: grabberSvg,
  lightning: lightningSvg,
  link: linkSvg,
  options: optionsSvg,
  queue: queueSvg,
  resume: resumeSvg,
  scheduler: schedulerSvg,
  'start-queue': startQueueSvg,
  'stop-all': stopAllSvg,
  'stop-queue': stopQueueSvg,
  stop: stopSvg,
  unfinished: unfinishedSvg,
} as const

export type PackIconName = keyof typeof ICONS

export function PackIcon({ name, className }: { name: PackIconName; className?: string }) {
  return (
    <span
      className={`pack-icon inline-flex shrink-0 ${className ?? ''}`}
      dangerouslySetInnerHTML={{ __html: ICONS[name] }}
      aria-hidden="true"
    />
  )
}
