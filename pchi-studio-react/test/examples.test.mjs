// The stage-show examples in tools/examples must stay valid as the checks grow. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { validateShow, ensureCueNumbers, rigCheck } = sdk
const dir = join(dirname(fileURLToPath(import.meta.url)), '../../tools/examples')
const files = readdirSync(dir).filter((f) => f.endsWith('.pchi'))

test('the three examples are there', () => {
  assert.deepEqual(files.sort(), ['arena-pop-show.pchi', 'stadium-pop-tour.pchi', 'stadium-preshow-doors.pchi'])
})

for (const f of files) {
  const show = JSON.parse(readFileSync(join(dir, f), 'utf8'))
  const chart = show.showchart
  const cues = chart.states.flatMap((s) => (s.timeline?.lanes ?? []).flatMap((l) => l.cues.map((c) => ({ s, l, c }))))

  test(`${f}: no problems the editor can find, and nothing unreachable`, () => {
    assert.deepEqual(validateShow(chart, show.nodes).map((v) => `${v.level}: ${v.text}`), [])
    assert.deepEqual(rigCheck(chart, show.nodes), [])
  })

  test(`${f}: every cue is inside its scene, every lane names a device, cue numbers are unique`, () => {
    for (const { s, l, c } of cues) {
      assert.ok(c.at_ms >= 0 && c.at_ms <= (s.timeline.duration_ms ?? Infinity), `${s.id}/${l.id}: a cue at ${c.at_ms} ms is outside the scene`)
      assert.ok(l.device && show.nodes.some((n) => n.id === l.device), `${s.id}/${l.id} has no device`)
    }
    const nums = cues.map(({ c }) => c.num)
    assert.ok(nums.every((n) => typeof n === 'number'), 'every cue is numbered')
    assert.equal(new Set(nums).size, nums.length, 'cue numbers are unique')
    assert.equal(ensureCueNumbers(chart), chart, 'numbering is already complete')
  })

  test(`${f}: it can be played from start to END with the transitions it has`, () => {
    const out = new Map()
    for (const t of chart.transitions) out.set(t.source, (out.get(t.source) ?? []).concat(t.target))
    const seen = new Set()
    for (let at = chart.initial; at && !seen.has(at); at = (out.get(at) ?? [])[0]) seen.add(at)
    const last = [...seen].pop()
    assert.ok(!out.has(last), `the run stops at ${last}, which has no way out`)
    assert.equal(seen.size, chart.states.length, 'one run from the start visits every scene')
  })

  test(`${f}: nothing that looks like a secret`, () => {
    assert.ok(!/pass(word)?"|secret|token"|api[_-]?key/i.test(readFileSync(join(dir, f), 'utf8')))
  })
}

test('the stadium show is stadium scale and the arena show is smaller', () => {
  const load = (f) => JSON.parse(readFileSync(join(dir, f), 'utf8'))
  const count = (s) => s.showchart.states.reduce((n, st) => n + (st.timeline?.lanes ?? []).reduce((m, l) => m + l.cues.length, 0), 0)
  const [stadium, arena] = [load('stadium-pop-tour.pchi'), load('arena-pop-show.pchi')]
  assert.ok(stadium.showchart.states.length >= 20 && count(stadium) >= 150 && stadium.nodes.length === 6)
  assert.ok(arena.showchart.states.length < stadium.showchart.states.length && count(arena) < count(stadium) && arena.nodes.length < stadium.nodes.length)
  // the stadium show has the moving stage on a motion lane, with only programs its device allows (validateShow checks that)
  assert.ok(stadium.showchart.states.some((s) => (s.timeline?.lanes ?? []).some((l) => l.dept === 'motion')))
})
