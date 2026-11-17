// Builds the three stage-show examples in this folder.
//
//   cd pchi-studio-react && npm run test:sdk     # builds .sdk-test/sdk.cjs, which this script uses to number cues and check the result
//   node tools/examples/build-examples.mjs       # writes stadium-pop-tour.pchi, arena-pop-show.pchi, stadium-preshow-doors.pchi
//
// Everything in them is INVENTED: song titles, timings, cue values, addresses and ports. They show the SCALE and the SHAPE of a
// big pop show (acts, a B-stage, an encore, a rig with walls, lights, lasers, desks, playback and stage automation), not any real
// tour's setlist or settings. Replace the placeholder addresses in the Rig tab before using one against real gear.
import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const sdk = createRequire(import.meta.url)(join(here, '../../pchi-studio-react/.sdk-test/sdk.cjs'))
const { ensureCueNumbers, validateShow } = sdk

// ───────── small helpers ─────────

let uid = 0
const nid = (p) => `${p}${(++uid).toString(36)}`
const q25 = (s) => Math.round(s * 4) / 4 // seconds, on the quarter-second grid
const cue = (at, kind, payload, label, len) => ({ id: nid('c'), at_ms: Math.round(q25(at) * 1000), ...(len ? { length_ms: Math.round(len * 1000) } : {}), ...(label ? { label } : {}), action: { kind, payload } })
const lane = (id, name, dept, device, cues) => ({ id, name, dept, device, cues: cues.sort((a, b) => a.at_ms - b.at_ms) })
const look = (at, exec, label) => cue(at, 'lighting.look', { look: String(exec) }, label)
const wall = (at, what, label) => cue(at, 'scene.activate', { scene: String(what) }, label) // a column (12) or layer/clip (2/5)
const desk = (at, scene, label) => cue(at, 'audio.snapshot', { snapshot: String(scene) }, label) // a desk scene number
const play = (at, qlabCue, label) => cue(at, 'audio.snapshot', { snapshot: String(qlabCue) }, label) // a QLab cue number
const laser = (at, name, label, len) => cue(at, 'laser.cue', { cue: name }, label, len)
const run = (at, program, label, len) => cue(at, 'motion.run', { program }, label, len)

const node = (id, type, label, x, y, params, dept) => ({ id, type, label, position: { x, y }, ...(dept ? { dept } : {}), params })
const RIG = {
  walls: node('walls', 'resolume', 'LED walls (Resolume)', 0, 0, { host: '10.20.1.10', port: 7000 }),
  lights: node('lights', 'grandma3', 'Lighting (grandMA3)', 260, 0, { host: '10.20.2.10', port: 8000 }),
  lasers: node('lasers', 'beyond', 'Lasers (BEYOND)', 520, 0, { host: '10.20.3.10', port: 8000 }),
  foh: node('foh', 'x32', 'FOH desk (X32)', 0, 150, { host: '10.20.4.10', port: 10023 }),
  playback: node('playback', 'qlab', 'Playback (QLab)', 260, 150, { host: '10.20.4.20', port: 53000 }),
  stage: node('stage', 'motion', 'Stage automation', 520, 150, { host: '10.20.5.10', port: 9000, programs: 'LIFT_UP, LIFT_DOWN, BSTAGE_OUT, BSTAGE_HOME' }, 'motion'),
}

/** One scene: its timeline (lanes of cues) and how long it is. */
const scene = (id, label, dur, lanes, extra = {}) => ({ id, label, ...extra, timeline: { duration_ms: Math.round(dur * 1000), lanes: lanes.filter((l) => l.cues.length) } })

/** The lanes of a song: the same shape for every song (look changes through the verses and choruses), varied by number so no two are identical. */
function songScene(i, s) {
  const d = s.dur
  const at = (f) => d * f
  const base = 100 + i * 10 // this song's family of grandMA3 executors
  const col = 10 + i * 2 // this song's LED column
  const lights = [look(0, base, 'Song look'), look(at(0.12), base + 1, 'Verse 1'), look(at(0.3), base + 2, 'Chorus 1'), look(at(0.48), base + 3, 'Verse 2'), look(at(0.6), base + 4, 'Bridge'), look(at(0.76), base + 5, 'Final chorus')]
  const walls = [wall(0, col, 'Song visuals'), wall(at(0.3), `2/${(i % 6) + 1}`, 'Chorus overlay'), wall(at(0.58), col + 1, 'Bridge visuals'), wall(at(0.76), `2/${(i % 6) + 2}`, 'Final chorus overlay')]
  const lanes = [
    lane('walls', 'LED walls', 'visual', 'walls', walls),
    lane('lights', 'Lighting', 'lighting', 'lights', lights),
    lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, s.fohScene ?? 10 + i, 'Desk scene')]),
  ]
  if (s.playback !== false) lanes.push(lane('playback', 'Playback', 'audio', 'playback', [play(0, `T${i + 1}`, 'Track start')]))
  if (s.lasers) lanes.push(lane('lasers', 'Lasers', 'laser', 'lasers', [laser(at(0.76), s.lasers, 'Laser look', at(0.2)), laser(at(0.97), 'LASERS_OFF', 'Lasers off')]))
  if (s.stage) lanes.push(lane('stage', 'Stage', 'motion', 'stage', s.stage))
  return scene(s.id, s.label, d, lanes, { min_ms: 45000 }) // a stray GO in the first 45 s is held, so a song cannot be skipped by accident
}

const chain = (ids, event = 'NEXT') => ids.slice(0, -1).map((source, k) => ({ source, target: ids[k + 1], event }))

function finish(chart, rig, name, project, layout) {
  const showchart = ensureCueNumbers({ ...chart, name, layout })
  const problems = validateShow(showchart, rig)
  const errors = problems.filter((p) => p.level === 'error')
  if (errors.length) throw new Error(`${name}: ${errors.map((e) => e.text).join('; ')}`)
  return { problems, file: { version: '1.0', project, name, nodes: rig, edges: [], timeline: [], assets: [], showchart } }
}

/** Place scenes on the canvas in rows, one row per group, so the Show tab reads as the running order. */
function grid(rows, w = 270, h = 170) {
  const out = {}
  rows.forEach((ids, r) => ids.forEach((id, c) => (out[id] = { x: c * w, y: r * h })))
  return out
}

// ═════════════ 1. Stadium pop tour: the full show ═════════════
function stadium() {
  const songs = [
    // Act 1
    { id: 'ACT1_OPENER', label: 'Act 1: Glass Cathedral (opener)', dur: 255, lasers: 'FAN_AURORA', stage: [run(0, 'LIFT_UP', 'Entrance platform up', 20)] },
    { id: 'ACT1_S2', label: 'Act 1: Paper Satellites', dur: 225 },
    { id: 'ACT1_S3', label: 'Act 1: Golden Static', dur: 240, lasers: 'GOLD_RAIN' },
    // Act 2
    { id: 'ACT2_S1', label: 'Act 2: Velvet Alarm', dur: 210 },
    { id: 'ACT2_S2', label: 'Act 2: Coastline Radio', dur: 235 },
    { id: 'ACT2_S3', label: 'Act 2: Ten Thousand Windows', dur: 270, lasers: 'WINDOW_GRID' },
    // B-stage acoustic set
    { id: 'BSTAGE_S1', label: 'B-stage: Kitchen Floor (acoustic)', dur: 200, playback: false, lasers: undefined },
    { id: 'BSTAGE_S2', label: 'B-stage: Lanterns (acoustic)', dur: 215, playback: false },
    // Act 3
    { id: 'ACT3_S1', label: 'Act 3: Midnight Parade', dur: 245, lasers: 'PARADE_SWEEP' },
    { id: 'ACT3_S2', label: 'Act 3: Starlit Ledger', dur: 230 },
    { id: 'ACT3_S3', label: 'Act 3: Wildfire Hymn', dur: 285, lasers: 'WILDFIRE_FULL' },
    // Encore
    { id: 'ENCORE_S1', label: 'Encore: Open Sky', dur: 240, lasers: 'SKY_FAN' },
    { id: 'ENCORE_S2', label: 'Encore: Last Light (finale)', dur: 300, lasers: 'FINALE_FULL' },
  ]
  const S = Object.fromEntries(songs.map((s, i) => [s.id, songScene(i, s)]))
  const bar = (id, label, dur, lanes, extra) => scene(id, label, dur, lanes, extra)
  const states = [
    bar('HOUSE', 'Doors: house loop', 600, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 1, 'House loop')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 1, 'House look')]), lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, 1, 'House desk scene')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H1', 'House playlist')])]),
    bar('SUPPORT_CHANGE', 'Support act changeover', 1500, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 2, 'Changeover loop'), wall(1200, 3, '5 minute slide')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 2, 'Changeover look')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H2', 'Changeover playlist')])]),
    bar('COUNTDOWN', 'Countdown clock (3:00)', 180, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 4, 'Countdown clock')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 3, 'Build look'), look(150, 4, 'Final 30 s')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'CD', 'Rising bed')])]),
    bar('INTRO_FILM', 'Intro film', 95, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 6, 'Intro film')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 5, 'Dark'), look(88, 6, 'Blackout before the entrance')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'FILM', 'Film audio')])]),
    ...songs.slice(0, 6).map((s) => S[s.id]),
    bar('BSTAGE_OUT', 'B-stage travel out', 90, [lane('stage', 'Stage', 'motion', 'stage', [run(2, 'BSTAGE_OUT', 'B-stage travels out', 80)]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 300, 'Travel look')]), lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 20, 'Travel graphic')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'WALK', 'Walk music')])]),
    S.BSTAGE_S1,
    S.BSTAGE_S2,
    bar('BSTAGE_BACK', 'B-stage travel home', 90, [lane('stage', 'Stage', 'motion', 'stage', [run(2, 'BSTAGE_HOME', 'B-stage travels home', 80)]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 301, 'Travel look')]), lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 21, 'Travel graphic')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'WALK2', 'Walk music')])]),
    ...songs.slice(8, 11).map((s) => S[s.id]),
    bar('ENCORE_BREAK', 'Encore break (chant)', 240, [lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 7, 'Low blue')]), lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 22, 'Chant visuals')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H3', 'Break music')])]),
    ...songs.slice(11).map((s) => S[s.id]),
    bar('HOUSE_UP', 'House lights up, exit music', 300, [lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 8, 'House up')]), lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 23, 'Thank you')]), lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, 2, 'Exit desk scene')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'OUT', 'Exit music')])]),
    { id: 'END', label: 'Show over' },
  ]
  const order = ['HOUSE', 'SUPPORT_CHANGE', 'COUNTDOWN', 'INTRO_FILM', ...songs.slice(0, 6).map((s) => s.id), 'BSTAGE_OUT', 'BSTAGE_S1', 'BSTAGE_S2', 'BSTAGE_BACK', ...songs.slice(8, 11).map((s) => s.id), 'ENCORE_BREAK', ...songs.slice(11).map((s) => s.id), 'HOUSE_UP', 'END']
  const transitions = [
    ...chain(['HOUSE', 'SUPPORT_CHANGE', 'COUNTDOWN']),
    { source: 'COUNTDOWN', target: 'INTRO_FILM', after_ms: 180000 },
    { source: 'INTRO_FILM', target: 'ACT1_OPENER', after_ms: 95000 },
    ...chain(order.slice(order.indexOf('ACT1_OPENER'), order.length)),
  ]
  const layout = grid([
    ['HOUSE', 'SUPPORT_CHANGE', 'COUNTDOWN', 'INTRO_FILM'],
    ['ACT1_OPENER', 'ACT1_S2', 'ACT1_S3'],
    ['ACT2_S1', 'ACT2_S2', 'ACT2_S3'],
    ['BSTAGE_OUT', 'BSTAGE_S1', 'BSTAGE_S2', 'BSTAGE_BACK'],
    ['ACT3_S1', 'ACT3_S2', 'ACT3_S3'],
    ['ENCORE_BREAK', 'ENCORE_S1', 'ENCORE_S2', 'HOUSE_UP', 'END'],
  ])
  return finish({ id: 'stadium-pop-tour', initial: 'HOUSE', states, transitions }, Object.values(RIG), 'stadium-pop-tour', 'Stadium pop tour (example)', layout)
}

// ═════════════ 2. Arena pop show: condensed, a smaller rig ═════════════
function arena() {
  const rig = [RIG.walls, RIG.lights, RIG.foh, RIG.playback].map((n, k) => ({ ...n, position: { x: k * 260, y: 0 } }))
  const songs = [
    { id: 'SET1_OPENER', label: 'Set 1: Glass Cathedral (opener)', dur: 240, lasers: undefined },
    { id: 'SET1_S2', label: 'Set 1: Paper Satellites', dur: 215 },
    { id: 'SET1_S3', label: 'Set 1: Golden Static', dur: 230 },
    { id: 'ACOUSTIC', label: 'Acoustic: Kitchen Floor', dur: 200, playback: false },
    { id: 'SET2_S1', label: 'Set 2: Velvet Alarm', dur: 205 },
    { id: 'SET2_S2', label: 'Set 2: Coastline Radio', dur: 225 },
    { id: 'SET2_S3', label: 'Set 2: Ten Thousand Windows', dur: 260 },
    { id: 'ENCORE', label: 'Encore: Last Light', dur: 280 },
  ]
  const S = Object.fromEntries(songs.map((s, i) => [s.id, songScene(i, s)]))
  const bar = (id, label, dur, lanes, extra) => scene(id, label, dur, lanes, extra)
  const states = [
    bar('HOUSE', 'Doors: house loop', 600, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 1, 'House loop')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 1, 'House look')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H1', 'House playlist')])]),
    bar('COUNTDOWN', 'Countdown clock (2:00)', 120, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 4, 'Countdown clock')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 3, 'Build look')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'CD', 'Rising bed')])]),
    bar('INTRO', 'Intro film', 60, [lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 6, 'Intro film')]), lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 5, 'Dark')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'FILM', 'Film audio')])]),
    ...songs.slice(0, 7).map((s) => S[s.id]),
    bar('ENCORE_BREAK', 'Encore break', 180, [lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 7, 'Low blue')]), lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 22, 'Chant visuals')])]),
    S.ENCORE,
    bar('HOUSE_UP', 'House lights up', 240, [lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 8, 'House up')]), lane('playback', 'Playback', 'audio', 'playback', [play(0, 'OUT', 'Exit music')])]),
    { id: 'END', label: 'Show over' },
  ]
  const order = ['HOUSE', 'COUNTDOWN', 'INTRO', ...songs.slice(0, 7).map((s) => s.id), 'ENCORE_BREAK', 'ENCORE', 'HOUSE_UP', 'END']
  const transitions = [{ source: 'HOUSE', target: 'COUNTDOWN', event: 'NEXT' }, { source: 'COUNTDOWN', target: 'INTRO', after_ms: 120000 }, { source: 'INTRO', target: 'SET1_OPENER', after_ms: 60000 }, ...chain(order.slice(order.indexOf('SET1_OPENER')))]
  const layout = grid([['HOUSE', 'COUNTDOWN', 'INTRO'], ['SET1_OPENER', 'SET1_S2', 'SET1_S3', 'ACOUSTIC'], ['SET2_S1', 'SET2_S2', 'SET2_S3'], ['ENCORE_BREAK', 'ENCORE', 'HOUSE_UP', 'END']])
  return finish({ id: 'arena-pop-show', initial: 'HOUSE', states, transitions }, rig, 'arena-pop-show', 'Arena pop show (example)', layout)
}

// ═════════════ 3. Stadium doors and pre-show: a day run by the clock ═════════════
function preshow() {
  const rig = [RIG.walls, RIG.lights, RIG.foh, RIG.playback].map((n, k) => ({ ...n, position: { x: k * 260, y: 0 } }))
  const min = (m) => m * 60
  const bar = (id, label, dur, lanes) => scene(id, label, dur, lanes)
  const states = [
    bar('GATES_OPEN', 'Gates open: house loop (90 min)', min(90), [
      lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 1, 'House loop'), wall(min(80), 7, 'Show starts soon slide')]),
      lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 1, 'House look'), look(min(85), 9, 'Pre-support build')]),
      lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, 1, 'House desk scene')]),
      lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H1', 'House playlist'), play(min(60), 'SAFE1', 'Safety announcement'), play(min(85), 'SAFE2', 'Support act soon')]),
    ]),
    bar('SUPPORT_SET', 'Support act (45 min)', min(45), [
      lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 8, 'Support act content')]),
      lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 20, 'Support wash'), look(min(40), 21, 'Support finale')]),
      lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, 5, 'Support desk scene')]),
    ]),
    bar('CHANGEOVER', 'Changeover (30 min)', min(30), [
      lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 2, 'Changeover loop'), wall(min(25), 3, '5 minute slide')]),
      lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 2, 'Changeover look')]),
      lane('foh', 'FOH desk', 'audio', 'foh', [desk(0, 1, 'House desk scene')]),
      lane('playback', 'Playback', 'audio', 'playback', [play(0, 'H2', 'Changeover playlist'), play(min(20), 'SAFE3', 'Safety reminder')]),
    ]),
    bar('COUNTDOWN', 'Countdown clock (3:00)', 180, [
      lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 4, 'Countdown clock')]),
      lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 3, 'Build look'), look(150, 4, 'Final 30 s')]),
      lane('playback', 'Playback', 'audio', 'playback', [play(0, 'CD', 'Rising bed')]),
    ]),
    bar('INTRO_FILM', 'Intro film', 95, [
      lane('walls', 'LED walls', 'visual', 'walls', [wall(0, 6, 'Intro film')]),
      lane('lights', 'Lighting', 'lighting', 'lights', [look(0, 5, 'Dark')]),
      lane('playback', 'Playback', 'audio', 'playback', [play(0, 'FILM', 'Film audio')]),
    ]),
    { id: 'HANDOVER', label: 'Hand over to the show file' },
  ]
  const transitions = [
    { source: 'GATES_OPEN', target: 'SUPPORT_SET', after_ms: min(90) * 1000 }, { source: 'GATES_OPEN', target: 'SUPPORT_SET', event: 'NEXT' },
    { source: 'SUPPORT_SET', target: 'CHANGEOVER', after_ms: min(45) * 1000 }, { source: 'SUPPORT_SET', target: 'CHANGEOVER', event: 'NEXT' },
    { source: 'CHANGEOVER', target: 'COUNTDOWN', after_ms: min(30) * 1000 }, { source: 'CHANGEOVER', target: 'COUNTDOWN', event: 'NEXT' },
    { source: 'COUNTDOWN', target: 'INTRO_FILM', after_ms: 180000 },
    { source: 'INTRO_FILM', target: 'HANDOVER', after_ms: 95000 },
  ]
  const layout = grid([['GATES_OPEN', 'SUPPORT_SET', 'CHANGEOVER'], ['COUNTDOWN', 'INTRO_FILM', 'HANDOVER']], 300)
  return finish({ id: 'stadium-preshow-doors', initial: 'GATES_OPEN', states, transitions }, rig, 'stadium-preshow-doors', 'Stadium doors and pre-show (example)', layout)
}

// ───────── write them out ─────────
const out = [
  ['stadium-pop-tour.pchi', stadium()],
  ['arena-pop-show.pchi', arena()],
  ['stadium-preshow-doors.pchi', preshow()],
]
for (const [file, { file: show, problems }] of out) {
  writeFileSync(join(here, file), JSON.stringify(show, null, 2) + '\n')
  const cues = show.showchart.states.reduce((n, s) => n + (s.timeline?.lanes ?? []).reduce((m, l) => m + l.cues.length, 0), 0)
  console.log(`${file}: ${show.showchart.states.length} scenes, ${cues} cues, ${show.nodes.length} devices, ${problems.length} things to look at`)
  for (const p of problems) console.log(`   ${p.level}: ${p.text}`)
}
