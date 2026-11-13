// "Did you mean ...": when a command is not understood, offer the closest thing that is,
// using the names in THIS show (its scenes and rig), so the suggestion can be applied as it stands.
import type { PchiNode, Showchart } from './index'

const words = (s: string) => s.toLowerCase().split(/[^a-z0-9.]+/).filter((w) => w.length > 1)

/** One typo apart (a missing, extra or wrong letter): "ad" for "add", "scen" for "scene". */
const near = (a: string, b: string) => {
  if (a === b) return true
  if (Math.min(a.length, b.length) < 3 && Math.abs(a.length - b.length) > 0 && Math.min(a.length, b.length) < 2) return false
  if (Math.abs(a.length - b.length) > 1) return false
  const [s, l] = a.length <= b.length ? [a, b] : [b, a]
  let i = 0
  let j = 0
  let edits = 0
  while (i < s.length && j < l.length) {
    if (s[i] === l[j]) {
      i++
      j++
    } else if (++edits > 1) return false
    else if (s.length === l.length) {
      i++
      j++
    } else j++
  }
  return edits + (l.length - j) + (s.length - i) <= 1
}

/** Commands that work for this show, with its real names filled in. */
export function exampleCommands(chart: Showchart, nodes: PchiNode[] = []): string[] {
  const a = chart.states[0]?.id.toLowerCase() ?? 'intro'
  const b = chart.states[1]?.id.toLowerCase() ?? 'drop'
  const first = chart.states[0]?.id.toLowerCase() ?? 'intro'
  const list = [
    'add scene drop',
    'add scenes intro, drop, finale',
    `from ${a} to ${b} on go`,
    `after 30 seconds go to ${b}`,
    `at 4 seconds lighting look blue in ${first}`,
    `at 0 seconds audio snapshot 1 in ${first}`,
    `rename ${a} to opener`,
    `delete scene ${b}`,
    `duplicate scene ${a}`,
    'add qlab',
    'use FOH desk for audio',
    'add x32 called FOH desk',
    'add resolume at 192.168.1.20',
    'add grandma3',
    'add lasers',
  ]
  for (const n of nodes.slice(0, 3)) list.push(`set ${n.label.toLowerCase()} host to 192.168.1.20`, `remove ${n.label.toLowerCase()}`)
  return list
}

/**
 * The closest working command to what was typed, or null if nothing is close.
 * Matching is by shared words, with the leading verb counting extra.
 */
export function suggestCommand(text: string, chart: Showchart, nodes: PchiNode[] = []): string | null {
  const w = new Set(words(text))
  if (w.size === 0) return null
  let best: { ex: string; score: number } | null = null
  for (const ex of exampleCommands(chart, nodes)) {
    const ew = words(ex)
    const hit = (x: string) => w.has(x) || [...w].some((y) => x.length >= 3 && y.length >= 2 && near(x, y))
    let score = ew.filter(hit).length
    if (ew[0] && hit(ew[0])) score += 1
    if (score > (best?.score ?? 0)) best = { ex, score }
  }
  return best && best.score >= 2 ? best.ex : null
}

/** The hint under an error: the closest working command, else a few starting points. */
export function hintFor(text: string, chart: Showchart, nodes: PchiNode[] = []): string {
  const near = suggestCommand(text, chart, nodes)
  const tail = 'Or say "help" for more.'
  if (near) return `Did you mean: ${near}  ·  ${tail}`
  return `Try: add scene drop  ·  add qlab  ·  after 30 seconds go to finale  ·  ${tail}`
}
