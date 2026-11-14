// Tests for reading a recorded run. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { replayAt, notable, clock, position } = sdk

const view = {
  started_ts: 1000, ended_ts: 91000, length_ms: 90000, initial_context: { energy: 0.2 },
  scenes: [
    { scene: 'PRESHOW', from_ms: 0, to_ms: 10000, via: 'start' },
    { scene: 'INTRO', from_ms: 10000, to_ms: 60000, via: 'START' },
    { scene: 'MAIN', from_ms: 60000, to_ms: null, via: 'NEXT' },
  ],
  frames: [
    { t: 10000, kind: 'scene', scene: 'INTRO', text: 'PRESHOW → INTRO' },
    { t: 10000, kind: 'cue', scene: 'INTRO', text: 'wash up' },
    { t: 12000, kind: 'context', scene: 'INTRO', text: 'energy = 0.6', key: 'energy', value: 0.6 },
    { t: 20000, kind: 'edit', scene: 'INTRO', text: 'edited: moved a cue' },
    { t: 30000, kind: 'delivery', scene: 'INTRO', text: 'laser: failed', bad: true },
    { t: 31000, kind: 'delivery', scene: 'INTRO', text: 'lights: sent', bad: false },
    { t: 40000, kind: 'decision', scene: 'INTRO', text: 'Alex: approved', by: 'Alex' },
    { t: 60000, kind: 'scene', scene: 'MAIN', text: 'INTRO → MAIN' },
    { t: 61000, kind: 'cue', scene: 'MAIN', text: 'strobe' },
  ],
  summary: { scenes_visited: 3, path: ['PRESHOW', 'INTRO', 'MAIN'], cues: 2, worst_cue_drift_ms: 0, deliveries_wrong: 1, edits: 1, decisions: 1, who: ['Alex'], line: '' },
}

test('the show is in the scene it was in at that moment, with the context it had', () => {
  const a = replayAt(view, 5000)
  assert.equal(a.scene, 'PRESHOW')
  assert.equal(a.inScene, 5000)
  assert.deepEqual(a.context, { energy: 0.2 })
  assert.equal(a.next.t, 10000)

  const b = replayAt(view, 15000)
  assert.equal(b.scene, 'INTRO')
  assert.equal(b.inScene, 5000)
  assert.equal(b.context.energy, 0.6)
  assert.deepEqual(b.cues.map((c) => c.text), ['wash up'])

  const c = replayAt(view, 61500)
  assert.equal(c.scene, 'MAIN')
  assert.deepEqual(c.cues.map((x) => x.text), ['strobe'], 'cues of an earlier scene are not listed')
  assert.equal(c.next, undefined)
})

test('a moment exactly on a scene change is already in the new scene', () => {
  assert.equal(replayAt(view, 10000).scene, 'INTRO')
  assert.equal(replayAt(view, 60000).scene, 'MAIN')
})

test('before the start and after the end are held to the run', () => {
  assert.equal(replayAt(view, -50).t, 0)
  assert.equal(replayAt(view, 1e9).t, 90000)
  assert.equal(replayAt(view, 1e9).scene, 'MAIN')
})

test('what is marked on the timeline is what a person should look at', () => {
  assert.deepEqual(notable(view).map((f) => f.kind), ['edit', 'delivery', 'decision'])
})

test('times read as a clock and sit between 0 and 1', () => {
  assert.equal(clock(83400), '1:23.4')
  assert.equal(clock(5000), '0:05.0')
  assert.equal(position(view, 45000), 0.5)
  assert.equal(position(view, 1e9), 1)
  assert.equal(position({ ...view, length_ms: 0 }, 5), 0)
})
