// Tests for the friendlier command bar: scene synonyms, systems by sentence, suggestions. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runStudioCommand, runCommandText, newShowchart, suggestCommand, hintFor } = sdk

const show = () => runCommandText('new show; chain preshow, intro, drop, finale', newShowchart()).chart
const blank = () => runCommandText('new show', newShowchart()).chart
const empty = { nodes: [], edges: [] }
let n = 0
const newId = () => `t${++n}`
const run = (text, chart = show(), rig = empty) => runStudioCommand(text, chart, rig, { newId })
const ids = (c) => c.states.map((s) => s.id)

test('scene and state mean the same thing', () => {
  for (const t of ['add scene test', 'add a scene called test', 'new scene test', 'create scene test', 'add state test']) {
    const r = run(t)
    assert.ok(r.ok, `${t}: ${r.error}`)
    assert.ok(ids(r.chart).includes('TEST'), t)
  }
  assert.deepEqual(ids(run('add scenes a1, a2 and a3').chart).slice(-3), ['A1', 'A2', 'A3'])
  assert.ok(!ids(run('delete scene drop').chart).includes('DROP'))
  assert.ok(!ids(run('delete drop').chart).includes('DROP'))
  assert.ok(!ids(run('remove the drop scene').chart).includes('DROP'))
})

test('duplicate copies a scene with its cues and gives the copies new cue numbers', () => {
  const c = sdk.ensureCueNumbers(runCommandText('at 2 seconds lighting look blue in intro', show()).chart)
  const r = run('duplicate scene intro as intro_b', c)
  assert.ok(r.ok, r.error)
  const lane = (chart, id) => chart.states.find((s) => s.id === id).timeline.lanes.find((l) => l.dept === 'lighting')
  assert.equal(lane(r.chart, 'INTRO_B').cues.length, 1)
  const numbered = sdk.ensureCueNumbers(r.chart)
  assert.notEqual(lane(numbered, 'INTRO_B').cues[0].num, lane(numbered, 'INTRO').cues[0].num, 'the copy has its own cue number')
  assert.equal(run('duplicate drop').chart.states.some((s) => s.id === 'DROP_2'), true)
  assert.equal(run('duplicate intro as drop').ok, false)
})

test('a then b then c is a chain; make a show with ... builds one', () => {
  const r = run('make a show with a, b and c', blank())
  assert.ok(r.ok, r.error)
  assert.deepEqual(ids(r.chart), ['A', 'B', 'C'])
  assert.equal(r.chart.transitions.length, 2)
  assert.deepEqual(ids(run('x then y then z', blank()).chart), ['X', 'Y', 'Z'])
})

test('systems are added by sentence, with a name, an address, a port and a department', () => {
  let r = run('add qlab')
  assert.ok(r.ok && r.rigChanged, r.error)
  assert.equal(r.rig.nodes[0].type, 'qlab')
  assert.equal(r.rig.nodes[0].params.port, 53000)
  assert.match(r.next, /audio snapshot/)
  r = run('add x32 called FOH desk at 192.168.1.20:10023')
  assert.deepEqual([r.rig.nodes[0].label, r.rig.nodes[0].params.host, r.rig.nodes[0].params.port], ['FOH desk', '192.168.1.20', 10023])
  assert.equal(run('add eos listening on 8001').rig.nodes[0].params.listen_port, 8001)
  assert.equal(run('add grandma3 for visuals').rig.nodes[0].dept, 'visual')
  r = run('add resolume on 10.0.0.5')
  assert.equal(r.rig.nodes[0].params.host, '10.0.0.5')
  assert.equal(r.rig.nodes[0].params.port, 7000, "the tool's own default port stays")
  assert.equal(run('connect beyond').rig.nodes[0].type, 'beyond')
  assert.equal(run('add a lighting desk').rig.nodes[0].type, 'lighting-desk')
  assert.deepEqual(run('add 3 lasers').rig.nodes.map((x) => x.label), ['Laser', 'Laser 2', 'Laser 3'])
})

test('a cue sentence about a laser is a cue, not a new laser', () => {
  const r = run('add a laser cue at 4 seconds in intro')
  assert.ok(r.ok && !r.rigChanged, r.error)
  assert.equal(r.rig.nodes.length, 0)
})

test('systems can be changed and removed by sentence', () => {
  let rig = run('add qlab').rig
  rig = run('add x32 called FOH', show(), rig).rig
  let r = run('set qlab host to 192.168.1.50', show(), rig)
  assert.equal(r.rig.nodes.find((x) => x.type === 'qlab').params.host, '192.168.1.50')
  r = run('set foh port 10024', show(), rig)
  assert.equal(r.rig.nodes.find((x) => x.label === 'FOH').params.port, 10024)
  assert.ok(run('rename qlab to Playback', show(), rig).rig.nodes.some((x) => x.label === 'Playback'))
  assert.equal(run('qlab for visuals', show(), rig).rig.nodes.find((x) => x.type === 'qlab').dept, 'visual')
  assert.equal(run('remove foh', show(), rig).rig.nodes.length, 1)
  assert.equal(run('set qlab host to not an address!', show(), rig).ok, false)
  assert.equal(run('set qlab port 99999', show(), rig).ok, false)
  // a label is exact: with "QLab" and "QLab 2", "qlab 2" is the second
  const two = run('add qlab; add qlab').rig
  assert.deepEqual(run('remove qlab 2', show(), two).rig.nodes.map((x) => x.label), ['QLab'])
  // two of the same tool with other names: the tool name alone is ambiguous, and it says so
  const named = run('add qlab called Playback; add qlab called Stage').rig
  const amb = run('remove qlab', show(), named)
  assert.equal(amb.ok, false)
  assert.match(amb.error, /more than one/)
  assert.match(amb.hint, /Playback, Stage/)
  assert.deepEqual(run('remove stage', show(), named).rig.nodes.map((x) => x.label), ['Playback'])
  // removing something that is not there says so, and what is
  const missing = run('remove resolume', show(), rig)
  assert.equal(missing.ok, false)
  assert.match(missing.hint, /The rig has/)
})

test('a sentence can change the show and the rig together, all or nothing', () => {
  const r = run('add scene encore; add qlab; at 2 seconds qlab cue 3 in encore')
  assert.ok(r.ok, r.error)
  assert.ok(ids(r.chart).includes('ENCORE') && r.rig.nodes.length === 1)
  const cue = r.chart.states.find((s) => s.id === 'ENCORE').timeline.lanes[0].cues[0]
  assert.deepEqual([cue.action.kind, String(cue.action.payload.snapshot)], ['audio.snapshot', '3'])
  assert.equal(run('add qlab; add scene intro').ok, false, 'INTRO already exists, so nothing at all is applied')
})

test('tool names work as actions', () => {
  const kinds = (t) => {
    const r = runCommandText(t, show())
    assert.ok(r.ok, `${t}: ${r.error}`)
    return r.steps.map((s) => s.command.action?.kind)
  }
  assert.deepEqual(kinds('at 1 seconds eos cue 5 in intro'), ['lighting.look'])
  assert.deepEqual(kinds('at 1 seconds x32 scene 12 in intro'), ['audio.snapshot'])
  assert.deepEqual(kinds('at 1 seconds resolume 2/5 in intro'), ['scene.activate'])
  assert.deepEqual(kinds('at 1 seconds beyond sweep in intro'), ['laser.cue'])
  assert.deepEqual(kinds('at 1 seconds grandma3 402 in intro'), ['lighting.look'])
  assert.deepEqual(kinds('at 1 seconds magicq 3 in intro'), ['lighting.look'])
})

test('what is not understood gets the closest working command, using real names', () => {
  const c = show()
  assert.equal(suggestCommand('ad qlab', c), 'add qlab')
  assert.equal(suggestCommand('add scen drop', c), 'add scene drop')
  assert.equal(suggestCommand('qwerty', c), null)
  assert.match(hintFor('delet scene intro', c), /^Did you mean: delete scene/)
  assert.match(hintFor('zzzz', c), /^Try: add scene drop/)
  const r = run('ad qlab')
  assert.equal(r.ok, false)
  assert.match(r.hint, /Did you mean: add qlab/)
  assert.match(hintFor('from preshow to', c), /preshow/, 'names in the hint come from THIS show')
})

test('use <device> for <department> picks the device on the lanes, in every scene or one', () => {
  const base = runCommandText('at 1 seconds audio snapshot 1 in intro; at 1 seconds audio snapshot 2 in drop', show()).chart
  const rig = run('add qlab; add x32 called FOH').rig
  const lanes = (c, scene) => c.states.find((s) => s.id === scene).timeline.lanes.filter((l) => l.dept === 'audio')
  let r = run('use foh for audio', base, rig)
  assert.ok(r.ok, r.error)
  assert.ok(!r.rigChanged, 'only the show changed')
  const fohId = rig.nodes.find((x) => x.label === 'FOH').id
  assert.equal(lanes(r.chart, 'INTRO')[0].device, fohId)
  assert.equal(lanes(r.chart, 'DROP')[0].device, fohId)
  r = run('use qlab for audio in drop', base, rig)
  assert.equal(lanes(r.chart, 'DROP')[0].device, rig.nodes.find((x) => x.type === 'qlab').id)
  assert.equal(lanes(r.chart, 'INTRO')[0].device, undefined)
  // the rig check no longer reports the lane as ambiguous
  const issues = sdk.rigCheck(run('use foh for audio', base, rig).chart, rig.nodes)
  assert.ok(!issues.some((i) => i.kind === 'ambiguous'))
  // plain mistakes get a reason
  assert.match(run('use foh for lighting', base, rig).error, /serves audio/)
  assert.match(run('use foh for audio in nowhere', base, rig).error, /don't know a scene/)
  assert.match(run('use foh for lasers', base, rig).error, /serves audio/)
  // naming a tool that is not in the rig yet adds it
  const added = run('use resolume for visuals', base, rig)
  assert.ok(added.ok && added.rig.nodes.some((x) => x.type === 'resolume'))
  assert.match(run('use resolume for visuals in drop', base, rig).error, /no resolume in the rig/, 'but assigning it to a scene needs it to exist first')
  assert.equal(run('use qlab for audio', base, rig).rig.nodes.filter((x) => x.type === 'qlab').length, 1, 'use an existing one, do not add another')
  assert.match(run('use foh for audio', show(), rig).error, /no audio lane/)
})
