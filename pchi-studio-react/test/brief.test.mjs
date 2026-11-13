// Tests for brief -> show draft. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { briefToCommands, draftText, runCommandText, newShowchart, ensureCueNumbers } = sdk

const empty = () => runCommandText('new show', newShowchart()).chart
const build = (brief) => {
  const d = briefToCommands(brief)
  const r = runCommandText(draftText(d), empty())
  return { d, r }
}

test('one scene per line becomes a chain with timed transitions', () => {
  const { d, r } = build('Preshow, 20 minutes\nIntro, 30 seconds\nDrop\nFinale')
  assert.deepEqual(d.scenes.map((s) => s.id), ['preshow', 'intro', 'drop', 'finale'])
  assert.equal(d.commands[0], 'chain preshow, intro, drop, finale')
  assert.ok(d.commands.includes('from preshow to intro after 1200 seconds'))
  assert.ok(d.commands.includes('from intro to drop after 30 seconds'))
  assert.ok(r.ok, r.error)
  assert.deepEqual(r.chart.states.map((s) => s.id), ['PRESHOW', 'INTRO', 'DROP', 'FINALE'])
  assert.ok(r.chart.transitions.some((t) => t.source === 'INTRO' && t.target === 'DROP' && t.after_ms === 30000))
})

test('department words become cues in the scene, with the colour or following word as the value', () => {
  const { d, r } = build('Intro with blue lights and lasers sweep\nDrop: visuals tunnel, audio drop')
  const intro = d.scenes[0]
  assert.deepEqual(intro.cues, [{ dept: 'lighting', value: 'blue' }, { dept: 'laser', value: 'sweep' }])
  assert.ok(r.ok, r.error)
  const c = ensureCueNumbers(r.chart)
  const laneOf = (scene, dept) => c.states.find((s) => s.id === scene).timeline.lanes.find((l) => l.dept === dept)
  assert.equal(laneOf('INTRO', 'lighting').cues[0].action.payload.look, 'blue')
  assert.equal(laneOf('INTRO', 'laser').cues[0].action.payload.cue, 'sweep')
  assert.equal(laneOf('DROP', 'visual').cues[0].action.payload.scene, 'tunnel')
})

test('sentences, then and arrows also split scenes; the last scene length sets its duration', () => {
  const { d, r } = build('Doors open for 10 minutes, then a big drop for 45 seconds -> finale for 1 min')
  assert.deepEqual(d.scenes.map((s) => s.id), ['doors_open', 'drop', 'finale'])
  assert.ok(d.commands.includes('set finale length to 60 seconds'))
  assert.ok(r.ok, r.error)
})

test('what it cannot use is reported, never guessed', () => {
  const none = briefToCommands('')
  assert.deepEqual(none.commands, [])
  assert.match(none.notes[0], /No scenes found/)
  const dup = briefToCommands('Drop\nDrop')
  assert.deepEqual(dup.scenes.map((s) => s.id), ['drop', 'drop_2'])
  assert.ok(dup.notes.some((n) => n.includes('appears twice')))
  const blank = briefToCommands('Intro with lasers')
  assert.ok(blank.notes.some((n) => n.includes('no value')))
})
