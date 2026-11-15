/** Speed graph sampling for the S03 progress dialog (Milestone 5).
 *
 * Pure functions only — no React, no Tauri, no timers — so the
 * sampling rules are unit-testable (see `speedGraph.test.mjs`,
 * run with `npm run test:speed-graph`). The dialog feeds one
 * sample per live progress event into `recordSpeedSample` and
 * renders `speedGraphPoints` as an SVG polyline.
 *
 * Rules, from the recon's 60 s chart: samples are kept for 60 s,
 * recorded at most once every 500 ms (progress events arrive
 * every 250 ms, so recording every one would double the points
 * for nothing), and the graph always scales to the fastest sample
 * seen so far. Nothing is invented: a stalled download graphs
 * at zero, and no sample is ever synthesised between events. */

export interface SpeedSample {
  /** `Date.now()` when the progress event arrived. */
  t: number
  /** Bytes per second reported by that event. */
  speedBps: number
}

/** The recon's chart window: the last 60 seconds of speed. */
export const SPEED_GRAPH_WINDOW_MS = 60_000
/** Fastest sampling cadence: one sample per 500 ms at most. */
export const SPEED_GRAPH_SAMPLE_INTERVAL_MS = 500

export interface GraphPoint {
  x: number
  y: number
}

/** Record one speed sample. Returns a new array (the input is
 *  never mutated):
 *  - a sample sooner than 500 ms after the previous one is dropped
 *    (progress events arrive every 250 ms);
 *  - samples older than the 60 s window fall off the front, which
 *    also bounds the list: at 500 ms cadence it holds at most 121
 *    samples, so no separate cap is needed.
 *  A negative speed (never reported by the engine) is stored as 0. */
export function recordSpeedSample(
  samples: readonly SpeedSample[],
  nowMs: number,
  speedBps: number,
): SpeedSample[] {
  const last = samples[samples.length - 1]
  if (last !== undefined && nowMs - last.t < SPEED_GRAPH_SAMPLE_INTERVAL_MS) {
    return [...samples]
  }
  const pruned = samples.filter((s) => nowMs - s.t <= SPEED_GRAPH_WINDOW_MS)
  return [...pruned, { t: nowMs, speedBps: Math.max(0, speedBps) }]
}

/** Turn samples into SVG points in a `width` × `height` box.
 *  The newest sample sits at the right edge; older samples sit
 *  further left by their age in the 60 s window. Height scales to
 *  the fastest sample seen (at least 1 B/s, so a stalled download
 *  graphs flat at the bottom instead of dividing by zero).
 *  Returns [] for no samples, and one point for a single sample
 *  (the dialog renders that as a dot, not a line). */
export function speedGraphPoints(
  samples: readonly SpeedSample[],
  width: number,
  height: number,
): GraphPoint[] {
  if (samples.length === 0) return []
  const newest = samples[samples.length - 1].t
  const maxSpeed = Math.max(1, ...samples.map((s) => s.speedBps))
  return samples.map((s) => ({
    x: Math.max(
      0,
      Math.round(((width - ((newest - s.t) / SPEED_GRAPH_WINDOW_MS) * width) * 10) / 10),
    ),
    y: Math.round(((height - (s.speedBps / maxSpeed) * height) * 10) / 10),
  }))
}

/** Points as an SVG `polyline` attribute: "x,y x,y …". */
export function speedGraphPolyline(points: readonly GraphPoint[]): string {
  return points.map((p) => `${p.x},${p.y}`).join(' ')
}
