// Tests for the show checks Studio runs while editing. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { validateShow, validationIndex, allowedPrograms } = sdk

const arm = (programs) => ({ id: 'arm1', type: 'motion', label: 'Arm', position: { x: 0, y: 0 }, params: programs === undefined ? {} : { programs } })
const chart = (cues, extra = {}) => ({
  initial: 'A',
  states: [{ id: 'A', timeline: { duration_ms: 10000, lanes: [{ id: 'm', name: 'Motion', dept: 'motion', device: 'arm1', cues }], ...extra } }],
  transitions: [],
})
const run = (program, at = 1000, id = 'c1', num = 1) => ({ id, num, at_ms: at, action: { kind: 'motion.run', payload: { program } } })

test('a clean show has nothing to say', () => {
  assert.deepEqual(validateShow(chart([run('OPEN_ARM')]), [arm('OPEN_ARM, PARK')]), [])
})

test('a motion cue for a program that is not on the device list is an error that names the list', () => {
  const v = validateShow(chart([run('SPIN')]), [arm('OPEN_ARM=3, PARK=5')])
  assert.equal(v.length, 1)
  assert.equal(v[0].level, 'error')
  assert.deepEqual([v[0].scene, v[0].laneId, v[0].cueId, v[0].device], ['A', 'm', 'c1', 'arm1'])
  assert.match(v[0].text, /SPIN is not on Arm's programs list \(OPEN_ARM, PARK\)/)
})

test('a robot with no programs list cannot run anything, and the message says what to do', () => {
  const v = validateShow(chart([run('OPEN_ARM')]), [arm(undefined)])
  assert.equal(v.length, 1)
  assert.match(v[0].text, /no list of allowed programs/)
  assert.match(v[0].text, /Add OPEN_ARM/)
})

test('a motion cue with no program is an error, not a refusal at run time', () => {
  const v = validateShow(chart([run('')]), [arm('PARK')])
  assert.ok(v.some((x) => x.level === 'error' && /names no program/.test(x.text)), JSON.stringify(v))
})

test('a cue after the end of its scene is a warning', () => {
  const v = validateShow(chart([run('PARK', 12000)]), [arm('PARK')])
  assert.equal(v.length, 1)
  assert.equal(v[0].level, 'warn')
  assert.match(v[0].text, /never fire/)
})

test('two cues with one number are both flagged', () => {
  const v = validateShow(chart([run('PARK', 1000, 'c1', 7), run('PARK', 2000, 'c2', 7)]), [arm('PARK')])
  assert.deepEqual(v.filter((x) => /same number/.test(x.text)).map((x) => x.cueId).sort(), ['c1', 'c2'])
})

test('a lane pointing at a device that was removed is an error with the lane named', () => {
  const v = validateShow(chart([run('PARK')]), [])
  assert.ok(v.some((x) => x.level === 'error' && x.laneId === 'm' && /removed from the rig/.test(x.text)), JSON.stringify(v))
})

test('a broken chart is reported with the scene when one is named', () => {
  const c = { initial: 'NOPE', states: [{ id: 'A' }], transitions: [] }
  const v = validateShow(c, [])
  assert.ok(v.some((x) => x.level === 'error' && /initial state/.test(x.text)))
})

test('the index counts problems by scene, cue and device', () => {
  const v = validateShow(chart([run('SPIN'), run('PARK', 2000, 'c2', 2)]), [arm('PARK')])
  const ix = validationIndex(v)
  assert.deepEqual(ix.cue.get('c1'), { error: 1, warn: 0 })
  assert.equal(ix.cue.get('c2'), undefined)
  assert.deepEqual(ix.scene.get('A'), { error: 1, warn: 0 })
  assert.deepEqual(ix.device.get('arm1'), { error: 1, warn: 0 })
})

test('allowed programs reads a string or a list and drops the numbers', () => {
  assert.deepEqual(allowedPrograms({ params: { programs: 'A=1, B' } }), ['A', 'B'])
  assert.deepEqual(allowedPrograms({ params: { programs: ['A=1', 'B'] } }), ['A', 'B'])
  assert.deepEqual(allowedPrograms({ params: {} }), [])
})
