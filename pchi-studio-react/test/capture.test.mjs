// Tests for rehearsal capture and quantize. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, newShowchart, captureCue, quantizeCues, beatMs, ensureCueNumbers } = sdk

const base = () => runCommandText('new show; chain preshow, intro, drop', newShowchart()).chart
let n = 0
const id = () => `c${++n}`
const lane = (c, scene, l) => c.states.find((s) => s.id === scene).timeline.lanes.find((x) => x.id === l)

test('capture creates the lane if the department has none, then reuses it', () => {
  const a = captureCue(base(), 'INTRO', 'lighting', 12340, id)
  assert.equal(a.laneAdded, true)
  assert.equal(lane(a.chart, 'INTRO', 'lighting').name, 'Lighting')
  const b = captureCue(a.chart, 'INTRO', 'lighting', 15000, id)
  assert.equal(b.laneAdded, false)
  assert.equal(lane(b.chart, 'INTRO', 'lighting').cues.length, 2)
})

test('captured cues are blank department actions at a 50 ms time, and do not mutate the input', () => {
  const c = base()
  const snap = JSON.stringify(c)
  const r = captureCue(c, 'DROP', 'laser', 4321, id)
  assert.equal(JSON.stringify(c), snap)
  const cue = lane(r.chart, 'DROP', 'laser').cues[0]
  assert.deepEqual([cue.at_ms, cue.action.kind, cue.action.payload.cue], [4300, 'laser.cue', ''])
  assert.equal(captureCue(c, 'NOPE', 'laser', 1, id), null)
})

test('capturing is a first-class cue: it gets a stable number like any other', () => {
  let c = captureCue(base(), 'INTRO', 'audio', 1000, id).chart
  c = ensureCueNumbers(c)
  c = captureCue(c, 'INTRO', 'visual', 500, id).chart
  c = ensureCueNumbers(c)
  assert.equal(lane(c, 'INTRO', 'audio').cues[0].num, 1)
  assert.equal(lane(c, 'INTRO', 'visual').cues[0].num, 2)
})

test('quantize moves cues to the nearest grid line and says how many moved', () => {
  let c = base()
  for (const t of [1010, 1260, 2490]) c = captureCue(c, 'INTRO', 'lighting', t, id).chart // kept at 1000, 1250, 2500
  const q = quantizeCues(c, 'INTRO', 1000)
  assert.deepEqual(lane(q.chart, 'INTRO', 'lighting').cues.map((x) => x.at_ms), [1000, 1000, 3000])
  assert.equal(q.moved, 2)
  const again = quantizeCues(q.chart, 'INTRO', 1000)
  assert.equal(again.moved, 0)
  assert.equal(again.chart, q.chart, 'nothing to do returns the same chart')
})

test('quantize can be limited to some cues, and to a beat grid', () => {
  const a = captureCue(base(), 'INTRO', 'lighting', 1030, () => 'x') // kept at 1050
  const b = captureCue(a.chart, 'INTRO', 'lighting', 2470, () => 'y') // kept at 2450
  const q = quantizeCues(b.chart, 'INTRO', 1000, new Set(['y']))
  assert.deepEqual(lane(q.chart, 'INTRO', 'lighting').cues.map((x) => x.at_ms), [1050, 2000])
  assert.equal(q.moved, 1)
  assert.equal(beatMs(120), 500)
  assert.equal(beatMs(120, 2), 250)
  assert.equal(beatMs(0), 3000, 'absurd tempos are clamped')
})
