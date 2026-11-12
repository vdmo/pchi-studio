// Tests for live-show commands and the start-screen templates. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { parseLiveCommand, runCommandText, newShowchart } = sdk

const chart = () => {
  const r = runCommandText('new show; chain preshow, intro, drop, finale; from intro to finale on skip if energy > 0.5', newShowchart())
  assert.ok(r.ok, r.error)
  return r.chart
}
const live = (text, state = 'INTRO', ctx = {}) => parseLiveCommand(text, chart(), state, ctx)

test('go to a state sends the event that leads there', () => {
  const r = live('go to drop')
  assert.ok(r.ok, r.error)
  assert.deepEqual(r.action, { kind: 'event', event: 'NEXT' })
})

test('speech slips and phrasing variants', () => {
  assert.equal(live('jump to the DROP').action.event, 'NEXT')
  assert.ok(live('go to dropp').notes.length > 0)
})

test('a guarded transition is refused with the reason', () => {
  const r = live('go to finale')
  assert.ok(!r.ok)
  assert.match(r.error, /blocked/)
  assert.ok(live('go to finale', 'INTRO', { energy: 0.9 }).ok)
})

test('"next" works only when there is exactly one way on', () => {
  assert.equal(live('next', 'FINALE').ok, false) // the last state has nothing to go on to
  assert.equal(live('next', 'INTRO').action.event, 'NEXT') // skip is guarded off
  assert.match(live('next', 'INTRO', { energy: 0.9 }).error, /2 ways/)
})

test('unknown state, same state and no route give helpful errors', () => {
  assert.match(live('go to zzzz').error, /no state called/)
  assert.match(live('go to intro').error, /Already/)
  assert.match(live('go to preshow').error, /no way/)
})

test('restart, set, and sending an event by name', () => {
  assert.deepEqual(live('restart the show').action, { kind: 'reset' })
  assert.deepEqual(live('set energy to 0.8').action, { kind: 'context', key: 'energy', value: 0.8 })
  assert.deepEqual(live('set mode to calm').action, { kind: 'context', key: 'mode', value: 'calm' })
  assert.equal(live('send next').action.event, 'NEXT')
  assert.ok(!live('make me a sandwich').ok)
})

test('not connected', () => {
  assert.match(parseLiveCommand('next', null, '', {}).error, /Not connected/)
})

test('start-screen templates all build a valid chart', () => {
  for (const cmd of [
    'chain preshow, intro, build, drop, breakdown, finale; add the standard lanes in drop; set drop length to 30 seconds',
    'chain houseopen, act1, interval, act2, bows',
    'chain idle, attract, interact, cooldown; from cooldown to idle after 10 seconds',
  ]) {
    const r = runCommandText(cmd, newShowchart())
    assert.ok(r.ok, `${cmd}: ${r.error}`)
    assert.ok(r.chart.states.length >= 5 || cmd.includes('idle'))
  }
})
