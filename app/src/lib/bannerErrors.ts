// Round 75 — banner dismissal + failed-capture retry state as pure
// logic, unit-tested without rendering App.
//
// A banner error carries its SOURCE, and dismissals are keyed per
// (source, message):
//  * a poll error is forgotten by the next successful poll OF THAT
//    SOURCE, so a genuinely new failure with the same text shows
//    again — but a successful poll of one source never clears
//    another source's dismissal;
//  * a non-poll error (e.g. a capture auto-open failure) is never
//    un-dismissed by an unrelated successful poll.
// A failed capture auto-open is remembered separately: it is not
// retried until a NEW pending id arrives or the user clicks Retry.
// (The old code forgot the id on failure, so the next poll
// re-queued it and the red banner looped every second.)

export interface BannerError {
  source: string
  message: string
}

export class BannerDismissals {
  private dismissed = new Map<string, string>()

  /** Should this (source, message) be shown, or did the user
   *  dismiss exactly this error from this source? */
  shouldShow(source: string, message: string): boolean {
    return this.dismissed.get(source) !== message
  }

  /** Remember that the user dismissed this (source, message). */
  dismiss(source: string, message: string): void {
    this.dismissed.set(source, message)
  }

  /** A successful poll of `source` forgets that source's
   *  dismissal, so a later failure with the same text is a new
   *  failure and shows again. Other sources are untouched. */
  clearForSource(source: string): void {
    this.dismissed.delete(source)
  }
}

export class FailedCaptures {
  private failed = new Set<string>()

  /** This capture's auto-open failed: do not retry it. */
  markFailed(id: string): void {
    this.failed.add(id)
  }

  /** The user clicked Retry (or a NEW pending id arrived for the
   *  same link): the capture may be offered again. */
  clear(id: string): void {
    this.failed.delete(id)
  }

  has(id: string): boolean {
    return this.failed.has(id)
  }
}
