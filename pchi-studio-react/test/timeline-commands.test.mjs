// Tests for timeline commands (lanes and cues from typed/spoken text). Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, applyCommand, newShowchart } = sdk

const base = () => {
  const r = runCommandText('new show; chain preshow, intro, drop, finale', newShowchart())
  assert.ok(r.ok, r.error)
  return r.chart
}
const ok = (text, chart = base(), opts) => {
  const r = runCommandText(text, chart, opts)
  assert.ok(r.ok, `expected "${text}" to work, got: ${r.error}`)
  return r
}
const bad = (text, chart = base(), opts) => {
  const r = runCommandText(text, chart, opts)
  assert.ok(!r.ok, `expected "${text}" to fail`)
  return r
}
const tl = (chart, id) => chart.states.find((s) => s.id === id).timeline
const lane = (chart, id, laneId) => tl(chart, id).lanes.find((l) => l.id === laneId)

test('standard lanes', () => {
  const r = ok('add the standard lanes in intro')
  assert.deepEqual(tl(r.chart, 'INTRO').lanes.map((l) => [l.id, l.dept]), [['visual', 'visual'], ['lighting', 'lighting'], ['laser', 'laser'], ['audio', 'audio']])
  // idempotent
  assert.match(ok('add the standard lanes in intro', r.chart).steps[0].summary, /already has/)
})

test('add a cue: lane is created, department action filled in', () => {
  const r = ok('add a lighting cue at 4 seconds look blue in intro')
  const l = lane(r.chart, 'INTRO', 'lighting')
  assert.equal(l.dept, 'lighting')
  assert.deepEqual(l.cues[0], { id: 'lighting-4000', at_ms: 4000, action: { kind: 'lighting.look', payload: { look: 'blue' } } })
  assert.match(r.steps[0].summary, /also created lane Lighting/)
})

test('the action alone decides the lane; time first works too', () => {
  const r = ok('at 10 seconds laser cue l3 in drop')
  assert.deepEqual(lane(r.chart, 'DROP', 'laser').cues[0].action, { kind: 'laser.cue', payload: { cue: 'l3' } })
  assert.equal(lane(r.chart, 'DROP', 'laser').cues[0].at_ms, 10000)
  const v = ok('at 2 seconds scene portal in intro')
  assert.equal(lane(v.chart, 'INTRO', 'visual').cues[0].action.payload.scene, 'portal')
  const a = ok('at 3 seconds audio snapshot 12 in intro')
  assert.equal(lane(a.chart, 'INTRO', 'audio').cues[0].action.kind, 'audio.snapshot')
})

test('"a laser cue l3" style: the department supplies the kind', () => {
  const r = ok('add a laser cue l3 at 6 seconds in drop')
  assert.deepEqual(lane(r.chart, 'DROP', 'laser').cues[0].action, { kind: 'laser.cue', payload: { cue: 'l3' } })
})

test('a cue with no value is allowed but flagged; no state is a clear question', () => {
  const r = ok('add a laser cue at 4 seconds', base(), { state: 'INTRO' })
  assert.equal(lane(r.chart, 'INTRO', 'laser').cues[0].action.payload.cue, '')
  assert.match(r.steps[0].summary, /left blank/)
  assert.match(bad('add a laser cue at 4 seconds').error, /Which state/)
})

test('the state in the sentence wins over the tab\'s state', () => {
  const r = ok('add a lighting cue at 1 second look red in drop', base(), { state: 'INTRO' })
  assert.ok(lane(r.chart, 'DROP', 'lighting'))
  assert.equal(tl(r.chart, 'INTRO'), undefined)
})

test('lengths, custom lanes and time formats', () => {
  let r = ok('add a lighting cue at 4 seconds look blue for 2 seconds in intro')
  assert.equal(lane(r.chart, 'INTRO', 'lighting').cues[0].length_ms, 2000)
  r = ok('add a cue at 3 seconds lighting look red on the followspot lane in intro')
  const f = lane(r.chart, 'INTRO', 'followspot')
  assert.equal(f.name, 'followspot')
  assert.equal(f.cues[0].action.kind, 'lighting.look')
  const at = (t) => lane(ok(`at ${t} laser cue x in intro`).chart, 'INTRO', 'laser').cues[0].at_ms
  assert.equal(at('1:30'), 90000)
  assert.equal(at('four seconds'), 4000)
  assert.equal(at('4s'), 4000)
  assert.equal(at('2.5'), 2500)
  assert.equal(at('500 ms'), 500)
  assert.equal(at('a minute'), 60000)
})

test('duplicates are a no-op; same time on different lanes is fine', () => {
  const a = ok('add a lighting cue at 4 seconds look blue in intro').chart
  const again = ok('add a lighting cue at 4 seconds look blue in intro', a)
  assert.match(again.steps[0].summary, /already there/)
  assert.equal(lane(again.chart, 'INTRO', 'lighting').cues.length, 1)
  const two = ok('add a laser cue at 4 seconds l1 in intro', a).chart
  assert.equal(tl(two, 'INTRO').lanes.length, 2)
})

test('move, remove, and the errors that help', () => {
  const c = ok('add a lighting cue at 2 seconds look blue in intro').chart
  let r = ok('move the lighting cue at 2 seconds to 5 seconds in intro', c)
  assert.equal(lane(r.chart, 'INTRO', 'lighting').cues[0].at_ms, 5000)
  r = ok('remove the lighting cue at 2 seconds in intro', c)
  assert.equal(lane(r.chart, 'INTRO', 'lighting').cues.length, 0)
  const miss = bad('move the lighting cue at 9 seconds to 5 seconds in intro', c)
  assert.match(miss.error, /no cue at 9 s/)
  assert.match(miss.hint, /2 s/)
  const two = ok('add a laser cue at 2 seconds l1 in intro', c).chart
  assert.match(bad('remove the cue at 2 seconds in intro', two).error, /2 cues start at 2 s/)
  assert.equal(ok('remove the cue at 2 seconds on lighting in intro', two).chart.states[1].timeline.lanes.find((l) => l.id === 'lighting').cues.length, 0)
})

test('remove a lane, clear a timeline, set its length', () => {
  const c = ok('add the standard lanes in intro; add a lighting cue at 2 seconds look blue in intro').chart
  let r = ok('remove the laser lane in intro', c)
  assert.deepEqual(tl(r.chart, 'INTRO').lanes.map((l) => l.id), ['visual', 'lighting', 'audio'])
  r = ok('clear the intro timeline', c)
  assert.equal(tl(r.chart, 'INTRO').lanes.reduce((n, l) => n + l.cues.length, 0), 0)
  assert.equal(tl(r.chart, 'INTRO').lanes.length, 4, 'lanes stay, cues go')
  assert.equal(tl(ok('set intro length to 20 seconds', c).chart, 'INTRO').duration_ms, 20000)
  assert.equal(tl(ok('make the intro timeline 30 seconds long', c).chart, 'INTRO').duration_ms, 30000)
  assert.equal(tl(ok('set the timeline length to 12 seconds', c, { state: 'INTRO' }).chart, 'INTRO').duration_ms, 12000)
  assert.match(bad('remove the laser lane in zzzz').error, /zzzz|no laser lane|Which state/)
})

test('other command families are not hijacked', () => {
  const c = base()
  assert.equal(ok('from intro to drop after 30 seconds', c).chart.transitions.at(-1).after_ms, 30000)
  assert.equal(ok('when entering drop set lighting look blue', c).chart.states[2].enter_actions[0].kind, 'lighting.look')
  assert.equal(ok('add state cue', c).chart.states.at(-1).id, 'CUE')
  assert.equal(ok('set energy to 0.8', c).chart.context.energy, 0.8)
})

test('several timeline commands in one sentence are applied in order, atomically', () => {
  const r = ok('add the standard lanes in intro; at 2 seconds lighting look warm in intro; at 4 seconds laser cue l1 in intro')
  assert.equal(r.steps.length, 3)
  assert.equal(lane(r.chart, 'INTRO', 'laser').cues[0].at_ms, 4000)
  const c = base()
  const before = JSON.stringify(c)
  assert.equal(bad('at 2 seconds lighting look warm in intro; move the cue at 99 seconds to 1 second in intro', c).ok, false)
  assert.equal(JSON.stringify(c), before)
})

test('speech: near-miss state names and spoken numbers', () => {
  const r = ok('add a lighting cue at four seconds look blue in intero')
  assert.ok(lane(r.chart, 'INTRO', 'lighting'))
  assert.ok(r.notes.some((n) => n.includes('INTRO')))
})

test('applyCommand for timelines never mutates its input', () => {
  const c = base()
  const snap = JSON.stringify(c)
  applyCommand(c, { op: 'addCue', state: 'INTRO', dept: 'laser', at_ms: 1000, action: { kind: 'laser.cue', payload: { cue: 'x' } } })
  applyCommand(c, { op: 'addLane', state: 'INTRO', standard: true })
  assert.equal(JSON.stringify(c), snap)
})
