// Tests for the cue sheet view of a showchart. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { runCommandText, newShowchart, buildCueSheet, sceneOrder, updateCue, removeCue, cueSheetCsv } = sdk

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

test('cues are in calling order: by time, ties by lane, numbered across the sheet', () => {
  const intro = buildCueSheet(show()).find((b) => b.id === 'INTRO')
  assert.deepEqual(intro.rows.map((r) => [r.at_ms, r.dept]), [[2000, 'visual'], [4000, 'lighting'], [4000, 'laser']])
  const all = buildCueSheet(show()).flatMap((b) => b.rows).map((r) => r.n)
  assert.deepEqual(all, all.map((_, i) => i + 1))
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
