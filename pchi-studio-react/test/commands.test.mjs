// Tests for show commands (typed/voice text -> chart changes). Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, applyCommand, newShowchart, validateShowchart } = sdk

const empty = () => runCommandText('new show', newShowchart()).chart
const ok = (text, chart = newShowchart()) => {
  const r = runCommandText(text, chart)
  assert.ok(r.ok, `expected "${text}" to work, got: ${r.error}`)
  return r
}
const bad = (text, chart = newShowchart()) => {
  const r = runCommandText(text, chart)
  assert.ok(!r.ok, `expected "${text}" to fail`)
  return r
}
const ids = (c) => c.states.map((s) => s.id)

test('chain builds states and NEXT transitions, first state becomes the start', () => {
  const r = ok('chain preshow, intro, drop, breakdown, finale', empty())
  assert.deepEqual(ids(r.chart), ['PRESHOW', 'INTRO', 'DROP', 'BREAKDOWN', 'FINALE'])
  assert.equal(r.chart.initial, 'PRESHOW')
  assert.equal(r.chart.transitions.length, 4)
  assert.deepEqual(r.chart.transitions[0], { source: 'PRESHOW', target: 'INTRO', event: 'NEXT' })
  assert.deepEqual(validateShowchart(r.chart), [])
})

test('chain accepts spoken style (no punctuation, "then")', () => {
  const r = ok('chain preshow then intro then drop then finale', empty())
  assert.deepEqual(ids(r.chart), ['PRESHOW', 'INTRO', 'DROP', 'FINALE'])
})

test('add state, with and without a label, and refuses duplicates', () => {
  let r = ok('add state drop', empty())
  assert.deepEqual(ids(r.chart), ['DROP'])
  r = ok('create a breakdown state labelled "The Breakdown"', r.chart)
  assert.equal(r.chart.states[1].label, 'The Breakdown')
  assert.match(bad('add state drop', r.chart).error, /already exists/)
})

test('transitions: event, delay, guard, and defaults', () => {
  const c = ok('chain intro, drop, breakdown', empty()).chart
  let r = ok('from intro to breakdown on skip', c)
  assert.deepEqual(r.chart.transitions.at(-1), { source: 'INTRO', target: 'BREAKDOWN', event: 'SKIP' })
  r = ok('from drop to breakdown after 30 seconds', c)
  assert.deepEqual(r.chart.transitions.at(-1), { source: 'DROP', target: 'BREAKDOWN', after_ms: 30000 })
  r = ok('from drop to intro after thirty seconds', c)
  assert.equal(r.chart.transitions.at(-1).after_ms, 30000)
  r = ok('from drop to intro after a minute', c)
  assert.equal(r.chart.transitions.at(-1).after_ms, 60000)
  r = ok('from drop to intro on go if energy is above 0.7', c)
  assert.deepEqual(r.chart.transitions.at(-1), { source: 'DROP', target: 'INTRO', event: 'GO', guard: 'energy > 0.7' })
  r = ok('connect breakdown to intro', c)
  assert.equal(r.chart.transitions.at(-1).event, 'NEXT')
})

test('alternative transition phrasings', () => {
  const c = ok('chain intro, drop', empty()).chart
  assert.deepEqual(ok('on skip in drop go to intro', c).chart.transitions.at(-1), { source: 'DROP', target: 'INTRO', event: 'SKIP' })
  assert.deepEqual(ok('drop goes to intro on back', c).chart.transitions.at(-1), { source: 'DROP', target: 'INTRO', event: 'BACK' })
  assert.deepEqual(ok('drop -> intro on loop', c).chart.transitions.at(-1), { source: 'DROP', target: 'INTRO', event: 'LOOP' })
})

test('a transition that could never fire is refused, with advice', () => {
  const c = ok('chain intro, drop', empty()).chart
  const r = bad('from intro to intro', c) // same source+event NEXT as intro->drop
  assert.match(r.error, /already goes to DROP/)
  assert.ok(r.hint)
})

test('a transition to an unknown state creates it', () => {
  const c = ok('add state intro', empty()).chart
  const r = ok('from intro to finale on end', c)
  assert.deepEqual(ids(r.chart), ['INTRO', 'FINALE'])
  assert.match(r.steps[0].summary, /also created FINALE/)
})

test('enter and exit actions, department aware (PIP capability names)', () => {
  const c = ok('chain intro, drop', empty()).chart
  const r = ok('when entering drop set lighting look blue, laser cue l3 and audio snapshot 12', c)
  const drop = r.chart.states.find((s) => s.id === 'DROP')
  assert.deepEqual(drop.enter_actions, [
    { kind: 'lighting.look', payload: { look: 'blue' } },
    { kind: 'laser.cue', payload: { cue: 'l3' } },
    { kind: 'audio.snapshot', payload: { snapshot: '12' } },
  ])
  const r2 = ok('when leaving drop activate scene fade_out', r.chart)
  assert.deepEqual(r2.chart.states.find((s) => s.id === 'DROP').exit_actions, [{ kind: 'scene.activate', payload: { scene: 'fade_out' } }])
  const r3 = ok('on enter intro activate scene intro, audio scene intro stem, playlist main, output ghost main', c)
  const kinds = r3.chart.states.find((s) => s.id === 'INTRO').enter_actions.map((a) => a.kind)
  assert.deepEqual(kinds, ['scene.activate', 'audio.scene', 'playlist.play', 'output.activate'])
  assert.equal(r3.chart.states[0].enter_actions[3].payload.target, 'ghost_main')
})

test('actions: shorthand forms and custom events', () => {
  const c = ok('chain intro, drop', empty()).chart
  assert.equal(ok('in drop set lighting look red', c).chart.states[1].enter_actions[0].kind, 'lighting.look')
  assert.equal(ok('drop: laser cue l1', c).chart.states[1].enter_actions[0].kind, 'laser.cue')
  const r = ok('when entering drop send event strobe.flash rate=4', c)
  assert.deepEqual(r.chart.states[1].enter_actions[0], { kind: 'strobe.flash', payload: { rate: 4 } })
  assert.match(bad('when entering drop wiggle the lights', c).error, /action/)
})

test('rename, label, initial, remove, clear actions, context', () => {
  const c = ok('chain intro, drop, finale', empty()).chart
  let r = ok('rename drop to peak', c)
  assert.deepEqual(ids(r.chart), ['INTRO', 'PEAK', 'FINALE'])
  assert.ok(r.chart.transitions.every((t) => t.source !== 'DROP' && t.target !== 'DROP'))
  r = ok('start at finale', c)
  assert.equal(r.chart.initial, 'FINALE')
  r = ok('make drop the initial state', c)
  assert.equal(r.chart.initial, 'DROP')
  r = ok('label intro as Welcome Home', c)
  assert.equal(r.chart.states[0].label, 'Welcome Home')
  r = ok('remove state drop', c)
  assert.deepEqual(ids(r.chart), ['INTRO', 'FINALE'])
  assert.equal(r.chart.transitions.length, 0)
  r = ok('set energy to 0.8', c)
  assert.equal(r.chart.context.energy, 0.8)
  const withAct = ok('when entering drop set lighting look blue', c).chart
  r = ok('clear actions of drop', withAct)
  assert.deepEqual(r.chart.states[1].enter_actions, [])
  r = ok('remove transition from intro to drop', c)
  assert.equal(r.chart.transitions.length, 1)
})

test('removing the initial state moves the start to the next state', () => {
  const c = ok('chain intro, drop', empty()).chart
  assert.equal(ok('remove state intro', c).chart.initial, 'DROP')
})

test('several commands in one sentence, applied in order', () => {
  const r = ok('add state intro; add state drop and then from intro to drop on go. when entering drop set lighting look blue', empty())
  assert.deepEqual(ids(r.chart), ['INTRO', 'DROP'])
  assert.equal(r.chart.transitions[0].event, 'GO')
  assert.equal(r.chart.states[1].enter_actions.length, 1)
  assert.equal(r.steps.length, 4)
})

test('speech slips: spaces, case and near-miss state names resolve, and are reported', () => {
  const c = ok('chain pre show, intro, breakdown', empty()).chart
  assert.deepEqual(ids(c), ['PRE_SHOW', 'INTRO', 'BREAKDOWN'])
  let r = ok('start at preshow', c)
  assert.equal(r.chart.initial, 'PRE_SHOW')
  r = ok('start at breakdon', c) // one letter off
  assert.equal(r.chart.initial, 'BREAKDOWN')
  assert.ok(r.notes.some((n) => n.includes('closest match')))
})

test('unknown states are errors that list the real ones', () => {
  const c = ok('chain intro, drop', empty()).chart
  const r = bad('remove state xylophone', c)
  assert.match(r.error, /xylophone/)
  assert.match(r.hint, /INTRO, DROP/)
  assert.match(bad('when entering zzzz set lighting look blue', c).error, /zzzz/)
})

test('all-or-nothing: if one part fails, the chart is untouched', () => {
  const c = ok('chain intro, drop', empty()).chart
  const before = JSON.stringify(c)
  const r = bad('add state finale; remove state nonexistent', c)
  assert.equal(r.ok, false)
  assert.equal(JSON.stringify(c), before)
})

test('nonsense gets a helpful message, not a crash', () => {
  const r = bad('make it pop')
  assert.match(r.error, /didn't understand/)
  assert.ok(r.hint.length > 10)
  assert.match(bad('   ').error, /Type or say/)
  assert.equal(bad('from intro to drop if energy >', ok('chain intro, drop', empty()).chart).ok, false)
})

test('undo, redo and help are recognised (the UI performs them)', () => {
  assert.equal(ok('undo').steps[0].command.op, 'undo')
  assert.equal(ok('redo').steps[0].command.op, 'redo')
  assert.equal(ok('help').steps[0].command.op, 'help')
})

test('applyCommand never mutates its input', () => {
  const c = newShowchart()
  const snap = JSON.stringify(c)
  applyCommand(c, { op: 'addState', id: 'X' })
  applyCommand(c, { op: 'removeState', id: 'INTRO' })
  applyCommand(c, { op: 'renameState', from: 'INTRO', to: 'Z' })
  applyCommand(c, { op: 'addAction', state: 'INTRO', when: 'enter', action: { kind: 'a.b', payload: {} } })
  assert.equal(JSON.stringify(c), snap)
})

test('the default demo show parses its own example commands', () => {
  for (const ex of sdk.COMMAND_EXAMPLES) {
    const base = ok('chain preshow, intro, drop, breakdown, finale', empty()).chart
    const r = runCommandText(ex, base)
    // examples that create something already there (e.g. "add state drop") may be refused: that is fine,
    // but none may be "not understood"
    if (!r.ok) assert.doesNotMatch(r.error, /didn't understand/, ex)
  }
})
