// Commands for a RUNNING show ("go to drop", "restart", "set energy to 0.8").
// Unlike the editing commands these never change the chart: they only produce
// the one input the runtime should receive. The runtime still decides whether
// it is allowed, so a typo cannot do more than send an event it would accept.
import { step, type Showchart, type ShowContext } from './index'

export type LiveAction =
  | { kind: 'event'; event: string }
  | { kind: 'reset' }
  | { kind: 'context'; key: string; value: unknown }

export type LiveResult =
  | { ok: true; action: LiveAction; summary: string; notes: string[] }
  | { ok: false; error: string; hint?: string }

export const LIVE_EXAMPLES = ['next', 'go to drop', 'restart the show', 'set energy to 0.8', 'send go']

const squash = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '')

function lev(a: string, b: string): number {
  const d = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)])
  for (let j = 1; j <= b.length; j++) d[0][j] = j
  for (let i = 1; i <= a.length; i++)
    for (let j = 1; j <= b.length; j++) d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1))
  return d[a.length][b.length]
}

/** Event transitions out of `state` that would actually fire right now (guards satisfied). */
function firable(def: Showchart, ctx: ShowContext, state: string) {
  return def.transitions.filter((t) => t.source === state && t.event !== undefined && step(def, ctx, state, t.event)?.target === t.target)
}

function matchState(def: Showchart, raw: string, notes: string[]): string | null {
  const sq = squash(raw)
  if (!sq) return null
  const hit = def.states.find((s) => squash(s.id) === sq || (s.label && squash(s.label) === sq))
  if (hit) return hit.id
  const maxd = sq.length >= 8 ? 2 : sq.length >= 4 ? 1 : 0
  const scored = def.states
    .map((s) => ({ id: s.id, d: lev(squash(s.id), sq) }))
    .filter((x) => x.d <= maxd)
    .sort((a, b) => a.d - b.d)
  if (maxd && scored.length && (scored.length === 1 || scored[0].d < scored[1].d)) {
    notes.push(`"${raw.trim()}" → ${scored[0].id} (closest match)`)
    return scored[0].id
  }
  return null
}

function parseValue(raw: string): unknown {
  const t = raw.trim()
  if (/^(true|on|yes)$/i.test(t)) return true
  if (/^(false|off|no)$/i.test(t)) return false
  if (t !== '' && !Number.isNaN(Number(t))) return Number(t)
  return t.replace(/^["“]|["”]$/g, '')
}

export function parseLiveCommand(text: string, def: Showchart | null, state: string, ctx: ShowContext): LiveResult {
  if (!def || !state) return { ok: false, error: 'Not connected to a running show yet.', hint: 'Start the runtime: pchi-studio-api <show.pchi.json>' }
  const t = text.trim().replace(/[.!?]+$/, '')
  const notes: string[] = []
  if (!t) return { ok: false, error: 'Type what the show should do.', hint: LIVE_EXAMPLES.join(' · ') }

  if (/^(restart|reset)( the)?( show)?$/i.test(t)) return { ok: true, action: { kind: 'reset' }, summary: 'Restart the show from the beginning', notes }

  const set = /^(?:set|make)\s+([a-z_][\w.-]*)\s+(?:to|=|is)\s+(.+)$/i.exec(t)
  if (set) {
    const value = parseValue(set[2])
    return { ok: true, action: { kind: 'context', key: set[1], value }, summary: `Set ${set[1]} to ${JSON.stringify(value)}`, notes }
  }

  const ready = firable(def, ctx, state)

  if (/^(next|go|continue|advance|go on|next (?:state|section|scene))$/i.test(t)) {
    if (ready.length === 1) return { ok: true, action: { kind: 'event', event: ready[0].event! }, summary: `Send ${ready[0].event} → ${ready[0].target}`, notes }
    if (ready.length === 0) return { ok: false, error: `Nothing can move the show on from ${state} by hand.`, hint: timed(def, state) }
    return { ok: false, error: `${ready.length} ways out of ${state}: ${ready.map((r) => `${r.event} → ${r.target}`).join(', ')}.`, hint: 'Say "go to <state>" to choose.' }
  }

  const go = /^(?:go|jump|skip|move|cut|switch|back|return)(?:\s+(?:back|forward|ahead))?\s*(?:to|into)\s+(?:the\s+)?(.+)$/i.exec(t) ?? /^(?:to)\s+(.+)$/i.exec(t)
  if (go) {
    const target = matchState(def, go[1], notes)
    if (!target) return { ok: false, error: `There is no state called "${go[1].trim()}".`, hint: `States: ${def.states.map((s) => s.id).join(', ')}` }
    if (target === state) return { ok: false, error: `Already in ${state}.` }
    const via = ready.filter((r) => r.target === target)
    if (via.length >= 1) return { ok: true, action: { kind: 'event', event: via[0].event! }, summary: `Send ${via[0].event} → ${target}`, notes }
    const blocked = def.transitions.some((x) => x.source === state && x.target === target && x.event !== undefined)
    return {
      ok: false,
      error: blocked ? `${state} → ${target} is blocked right now (its guard is not satisfied).` : `There is no way to go from ${state} straight to ${target}.`,
      hint: ready.length ? `From ${state} you can go to: ${[...new Set(ready.map((r) => r.target))].join(', ')}` : timed(def, state),
    }
  }

  // "send go", or just the event's own name
  const ev = /^(?:send|fire|trigger|input)\s+(.+)$/i.exec(t)?.[1] ?? t
  const byName = ready.find((r) => squash(r.event!) === squash(ev))
  if (byName) return { ok: true, action: { kind: 'event', event: byName.event! }, summary: `Send ${byName.event} → ${byName.target}`, notes }

  return { ok: false, error: `I did not understand "${t}".`, hint: ready.length ? `Right now you can say: ${ready.map((r) => `"go to ${r.target}"`).join(', ')}, "restart", "set <key> to <value>"` : LIVE_EXAMPLES.join(' · ') }
}

function timed(def: Showchart, state: string): string {
  const t = def.transitions.filter((x) => x.source === state && x.after_ms !== undefined)
  return t.length ? `It moves on by itself: ${t.map((x) => `after ${x.after_ms! / 1000} s → ${x.target}`).join(', ')}` : 'No transitions leave this state.'
}
