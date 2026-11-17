// Tests for time on a timeline: written, read, snapped, nudged, ruled. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { fmtTime, fmtTick, parseTime, snapMs, nudgeMs, rulerTicks } = sdk

test('a time is written with exactly the decimals it needs, so what is shown is what is stored', () => {
  assert.equal(fmtTime(0), '0:00.0')
  assert.equal(fmtTime(12500), '0:12.5')
  assert.equal(fmtTime(12250), '0:12.25')
  assert.equal(fmtTime(12005), '0:12.005')
  assert.equal(fmtTime(12050), '0:12.05')
  assert.equal(fmtTime(61000), '1:01.0')
  assert.equal(fmtTime(3599999), '59:59.999')
  assert.equal(fmtTime(-5), '0:00.0')
})

test('a ruler label is whole seconds from one second up and exact below', () => {
  assert.equal(fmtTick(12000, 1000), '0:12')
  assert.equal(fmtTick(65000, 5000), '1:05')
  assert.equal(fmtTick(12500, 500), '0:12.5')
})

test('typed times: seconds, minutes and seconds, a comma, a trailing s', () => {
  for (const [text, ms] of [['12', 12000], ['12.5', 12500], ['12,5', 12500], [' 12.25 s ', 12250], ['0:12.5', 12500], ['1:02', 62000], ['1:02.25', 62250], ['0', 0], ['90', 90000]]) {
    assert.equal(parseTime(text), ms, text)
  }
})

test('things that are not times are refused, never guessed at', () => {
  for (const text of ['', ' ', 'abc', '-1', '1:75', '1:2:3', '12..5', '1e3', '0:-5', '12 sec']) assert.equal(parseTime(text), null, JSON.stringify(text))
})

test('what is written can be read back exactly', () => {
  for (const ms of [0, 100, 250, 12250, 12005, 61000, 125500]) assert.equal(parseTime(fmtTime(ms)), ms, String(ms))
})

test('snapping goes to the nearest grid line and never below zero', () => {
  assert.equal(snapMs(1130, 250), 1250)
  assert.equal(snapMs(1120, 250), 1000)
  assert.equal(snapMs(-300, 250), 0)
  assert.equal(snapMs(1234.6, 0), 1235)
  assert.equal(snapMs(4999, 1000), 5000)
})

test('a nudge lands on the next grid line, so an off-grid cue gets onto the grid', () => {
  assert.equal(nudgeMs(1000, 1, 250), 1250)
  assert.equal(nudgeMs(1000, -1, 250), 750)
  assert.equal(nudgeMs(1100, 1, 250), 1250)
  assert.equal(nudgeMs(1100, -1, 250), 1000)
  assert.equal(nudgeMs(0, -1, 250), 0)
})

test('a big nudge is a second, and with no grid a nudge is 100 ms', () => {
  assert.equal(nudgeMs(1500, 1, 250, true), 2000)
  assert.equal(nudgeMs(1500, -1, 250, true), 1000)
  assert.equal(nudgeMs(1000, 1, 0), 1100)
  assert.equal(nudgeMs(50, -1, 0), 0)
})

test('a ruler labels no closer than a label is wide, and draws minor marks only when they fit', () => {
  for (const pps of [10, 30, 60, 90, 240, 800]) {
    const { ticks, majorMs } = rulerTicks(60000, pps)
    assert.ok(majorMs * (pps / 1000) >= 64 || majorMs === 3600000, `labels too close at ${pps} px/s (major ${majorMs})`)
    const majors = ticks.filter((t) => t.major)
    assert.ok(majors.length >= 1 && majors[0].ms === 0)
    for (let i = 1; i < majors.length; i++) assert.equal(majors[i].ms - majors[i - 1].ms, majorMs)
  }
})

test('zoomed in a ruler shows quarter seconds, zoomed out only the big steps', () => {
  const near = rulerTicks(10000, 120)
  assert.equal(near.majorMs, 1000)
  assert.ok(near.ticks.some((t) => !t.major && t.ms === 250), 'quarter-second marks when zoomed in')
  const far = rulerTicks(600000, 4)
  assert.ok(far.majorMs >= 15000)
  assert.ok(far.ticks.every((t) => t.major || t.ms % far.majorMs !== 0))
})

test('every tick is a whole number of milliseconds, with no float drift', () => {
  const { ticks } = rulerTicks(120000, 100)
  assert.ok(ticks.every((t) => Number.isInteger(t.ms)))
})
