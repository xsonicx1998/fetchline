// Speed-graph sampler unit tests (S03, Milestone 5).
// Run with `npm run test:speed-graph` (`node --test` on this file;
// Node 24 runs the imported .ts source directly via type stripping).
// Pure-function tests only: no React, no Tauri, no timers.

import test from 'node:test'
import assert from 'node:assert/strict'

import {
  SPEED_GRAPH_SAMPLE_INTERVAL_MS,
  SPEED_GRAPH_WINDOW_MS,
  recordSpeedSample,
  speedGraphPoints,
  speedGraphPolyline,
} from './speedGraph.ts'

test('samples are appended at the right cadence, not per event', () => {
  let samples = []
  samples = recordSpeedSample(samples, 1000, 100)
  assert.equal(samples.length, 1)
  // 100 ms later: too soon, dropped (events arrive every 250 ms).
  samples = recordSpeedSample(samples, 1100, 200)
  assert.equal(samples.length, 1)
  assert.equal(samples[0].speedBps, 100)
  // Exactly at the 500 ms cadence: kept.
  samples = recordSpeedSample(samples, 1500, 200)
  assert.equal(samples.length, 2)
  assert.equal(samples[1].speedBps, 200)
  // The input array is never mutated.
  const before = samples
  const after = recordSpeedSample(samples, 2000, 300)
  assert.equal(before.length, 2)
  assert.equal(after.length, 3)
})

test('old samples are pruned past the 60 s window', () => {
  let samples = []
  samples = recordSpeedSample(samples, 0, 100)
  samples = recordSpeedSample(samples, 500, 100)
  samples = recordSpeedSample(samples, 1000, 100)
  assert.equal(samples.length, 3)
  // 61 s after the first sample: the t=0 and t=500 samples fall off
  // (older than 60 s); the t=1000 sample is exactly at the edge and stays.
  samples = recordSpeedSample(samples, 61_000, 50)
  assert.deepEqual(
    samples.map((s) => s.t),
    [1000, 61_000],
  )
  assert.ok(samples.every((s) => 61_000 - s.t <= SPEED_GRAPH_WINDOW_MS))
  // A sample exactly at the window edge is kept.
  let edge = recordSpeedSample([], 0, 10)
  edge = recordSpeedSample(edge, SPEED_GRAPH_WINDOW_MS, 20)
  assert.equal(edge.length, 2)
})

test('the window bounds the list: long runs never grow it past 121', () => {
  let samples = []
  for (let i = 0; i < 500; i++) {
    samples = recordSpeedSample(samples, i * SPEED_GRAPH_SAMPLE_INTERVAL_MS, i)
  }
  // 500 samples over 250 s, but only the last 60 s survive: 121 at most.
  assert.equal(samples.length, 121)
  assert.equal(samples[0].speedBps, 500 - 121)
  assert.equal(samples[samples.length - 1].speedBps, 499)
})

test('a negative speed is stored as zero, never graphed below the axis', () => {
  const samples = recordSpeedSample([], 1000, -5)
  assert.equal(samples[0].speedBps, 0)
  const points = speedGraphPoints(samples, 200, 40)
  assert.equal(points[0].y, 40)
})

test('graph points scale to the fastest sample, newest at the right edge', () => {
  const samples = [
    { t: 0, speedBps: 0 },
    { t: 30_000, speedBps: 50 },
    { t: 60_000, speedBps: 100 },
  ]
  const points = speedGraphPoints(samples, 120, 40)
  assert.equal(points.length, 3)
  // Newest at the right edge, oldest at the left edge.
  assert.equal(points[2].x, 120)
  assert.equal(points[0].x, 0)
  assert.equal(points[1].x, 60)
  // Fastest at the top, stalled at the bottom, half speed halfway.
  assert.equal(points[2].y, 0)
  assert.equal(points[0].y, 40)
  assert.equal(points[1].y, 20)
})

test('graph points stay inside the box when the window is older than the samples', () => {
  const samples = [
    { t: 0, speedBps: 10 },
    { t: 10_000, speedBps: 20 },
  ]
  const points = speedGraphPoints(samples, 120, 40)
  assert.ok(points.every((p) => p.x >= 0 && p.x <= 120))
  assert.ok(points.every((p) => p.y >= 0 && p.y <= 40))
})

test('empty samples give no points; one sample gives a dot, not a line', () => {
  assert.deepEqual(speedGraphPoints([], 120, 40), [])
  const one = speedGraphPoints([{ t: 5000, speedBps: 80 }], 120, 40)
  assert.equal(one.length, 1)
  assert.equal(one[0].x, 120)
  assert.equal(one[0].y, 0)
  assert.equal(speedGraphPolyline(one), '120,0')
})

test('polyline string joins points in order', () => {
  assert.equal(
    speedGraphPolyline([
      { x: 0, y: 40 },
      { x: 60, y: 20 },
      { x: 120, y: 0 },
    ]),
    '0,40 60,20 120,0',
  )
})
