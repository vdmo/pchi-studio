// Tests for the rig <-> showchart link. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, newShowchart, captureCue, rigDept, devicesFor, laneDevice, rigCheck, setLaneDevice, clearDevice, specOf } = sdk

const node = (id, type, extra = {}) => ({ id, type, label: id, position: { x: 0, y: 0 }, params: {}, ...extra })
let n = 0
const withCue = (c, dept) => captureCue(c, 'INTRO', dept, 1000, () => `c${++n}`).chart
const show = () => runCommandText('new show; chain preshow, intro', newShowchart()).chart

test('departments are inferred from the device type, and can be overridden', () => {
  assert.equal(rigDept(node('a', 'laser')), 'laser')
  assert.equal(rigDept(node('b', 'notch')), 'visual')
  assert.equal(rigDept(node('c', 'audio-console')), 'audio')
  assert.equal(rigDept(node('d', 'cue')), undefined)
  assert.equal(rigDept(node('e', 'osc', { dept: 'lighting' })), 'lighting')
  assert.equal(specOf('laser').label, 'Laser')
})

test('a lane uses its chosen device, else the only one for its department', () => {
  const rig = [node('l1', 'laser'), node('d1', 'lighting-desk'), node('d2', 'lighting-desk')]
  assert.equal(laneDevice(rig, { dept: 'laser' }).id, 'l1')
  assert.equal(laneDevice(rig, { dept: 'lighting' }), undefined) // two candidates: nothing assumed
  assert.equal(laneDevice(rig, { dept: 'lighting', device: 'd2' }).id, 'd2')
  assert.equal(laneDevice(rig, { dept: 'lighting', device: 'gone' }), undefined)
  assert.equal(devicesFor(rig, 'lighting').length, 2)
})

test('rigCheck reports missing, ambiguous, removed and unused devices', () => {
  let c = withCue(withCue(show(), 'laser'), 'lighting')
  // nothing in the rig
  let kinds = rigCheck(c, []).map((i) => i.kind).sort()
  assert.deepEqual(kinds, ['no-device', 'no-device'])
  // matching rig: clean, apart from an unrelated unused device
  const rig = [node('l1', 'laser'), node('d1', 'lighting-desk'), node('a1', 'audio-console')]
  const issues = rigCheck(c, rig)
  assert.deepEqual(issues.map((i) => i.kind), ['unused-device'])
  assert.equal(issues[0].deviceId, 'a1')
  // two lighting desks: ambiguous until one is picked
  const two = [...rig, node('d2', 'lighting-desk')]
  assert.ok(rigCheck(c, two).some((i) => i.kind === 'ambiguous'))
  c = setLaneDevice(c, 'INTRO', 'lighting', 'd2')
  assert.ok(!rigCheck(c, two).some((i) => i.kind === 'ambiguous'))
  // delete the chosen device from the rig: the lane is flagged, then cleared by clearDevice
  assert.ok(rigCheck(c, rig).some((i) => i.kind === 'missing-device'))
  c = clearDevice(c, 'd2')
  assert.equal(c.states.find((s) => s.id === 'INTRO').timeline.lanes.find((l) => l.id === 'lighting').device, undefined)
})

test('empty lanes and lanes without a department are not flagged', () => {
  const c = show()
  assert.deepEqual(rigCheck(c, []), [])
  const o = withCue(c, 'other')
  assert.deepEqual(rigCheck(o, []), [])
})

test('setLaneDevice does not mutate and can clear', () => {
  const c = withCue(show(), 'laser')
  const snap = JSON.stringify(c)
  const d = setLaneDevice(c, 'INTRO', 'laser', 'x')
  assert.equal(JSON.stringify(c), snap)
  assert.equal(d.states.find((s) => s.id === 'INTRO').timeline.lanes[0].device, 'x')
  assert.equal(setLaneDevice(d, 'INTRO', 'laser', undefined).states.find((s) => s.id === 'INTRO').timeline.lanes[0].device, undefined)
})

test('requiredFor lists the departments and devices that have cues in a scene', () => {
  const { requiredFor } = sdk
  let c = withCue(withCue(show(), 'laser'), 'lighting')
  // no rig: the departments themselves
  assert.deepEqual([...requiredFor(c, 'INTRO', [])].sort(), ['laser', 'lighting'])
  // a rig: the devices, every candidate when the lane has not chosen
  const rig = [node('l1', 'laser'), node('d1', 'lighting-desk'), node('d2', 'lighting-desk'), node('a1', 'audio-console')]
  assert.deepEqual([...requiredFor(c, 'INTRO', rig)].sort(), ['d1', 'd2', 'l1'])
  c = setLaneDevice(c, 'INTRO', 'lighting', 'd2')
  assert.deepEqual([...requiredFor(c, 'INTRO', rig)].sort(), ['d2', 'l1'])
  // a scene with no cues, an unknown scene, an empty lane or an "other" lane needs nobody
  assert.equal(requiredFor(c, 'PRESHOW', rig).size, 0)
  assert.equal(requiredFor(c, 'NOPE', rig).size, 0)
  assert.equal(requiredFor(c, undefined, rig).size, 0)
  assert.equal(requiredFor(withCue(show(), 'other'), 'INTRO', rig).size, 0)
})

test('the tool profiles serve the departments they are for', () => {
  assert.equal(rigDept(node('r', 'resolume')), 'visual')
  assert.equal(rigDept(node('m', 'grandma3')), 'lighting')
  assert.equal(rigDept(node('e', 'eos')), 'lighting')
  assert.equal(rigDept(node('x', 'x32')), 'audio')
  assert.equal(rigDept(node('b', 'beyond')), 'laser')
  assert.equal(specOf('beyond').label, 'Pangolin BEYOND')
  assert.equal(specOf('resolume').defaults.port, 7000)
})
