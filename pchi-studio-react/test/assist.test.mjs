// Tests for the AI panel's logic (no network). Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { assistPrompt, redactRig, readProposal, canApply, describeChanges, ASSIST_SYSTEM } = sdk

const arm = { id: 'arm1', type: 'ur', label: 'Arm', position: { x: 0, y: 0 }, params: { host: '10.1.2.3', port: 29999, password: 'hunter2', programs: 'OPEN_ARM=3, PARK' } }
const lasers = { id: 'l1', type: 'laser', label: 'Beyond', position: { x: 0, y: 0 }, params: { host: '10.9.9.9' } }
const note = { id: 's1', type: 'scene', label: 'a scene', position: { x: 0, y: 0 }, params: {} }
const nodes = [arm, lasers, note]
const cue = (id, at, program, num) => ({ id, at_ms: at, num, action: { kind: 'motion.run', payload: { program } } })
const chart = (cues = [cue('c1', 1000, 'PARK', 1)]) => ({
  initial: 'A',
  cue_seq: 5,
  layout: { A: { x: 1, y: 2 } },
  states: [{ id: 'A', timeline: { duration_ms: 10000, lanes: [{ id: 'm', name: 'Motion', dept: 'motion', device: 'arm1', cues }] } }],
  transitions: [],
})
const reply = (c, summary = 'did it') => JSON.stringify({ summary, showchart: c })

test('the model is never shown an address, a port or a password', () => {
  const text = assistPrompt(chart(), nodes, 'add a park cue')
  for (const secret of ['10.1.2.3', '10.9.9.9', '29999', 'hunter2', '"host"', '"password"']) assert.ok(!text.includes(secret), `${secret} leaked into the prompt`)
  assert.ok(text.includes('OPEN_ARM') && text.includes('"programs"'), 'the allowed program names are shown')
  assert.ok(text.includes('add a park cue'))
  assert.ok(!text.includes('"layout"'), 'the canvas layout is not sent')
})

test('only devices with a department are listed, with their program names and no numbers', () => {
  const r = redactRig(nodes)
  assert.deepEqual(r.map((x) => x.id), ['arm1', 'l1'])
  assert.deepEqual(r[0].programs, ['OPEN_ARM', 'PARK'])
  assert.equal(r[1].programs, undefined)
})

test('the instructions say a model cannot arm, send or change the rig', () => {
  assert.match(ASSIST_SYSTEM, /cannot change the rig, arm anything, or send anything/)
})

test('a good proposal comes with what it changes, and can be applied', () => {
  const next = chart([cue('c1', 1000, 'PARK', 1), cue('c2', 3000, 'OPEN_ARM', 2)])
  const r = readProposal(reply(next, 'added OPEN_ARM at 3 s'), chart(), nodes)
  assert.ok(r.ok, r.error)
  assert.equal(r.proposal.summary, 'added OPEN_ARM at 3 s')
  assert.deepEqual(r.proposal.changes, ['Adds cue A / Motion at 3.00 s: motion.run {"program":"OPEN_ARM"}'])
  assert.deepEqual(r.proposal.added, [])
  assert.ok(canApply(r.proposal))
})

test('a program that is not on the robot list makes the proposal impossible to apply', () => {
  const r = readProposal(reply(chart([cue('c1', 1000, 'PARK', 1), cue('c2', 3000, 'SPIN', 2)])), chart(), nodes)
  assert.ok(r.ok)
  assert.equal(canApply(r.proposal), false)
  assert.match(r.proposal.added[0].text, /SPIN is not on Arm's programs list/)
})

test('a device the model invented makes it impossible to apply', () => {
  const c = chart()
  c.states[0].timeline.lanes[0].device = 'arm2'
  const r = readProposal(reply(c), chart(), nodes)
  assert.ok(r.ok)
  assert.equal(canApply(r.proposal), false)
})

test('a problem the show already had does not block a proposal, and a fix is reported', () => {
  const broken = chart([cue('c1', 1000, 'SPIN', 1)])
  const same = readProposal(reply(chart([cue('c1', 1000, 'SPIN', 1), cue('c2', 2000, 'PARK', 2)])), broken, nodes)
  assert.ok(same.ok && canApply(same.proposal), 'the old SPIN error is not the proposal\'s fault')
  const fixed = readProposal(reply(chart([cue('c1', 1000, 'PARK', 1)])), broken, nodes)
  assert.ok(fixed.ok && canApply(fixed.proposal))
  assert.equal(fixed.proposal.fixed.length, 1)
})

test('the canvas layout and the cue counter are kept from the current show', () => {
  const next = chart()
  delete next.layout
  next.cue_seq = 1
  const r = readProposal(reply(next), chart(), nodes)
  assert.ok(r.ok)
  assert.deepEqual(r.proposal.chart.layout, { A: { x: 1, y: 2 } })
  assert.equal(r.proposal.chart.cue_seq, 5, 'a number handed out before is never reused')
})

test('a reply in a code fence, with words round it, still reads', () => {
  const r = readProposal('Sure!\n```json\n' + reply(chart()) + '\n```\nDone.', chart(), nodes)
  assert.ok(r.ok, r.error)
})

test('replies that are not shows are refused with a reason, never half-applied', () => {
  for (const [bad, why] of [
    ['I cannot do that.', /no JSON/],
    ['{"summary": "x", "showchart": {"states": [', /cut off/],
    [JSON.stringify({ summary: 'x' }), /no complete showchart/],
    [JSON.stringify({ summary: 'x', showchart: { initial: 'A', states: [{}], transitions: [] } }), /not shaped like a show/],
    [JSON.stringify({ summary: 'x', showchart: { initial: 'A', states: [{ id: 'A', timeline: { lanes: [{ id: 'l' }] } }], transitions: [] } }), /not shaped like a show/],
    [JSON.stringify({ summary: 'x', showchart: { initial: 'A', states: [{ id: 'A', timeline: { lanes: [{ id: 'l', cues: [{ id: 'c' }] }] } }], transitions: [] } }), /not shaped like a show/],
  ]) {
    const r = readProposal(bad, chart(), nodes)
    assert.equal(r.ok, false, bad)
    assert.match(r.error, why)
  }
})

test('a reply that changes nothing says so', () => {
  const r = readProposal(reply(chart(), 'I cannot arm anything.'), chart(), nodes)
  assert.ok(r.ok)
  assert.deepEqual(r.proposal.changes, [])
})

test('removed scenes, cues and moved cues are described', () => {
  const a = chart([cue('c1', 1000, 'PARK', 1), cue('c2', 2000, 'PARK', 2)])
  const b = chart([cue('c1', 4000, 'PARK', 1)])
  b.states.push({ id: 'B' })
  const d = describeChanges(a, b)
  assert.ok(d.includes('Adds scene B'))
  assert.ok(d.some((x) => x.startsWith('Changes cue A / Motion at 1.00 s')))
  assert.ok(d.some((x) => x.startsWith('Removes cue A / Motion at 2.00 s')))
})
