// Show commands: arranging a showchart from typed or spoken text.
//
//   text ──parse──▶ ShowCommand[] ──apply (on a copy)──▶ new Showchart + summaries
//
// Everything here is pure and offline: no network, no model, no microphone.
// Typed text and voice transcripts take the exact same path. A future LLM
// translator only has to produce ShowCommand[]; applyCommand() is the single
// place that validates and changes a chart.
//
// Importing values from './index' is circular (index re-exports this file);
// that is safe because nothing here runs at import time.

import { guardError, type ShowAction, type ShowContext, type Showchart, type ShowState, type ShowTransition } from './index'

export type ShowCommand =
  | { op: 'clearShow' }
  | { op: 'addState'; id: string; label?: string; ifMissing?: boolean }
  | { op: 'removeState'; id: string }
  | { op: 'renameState'; from: string; to: string }
  | { op: 'labelState'; id: string; label: string }
  | { op: 'setInitial'; id: string }
  | { op: 'addTransition'; source: string; target: string; event?: string; after_ms?: number; guard?: string }
  | { op: 'removeTransition'; source: string; target: string; event?: string }
  | { op: 'addAction'; state: string; when: 'enter' | 'exit'; action: ShowAction }
  | { op: 'clearActions'; state: string; when?: 'enter' | 'exit' }
  | { op: 'setContext'; key: string; value: unknown }
  | { op: 'undo' }
  | { op: 'redo' }
  | { op: 'help' }

export class CommandError extends Error {
  constructor(message: string, public hint?: string) {
    super(message)
  }
}

export interface CommandStep {
  command: ShowCommand
  summary: string
}

export type CommandResult =
  | { ok: true; chart: Showchart; steps: CommandStep[]; notes: string[] }
  | { ok: false; error: string; hint?: string }

/** Phrases the parser understands; the Studio shows these as examples. */
export const COMMAND_EXAMPLES = [
  'chain preshow, intro, drop, breakdown, finale',
  'add state drop',
  'from intro to drop on go',
  'from drop to breakdown after 30 seconds',
  'from intro to drop on go if energy > 0.7',
  'when entering drop set lighting look blue, laser cue l3',
  'when entering intro activate scene intro and audio scene intro stem',
  'rename drop to peak',
  'start at preshow',
  'set energy to 0.8',
  'remove state breakdown',
  'undo',
]

// ───────────────────────── helpers ─────────────────────────

/** State and event ids: UPPER_SNAKE. "pre show" → PRE_SHOW */
const norm = (s: string) =>
  s
    .trim()
    .replace(/^["'“”]+|["'“”]+$/g, '')
    .replace(/[^A-Za-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .toUpperCase()

const squash = (s: string) => norm(s).replace(/_/g, '')

/** Action values: lower snake, matching names like intro_warm. */
const val = (s: string) =>
  s
    .trim()
    .replace(/^["'“”]+|["'“”]+$/g, '')
    .replace(/\s+/g, '_')
    .toLowerCase()

function lev(a: string, b: string): number {
  const d = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)])
  for (let j = 1; j <= b.length; j++) d[0][j] = j
  for (let i = 1; i <= a.length; i++)
    for (let j = 1; j <= b.length; j++)
      d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1))
  return d[a.length][b.length]
}

const WORD_NUM: Record<string, number> = {
  zero: 0, one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7, eight: 8, nine: 9, ten: 10,
  eleven: 11, twelve: 12, fifteen: 15, twenty: 20, thirty: 30, forty: 40, fifty: 50, sixty: 60, ninety: 90,
  a: 1, an: 1, half: 0.5,
}

function duration(num: string, unit: string): number | null {
  const n = /^\d+(\.\d+)?$/.test(num) ? Number(num) : WORD_NUM[num.toLowerCase()]
  if (n === undefined) return null
  const u = unit.toLowerCase()
  const mult = /^(ms|milliseconds?)$/.test(u) ? 1 : /^(m|mins?|minutes?)$/.test(u) ? 60000 : 1000
  return Math.round(n * mult)
}

/**
 * Find a state by what the user said. Exact id, then ignoring spaces and
 * underscores or matching its label, then a unique near match (one or two
 * letters off), which covers speech-recognition slips. Near matches are
 * reported in `notes` so the preview shows what was assumed.
 */
function resolveState(chart: Showchart, raw: string, notes?: string[]): string | null {
  const id = norm(raw)
  if (!id) return null
  if (chart.states.some((s) => s.id === id)) return id
  const sq = squash(raw)
  const same = chart.states.filter((s) => squash(s.id) === sq || (s.label && squash(s.label) === sq))
  if (same.length === 1) {
    notes?.push(`"${raw.trim()}" → ${same[0].id}`)
    return same[0].id
  }
  const maxd = sq.length >= 8 ? 2 : sq.length >= 4 ? 1 : 0
  if (maxd) {
    const scored = chart.states
      .map((s) => ({ id: s.id, d: lev(squash(s.id), sq) }))
      .filter((x) => x.d <= maxd)
      .sort((a, b) => a.d - b.d)
    if (scored.length && (scored.length === 1 || scored[0].d < scored[1].d)) {
      notes?.push(`"${raw.trim()}" → ${scored[0].id} (closest match)`)
      return scored[0].id
    }
  }
  return null
}

const stateList = (c: Showchart) => (c.states.length ? c.states.map((s) => s.id).join(', ') : 'none yet')

/** "energy is above 0.7" → "energy > 0.7"; named guards pass through. */
function normalizeGuard(chart: Showchart, text: string): string {
  const t = text.trim()
  if (chart.guards?.[t]) return t
  return t
    .replace(/\s+(?:is\s+)?(?:above|greater than|more than|over|higher than)\s+/gi, ' > ')
    .replace(/\s+(?:is\s+)?(?:below|less than|under|lower than)\s+/gi, ' < ')
    .replace(/\s+(?:is\s+)?at least\s+/gi, ' >= ')
    .replace(/\s+(?:is\s+)?at most\s+/gi, ' <= ')
    .replace(/\s+(?:is\s+not|isn't|not equal to)\s+/gi, ' != ')
    .replace(/\s+(?:equals|equal to|is)\s+/gi, ' == ')
    .replace(/\s+/g, ' ')
    .trim()
}

const sameT = (a: ShowTransition, b: ShowTransition) =>
  a.source === b.source && a.target === b.target && a.event === b.event && a.guard === b.guard && a.after_ms === b.after_ms

const trigLabel = (t: { event?: string; after_ms?: number }) =>
  t.event ?? (t.after_ms !== undefined ? `after ${t.after_ms / 1000}s` : 'NEXT')

// ───────────────────────── apply ─────────────────────────

/** Applies one command to a copy of the chart. Throws CommandError. */
export function applyCommand(chart: Showchart, cmd: ShowCommand): { chart: Showchart; summary: string } {
  const c: Showchart = { ...chart, states: [...chart.states], transitions: [...chart.transitions] }
  const need = (id: string): ShowState => {
    const s = c.states.find((x) => x.id === id)
    if (!s) throw new CommandError(`There is no state ${id}.`, `States: ${stateList(c)}`)
    return s
  }
  const patchState = (id: string, p: Partial<ShowState>) => {
    c.states = c.states.map((s) => (s.id === id ? { ...s, ...p } : s))
  }

  switch (cmd.op) {
    case 'clearShow':
      return { chart: { ...c, initial: '', states: [], transitions: [] }, summary: 'Start a new, empty show' }

    case 'addState': {
      if (c.states.some((s) => s.id === cmd.id)) {
        if (cmd.ifMissing) return { chart: c, summary: `State ${cmd.id} already exists` }
        throw new CommandError(`State ${cmd.id} already exists.`)
      }
      c.states.push({ id: cmd.id, ...(cmd.label ? { label: cmd.label } : {}) })
      if (!c.states.some((s) => s.id === c.initial)) c.initial = cmd.id // first state becomes the start
      return { chart: c, summary: `Add state ${cmd.id}${cmd.label ? ` ("${cmd.label}")` : ''}` }
    }

    case 'removeState': {
      need(cmd.id)
      const gone = c.transitions.filter((t) => t.source === cmd.id || t.target === cmd.id).length
      c.states = c.states.filter((s) => s.id !== cmd.id)
      c.transitions = c.transitions.filter((t) => t.source !== cmd.id && t.target !== cmd.id)
      if (c.initial === cmd.id) c.initial = c.states[0]?.id ?? ''
      return { chart: c, summary: `Remove state ${cmd.id}${gone ? ` and ${gone} transition${gone > 1 ? 's' : ''}` : ''}` }
    }

    case 'renameState': {
      need(cmd.from)
      if (c.states.some((s) => s.id === cmd.to)) throw new CommandError(`State ${cmd.to} already exists.`)
      patchState(cmd.from, { id: cmd.to })
      c.transitions = c.transitions.map((t) => ({
        ...t,
        source: t.source === cmd.from ? cmd.to : t.source,
        target: t.target === cmd.from ? cmd.to : t.target,
      }))
      if (c.initial === cmd.from) c.initial = cmd.to
      return { chart: c, summary: `Rename ${cmd.from} to ${cmd.to}` }
    }

    case 'labelState':
      need(cmd.id)
      patchState(cmd.id, { label: cmd.label })
      return { chart: c, summary: `Label ${cmd.id} "${cmd.label}"` }

    case 'setInitial':
      need(cmd.id)
      c.initial = cmd.id
      return { chart: c, summary: `Start the show at ${cmd.id}` }

    case 'addTransition': {
      const created: string[] = []
      for (const id of [cmd.source, cmd.target]) {
        if (!c.states.some((s) => s.id === id)) {
          c.states.push({ id })
          created.push(id)
          if (!c.states.some((s) => s.id === c.initial)) c.initial = id
        }
      }
      if (cmd.guard !== undefined) {
        const g = c.guards?.[cmd.guard] ?? cmd.guard
        const err = guardError(g)
        if (err) throw new CommandError(`That condition does not make sense: ${err}`, 'Try: energy > 0.7, or a guard name from the show.')
      }
      const t: ShowTransition = {
        source: cmd.source,
        target: cmd.target,
        ...(cmd.event !== undefined ? { event: cmd.event } : {}),
        ...(cmd.after_ms !== undefined ? { after_ms: cmd.after_ms } : {}),
        ...(cmd.guard !== undefined ? { guard: cmd.guard } : {}),
      }
      if (t.event === undefined && t.after_ms === undefined) t.event = 'NEXT'
      const label = `${cmd.source} —${trigLabel(t)}${t.guard ? ` [${t.guard}]` : ''}→ ${cmd.target}`
      if (c.transitions.some((x) => sameT(x, t))) return { chart: c, summary: `${label} already exists` }
      const trig = t.event ?? `after:${t.after_ms}`
      const clash = c.transitions.find(
        (x) => x.source === t.source && x.guard === undefined && t.guard === undefined && (x.event ?? `after:${x.after_ms}`) === trig,
      )
      if (clash)
        throw new CommandError(
          `${t.source} already goes to ${clash.target} on ${trigLabel(clash)}, so this one could never fire.`,
          'Use a different event, add a condition ("if ..."), or remove the other transition first.',
        )
      c.transitions.push(t)
      return { chart: c, summary: `Add transition ${label}${created.length ? `  (also created ${created.join(', ')})` : ''}` }
    }

    case 'removeTransition': {
      const hit = c.transitions.filter((t) => t.source === cmd.source && t.target === cmd.target && (cmd.event === undefined || t.event === cmd.event))
      if (!hit.length) throw new CommandError(`There is no transition from ${cmd.source} to ${cmd.target}${cmd.event ? ` on ${cmd.event}` : ''}.`)
      c.transitions = c.transitions.filter((t) => !hit.includes(t))
      return { chart: c, summary: `Remove ${hit.length} transition${hit.length > 1 ? 's' : ''} from ${cmd.source} to ${cmd.target}` }
    }

    case 'addAction': {
      const s = need(cmd.state)
      const key = cmd.when === 'enter' ? 'enter_actions' : 'exit_actions'
      const list = s[key] ?? []
      const dup = list.some((a) => a.kind === cmd.action.kind && JSON.stringify(a.payload ?? {}) === JSON.stringify(cmd.action.payload ?? {}))
      if (!dup) patchState(cmd.state, { [key]: [...list, cmd.action] })
      return {
        chart: c,
        summary: `On ${cmd.when === 'enter' ? 'entering' : 'leaving'} ${cmd.state}: ${cmd.action.kind} ${JSON.stringify(cmd.action.payload ?? {})}${dup ? ' (already there)' : ''}`,
      }
    }

    case 'clearActions': {
      need(cmd.state)
      patchState(cmd.state, {
        ...(cmd.when !== 'exit' ? { enter_actions: [] } : {}),
        ...(cmd.when !== 'enter' ? { exit_actions: [] } : {}),
      })
      return { chart: c, summary: `Clear ${cmd.when ?? 'all'} actions of ${cmd.state}` }
    }

    case 'setContext':
      c.context = { ...(c.context ?? {}), [cmd.key]: cmd.value } as ShowContext
      return { chart: c, summary: `Set context ${cmd.key} = ${JSON.stringify(cmd.value)}` }

    case 'undo':
    case 'redo':
    case 'help':
      return { chart: c, summary: cmd.op } // handled by the UI
  }
}

// ───────────────────────── parse ─────────────────────────

const ACTION_VERB = /^(?:set|activate|trigger|fire|load|switch to|launch|run|start|play|go to)\s+/i

/** One spoken/typed action phrase → a PCHI action, or null. */
function parseAction(phrase: string): ShowAction | null {
  const p = phrase.trim().replace(ACTION_VERB, '').trim()
  let m: RegExpMatchArray | null
  if ((m = p.match(/^(?:(?:visuals?|video)\s+)?scene\s+(.+)$/i))) return { kind: 'scene.activate', payload: { scene: val(m[1]) } }
  if ((m = p.match(/^(?:lighting|lights?)\s+(?:look\s+|cue\s+|preset\s+|scene\s+)?(.+)$/i))) return { kind: 'lighting.look', payload: { look: val(m[1]) } }
  if ((m = p.match(/^laser\s+(?:cue\s+|preset\s+|look\s+|scene\s+)?(.+)$/i))) return { kind: 'laser.cue', payload: { cue: val(m[1]) } }
  // PIP capability names: audio.snapshot (desk snapshot) and audio.scene (stem / scene)
  if ((m = p.match(/^(?:audio|sound)\s+snapshot\s+(.+)$/i))) return { kind: 'audio.snapshot', payload: { snapshot: val(m[1]) } }
  if ((m = p.match(/^(?:audio|sound)\s+(?:scene\s+|stem\s+|cue\s+|preset\s+)?(.+)$/i))) return { kind: 'audio.scene', payload: { scene: val(m[1]) } }
  if ((m = p.match(/^playlist\s+(.+)$/i))) return { kind: 'playlist.play', payload: { playlist: val(m[1]) } }
  if ((m = p.match(/^output\s+(.+)$/i))) return { kind: 'output.activate', payload: { target: val(m[1]) } }
  if ((m = p.match(/^cue\s+(.+)$/i))) return { kind: 'cue.trigger', payload: { cue: val(m[1]) } }
  if ((m = p.match(/^(?:send|emit)\s+(?:event\s+)?([a-z][\w-]*(?:\.[\w-]+)+)((?:\s+\w+=\S+)*)$/i))) {
    const payload: Record<string, unknown> = {}
    for (const kv of m[2].trim().split(/\s+/).filter(Boolean)) {
      const [k, ...v] = kv.split('=')
      const raw = v.join('=')
      try {
        payload[k] = JSON.parse(raw)
      } catch {
        payload[k] = raw
      }
    }
    return { kind: m[1].toLowerCase(), payload }
  }
  return null
}

const ACTION_BOUNDARY =
  /\b(?:activate|set|trigger|fire|load|play|start|launch|run|cue|lighting|lights?|laser|audio|sound|visuals?|video|scene|playlist|output|send|emit|switch)\b/i

/** "drop set lighting look blue, laser cue l3" → state text + action phrases */
function splitStateAndActions(rest: string): { stateText: string; phrases: string[] } | null {
  const m = ACTION_BOUNDARY.exec(rest)
  if (!m || m.index === 0) return null
  const stateText = rest.slice(0, m.index).replace(/[,:]\s*$|\s+(?:then|do|and)\s*$/i, '').trim()
  const phrases = rest
    .slice(m.index)
    .split(/\s*(?:,|\band\b|\bthen\b|\bplus\b)\s*/i)
    .map((s) => s.trim())
    .filter(Boolean)
  return stateText ? { stateText, phrases } : null
}

const FRAGMENT_START = 'add|create|new|make|remove|delete|rename|label|title|connect|link|disconnect|unlink|set|start|begin|when|on|from|clear|chain|sequence|undo|redo|help|entering|leaving'

function splitFragments(text: string): string[] {
  return text
    .split(/\s*(?:;|\n|\.\s+)\s*/)
    .flatMap((p) => p.split(new RegExp(`\\s+(?:and then|then)\\s+(?=(?:${FRAGMENT_START})\\b)`, 'i')))
    .map((s) => s.trim().replace(/[.!?]+$/, ''))
    .filter(Boolean)
}

const DEPTS = /^(?:scene|lighting|lights?|laser|audio|sound|playlist|output|initial|start|cue)\b/i

class Unparsed extends Error {}

/** Parse one fragment against `chart` (for name resolution). */
function parseFragment(text: string, chart: Showchart, notes: string[]): ShowCommand[] {
  let m: RegExpMatchArray | null
  const st = (raw: string) => {
    const id = resolveState(chart, raw, notes)
    if (!id) throw new CommandError(`I don't know a state called "${raw.trim()}".`, `States: ${stateList(chart)}`)
    return id
  }

  if (/^undo$/i.test(text)) return [{ op: 'undo' }]
  if (/^redo$/i.test(text)) return [{ op: 'redo' }]
  if (/^(?:help|what can i say|commands|examples)\??$/i.test(text)) return [{ op: 'help' }]
  if (/^(?:new|clear|reset|empty)\s+(?:the\s+)?show$/i.test(text)) return [{ op: 'clearShow' }]

  // chain preshow, intro, drop → states + NEXT transitions
  if ((m = text.match(/^(?:chain|sequence|flow|arrange|build|show)(?:\s+(?:of|with|from))?\s*:?\s+(.+)$/i))) {
    const names = m[1].split(/\s*(?:,|->|→|\bthen\b|\bto\b|\band\b)\s*/i).map((s) => s.trim()).filter(Boolean)
    if (names.length >= 2) {
      const cmds: ShowCommand[] = []
      const ids = names.map((n) => resolveState(chart, n, notes) ?? norm(n))
      ids.forEach((id) => cmds.push({ op: 'addState', id, ifMissing: true }))
      for (let i = 0; i < ids.length - 1; i++) cmds.push({ op: 'addTransition', source: ids[i], target: ids[i + 1], event: 'NEXT' })
      return cmds
    }
  }

  // label X as Y (before rename: keeps the user's casing for the label)
  if ((m = text.match(/^(?:label|title|caption|call)(?:\s+state)?\s+(.+?)\s+(?:as|to)\s+(.+)$/i)))
    return [{ op: 'labelState', id: st(m[1]), label: m[2].trim().replace(/^["'“”]+|["'“”]+$/g, '') }]

  if ((m = text.match(/^rename(?:\s+state)?\s+(.+?)\s+to\s+(.+)$/i))) return [{ op: 'renameState', from: st(m[1]), to: norm(m[2]) }]

  // initial state
  if ((m = text.match(/^(?:start|begin)\s+(?:at|in|with|from)\s+(.+)$/i)) || (m = text.match(/^(?:set\s+)?(?:the\s+)?(?:initial|starting|first)\s+state\s+(?:to|is|=)?\s*(.+)$/i)))
    return [{ op: 'setInitial', id: st(m[1]) }]
  if ((m = text.match(/^(?:make|set)\s+(.+?)\s+(?:as\s+)?(?:the\s+)?(?:initial|starting|first|start)(?:\s+state)?$/i))) return [{ op: 'setInitial', id: st(m[1]) }]

  // add / remove state
  if (
    (m = text.match(/^(?:add|create|new|make)\s+(?:a\s+|an\s+|the\s+)?(?:new\s+)?state(?:\s+(?:called|named))?\s+(.+?)(?:\s+(?:labell?ed|with label|titled)\s+(.+))?$/i)) ||
    (m = text.match(/^(?:add|create|new|make)\s+(?:a\s+|an\s+|the\s+)?(?:new\s+)?(.+?)\s+state(?:\s+(?:labell?ed|with label|titled)\s+(.+))?$/i))
  ) {
    const id = norm(m[1])
    if (!id) throw new Unparsed()
    return [{ op: 'addState', id, ...(m[2] ? { label: m[2].trim().replace(/^["'“”]+|["'“”]+$/g, '') } : {}) }]
  }
  if ((m = text.match(/^(?:remove|delete|erase)\s+(?:the\s+)?state\s+(.+)$/i)) || (m = text.match(/^(?:remove|delete|erase)\s+(?:the\s+)?(.+?)\s+state$/i)))
    return [{ op: 'removeState', id: st(m[1]) }]

  // clear actions
  if ((m = text.match(/^(?:clear|remove|delete)\s+(?:all\s+)?(?:(enter|exit|entry)\s+)?actions?\s+(?:of|for|on|from|in)\s+(.+)$/i)))
    return [{ op: 'clearActions', state: st(m[2]), ...(m[1] ? { when: m[1].toLowerCase() === 'exit' ? 'exit' : 'enter' } : {}) as { when?: 'enter' | 'exit' } }]

  // remove transition
  if ((m = text.match(/^(?:remove|delete|disconnect|unlink)\s+(?:the\s+)?(?:transition\s+)?(?:from\s+)?(.+?)\s+(?:to|->|→)\s+(.+?)(?:\s+on\s+(?:event\s+)?(.+))?$/i)))
    return [{ op: 'removeTransition', source: st(m[1]), target: st(m[2]), ...(m[3] ? { event: norm(m[3]) } : {}) }]

  // actions: "when entering X ..., leaving X ..., in X ..., X: ..."
  const actionClause = (kind: 'enter' | 'exit', rest: string): ShowCommand[] => {
    // Prefer the longest leading words that name a real state; the remainder is the action text.
    // Falls back to cutting at the first action keyword (so unknown states still get a clear error).
    let sp: { stateText: string; phrases: string[] } | null = null
    const words = rest.trim().split(/\s+/)
    for (let n = words.length - 1; n >= 1 && !sp; n--) {
      const cand = words.slice(0, n).join(' ').replace(/[,:]$/, '')
      if (resolveState(chart, cand)) {
        const tail = words.slice(n).join(' ').replace(/^(?:then|do|and)\s+/i, '')
        sp = { stateText: cand, phrases: tail.split(/\s*(?:,|\band\b|\bthen\b|\bplus\b)\s*/i).map((s) => s.trim()).filter(Boolean) }
      }
    }
    sp = sp ?? splitStateAndActions(rest)
    if (!sp) throw new CommandError(`I couldn't find an action after the state name in "${text}".`, 'Try: when entering drop set lighting look blue')
    const state = st(sp.stateText)
    const cmds: ShowCommand[] = []
    for (const ph of sp.phrases) {
      const action = parseAction(ph)
      if (!action) throw new CommandError(`I don't know the action "${ph}".`, 'Say things like: scene X, lighting look X, laser cue X, audio scene X, playlist X, output X, cue X')
      cmds.push({ op: 'addAction', state, when: kind, action })
    }
    return cmds
  }
  if ((m = text.match(/^(?:(?:when|on|upon|as soon as)\s+)?(?:entering|enter|enters|arriving at|going into)\s+(.+)$/i))) return actionClause('enter', m[1])
  if ((m = text.match(/^(?:(?:when|on|upon)\s+)?(?:leaving|exiting|exit|leave|leaves)\s+(.+)$/i))) return actionClause('exit', m[1])
  if ((m = text.match(/^(?:in|during|at)\s+(.+)$/i)) && splitStateAndActions(m[1]) && resolveState(chart, splitStateAndActions(m[1])!.stateText))
    return actionClause('enter', m[1])
  if ((m = text.match(/^(.+?)\s*:\s*(.+)$/)) && resolveState(chart, m[1]) && parseAction(m[2].split(/\s*(?:,|\band\b)\s*/i)[0]))
    return actionClause('enter', `${m[1]} ${m[2]}`)

  // transitions
  const tr = parseTransition(text, chart, notes)
  if (tr) return [tr]

  // context: "set energy to 0.8"
  if ((m = text.match(/^set\s+([\w.]+)\s+(?:to|=|as)\s+(.+)$/i)) && !DEPTS.test(m[1])) {
    let value: unknown = m[2].trim()
    try {
      value = JSON.parse(m[2].trim().toLowerCase())
    } catch {
      /* plain text */
    }
    return [{ op: 'setContext', key: m[1], value }]
  }

  throw new Unparsed()
}

function parseTransition(text: string, chart: Showchart, notes: string[]): ShowCommand | null {
  let rest = text
  let guard: string | undefined
  let after_ms: number | undefined
  let event: string | undefined

  const gm = rest.match(/\s+(?:only\s+)?if\s+(.+)$/i)
  if (gm && gm.index !== undefined) {
    guard = normalizeGuard(chart, gm[1])
    rest = rest.slice(0, gm.index)
  }
  const dm = rest.match(/\s+after\s+(\S+)\s*(ms|milliseconds?|s|secs?|seconds?|m|mins?|minutes?)$/i)
  if (dm && dm.index !== undefined) {
    const ms = duration(dm[1], dm[2])
    if (ms === null) throw new CommandError(`I couldn't read the time "${dm[1]} ${dm[2]}".`, 'Say a number: after 30 seconds')
    after_ms = ms
    rest = rest.slice(0, dm.index)
  }

  let m: RegExpMatchArray | null
  // "on GO in INTRO go to DROP"
  if ((m = rest.match(/^(?:when|on|upon)\s+(?:event\s+)?(.+?)\s+(?:in|from|at|during)\s+(.+?),?\s+(?:go|goto|move|jump|switch|transition)\s+(?:to\s+)?(.+)$/i))) {
    return mk(m[2], m[3], m[1])
  }
  // trailing "on EVENT"
  const toks = rest.split(/\s+/)
  const i = toks.map((t) => t.toLowerCase()).lastIndexOf('on')
  const j = toks.map((t) => t.toLowerCase()).lastIndexOf('via')
  const k = Math.max(i, j)
  if (k > 0 && k < toks.length - 1 && /(?:\bto\b|->|→|goes|go|leads|connect|link)/i.test(toks.slice(0, k).join(' '))) {
    let ev = toks.slice(k + 1).join(' ')
    ev = ev.replace(/^event\s+/i, '')
    event = ev
    rest = toks.slice(0, k).join(' ')
  }

  if (
    (m = rest.match(/^(?:add\s+|create\s+|make\s+)?(?:a\s+)?(?:new\s+)?(?:transition\s+|link\s+|connection\s+)?from\s+(.+?)\s+(?:to|->|→)\s+(.+)$/i)) ||
    (m = rest.match(/^(?:connect|link)\s+(.+?)\s+(?:to|with|and)\s+(.+)$/i)) ||
    (m = rest.match(/^(.+?)\s+(?:goes|go|moves|leads|jumps)\s+to\s+(.+)$/i)) ||
    (m = rest.match(/^(.+?)\s*(?:->|→)\s*(.+)$/))
  ) {
    return mk(m[1], m[2], event)
  }
  return null

  function mk(src: string, dst: string, ev?: string): ShowCommand {
    const source = resolveState(chart, src, notes) ?? norm(src)
    const target = resolveState(chart, dst, notes) ?? norm(dst)
    if (!source || !target) throw new Unparsed()
    const e = ev ?? event
    return {
      op: 'addTransition',
      source,
      target,
      ...(e ? { event: norm(e) } : {}),
      ...(after_ms !== undefined ? { after_ms } : {}),
      ...(guard !== undefined ? { guard } : {}),
    }
  }
}

/**
 * Text → a validated, previewable change. Nothing is applied to `chart`; the
 * result carries the new chart and a step-by-step summary. All-or-nothing:
 * if any part fails, nothing changes.
 */
export function runCommandText(text: string, chart: Showchart): CommandResult {
  const fragments = splitFragments(text)
  if (!fragments.length) return { ok: false, error: 'Type or say a command.', hint: COMMAND_EXAMPLES[0] }
  const notes: string[] = []
  const steps: CommandStep[] = []
  let working = chart
  try {
    for (const frag of fragments) {
      let cmds: ShowCommand[]
      try {
        cmds = parseFragment(frag, working, notes)
      } catch (e) {
        if (e instanceof Unparsed) throw new CommandError(`I didn't understand "${frag}".`, `Try something like: ${COMMAND_EXAMPLES[1]}  ·  ${COMMAND_EXAMPLES[2]}  ·  say "help" for more`)
        throw e
      }
      for (const command of cmds) {
        const r = applyCommand(working, command)
        working = r.chart
        steps.push({ command, summary: r.summary })
      }
    }
  } catch (e) {
    if (e instanceof CommandError) return { ok: false, error: e.message, hint: e.hint }
    throw e
  }
  return { ok: true, chart: working, steps, notes }
}
