// Tests for the cue sheet view of a showchart. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, newShowchart, buildCueSheet, sceneOrder, updateCue, removeCue, cueSheetCsv, ensureCueNumbers, duplicateCueNumbers } = sdk

const show = () => {
  const r = runCommandText(
    'new show; chain preshow, intro, drop, finale; add the standard lanes in intro; at 4 seconds lighting look blue in intro; at 2 seconds scene portal in intro; at 4 seconds laser cue l1 in intro; from intro to finale after 12 seconds; when entering drop set lighting look red',
    newShowchart(),
  )
  assert.ok(r.ok, r.error)
  return r.chart
}

test('scenes are listed in the order the show runs them', () => {
  assert.deepEqual(sceneOrder(show()), ['PRESHOW', 'INTRO', 'DROP', 'FINALE'])
})

test('cues are in calling order: by time, ties by lane', () => {
  const intro = buildCueSheet(show()).find((b) => b.id === 'INTRO')
  assert.deepEqual(intro.rows.map((r) => [r.at_ms, r.dept]), [[2000, 'visual'], [4000, 'lighting'], [4000, 'laser']])
})

test('enter actions appear as read-only rows, and the scene says how it is left', () => {
  const b = buildCueSheet(show())
  const drop = b.find((x) => x.id === 'DROP')
  assert.equal(drop.rows[0].source, 'enter')
  assert.equal(drop.rows[0].dept, 'lighting')
  assert.ok(b.find((x) => x.id === 'INTRO').leaves.some((l) => l.startsWith('after 12 s')))
  assert.deepEqual(b.find((x) => x.id === 'FINALE').leaves, [])
})

test('updateCue edits time, note and value, never mutates, and keeps a blank note out of the file', () => {
  const c = show()
  const snap = JSON.stringify(c)
  const row = buildCueSheet(c).find((b) => b.id === 'INTRO').rows[1]
  let n = updateCue(c, 'INTRO', row.cueId, { at_ms: 6500, label: 'Warm wash', value: 'amber' })
  assert.equal(JSON.stringify(c), snap)
  const cue = n.states.find((s) => s.id === 'INTRO').timeline.lanes.find((l) => l.id === 'lighting').cues[0]
  assert.deepEqual([cue.at_ms, cue.label, cue.action.payload.look], [6500, 'Warm wash', 'amber'])
  n = updateCue(n, 'INTRO', row.cueId, { label: '  ' })
  assert.equal('label' in n.states.find((s) => s.id === 'INTRO').timeline.lanes.find((l) => l.id === 'lighting').cues[0], false)
  assert.equal(updateCue(c, 'NOPE', 'x', { at_ms: 1 }).states.length, c.states.length)
})

test('removeCue removes exactly one cue', () => {
  const c = show()
  const row = buildCueSheet(c).find((b) => b.id === 'INTRO').rows[0]
  const n = removeCue(c, 'INTRO', row.cueId)
  assert.equal(buildCueSheet(n).find((b) => b.id === 'INTRO').rows.length, 2)
})

test('CSV: one line per row, quotes escaped, formulas neutralised', () => {
  let c = show()
  const row = buildCueSheet(c).find((b) => b.id === 'INTRO').rows[0]
  c = updateCue(c, 'INTRO', row.cueId, { label: '=HYPERLINK("http://x")', value: 'say "hi", ok' })
  const csv = cueSheetCsv(c)
  const lines = csv.trim().split('\r\n')
  assert.equal(lines.length, 1 + buildCueSheet(c).reduce((n, b) => n + b.rows.length, 0))
  assert.match(csv, /'=HYPERLINK/)
  assert.match(csv, /"say ""hi"", ok"/)
  assert.ok(!/(^|,)=HYPERLINK/m.test(csv))
})

const numbered = () => ensureCueNumbers(show())
const numsOf = (c) => buildCueSheet(c).flatMap((b) => b.rows).filter((r) => r.source === 'cue').map((r) => r.num)

test('numbers are assigned once, in show order, and enter actions get none', () => {
  const c = numbered()
  assert.deepEqual(numsOf(c), [1, 2, 3])
  assert.ok(buildCueSheet(c).flatMap((b) => b.rows).filter((r) => r.source !== 'cue').every((r) => r.num === null))
  assert.equal(c.cue_seq, 3)
  assert.equal(ensureCueNumbers(c), c, 'nothing to do returns the same object')
})

test('numbers stay fixed: adding, moving and deleting cues never renumbers the others', () => {
  let c = numbered()
  const before = Object.fromEntries(buildCueSheet(c).flatMap((b) => b.rows).filter((r) => r.cueId).map((r) => [r.cueId, r.num]))
  // a new cue earlier in the show gets the next number, not 1
  c = ensureCueNumbers(runCommandText('at 1 second lighting look red in intro', c).chart)
  const after = Object.fromEntries(buildCueSheet(c).flatMap((b) => b.rows).filter((r) => r.cueId).map((r) => [r.cueId, r.num]))
  for (const [id, n] of Object.entries(before)) assert.equal(after[id], n, id)
  assert.equal(after['lighting-1000'], 4)
  // moving a cue in time keeps its number
  const moved = updateCue(c, 'INTRO', 'lighting-1000', { at_ms: 9000 })
  assert.equal(buildCueSheet(moved).flatMap((b) => b.rows).find((r) => r.cueId === 'lighting-1000').num, 4)
  // delete the highest, add another: the number is not reused
  const gone = removeCue(c, 'INTRO', 'lighting-1000')
  const again = ensureCueNumbers(runCommandText('at 1 second lighting look green in intro', gone).chart)
  assert.equal(buildCueSheet(again).flatMap((b) => b.rows).find((r) => r.cueId === 'lighting-1000').num, 5)
})

test('a number can be set by hand (12.5 slots in), and duplicates are reported', () => {
  let c = numbered()
  c = updateCue(c, 'INTRO', 'lighting-4000', { num: 2.5 })
  assert.deepEqual(numsOf(c), [1, 2.5, 3])
  assert.equal(duplicateCueNumbers(c).size, 0)
  c = updateCue(c, 'INTRO', 'lighting-4000', { num: 3 })
  assert.deepEqual([...duplicateCueNumbers(c)], [3])
  assert.deepEqual(numsOf(updateCue(c, 'INTRO', 'lighting-4000', { num: -4 })), [1, 3, 3], 'invalid numbers are ignored')
})

test('CSV carries the stored numbers', () => {
  assert.match(cueSheetCsv(numbered()), /^Q1,INTRO/m)
})
