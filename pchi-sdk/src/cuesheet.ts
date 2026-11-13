// The cue sheet: every action the show performs, listed in the order a stage
// manager calls it. It is a VIEW of the showchart, never a second copy: edits
// go back into the chart through the helpers here.
import { deptOf, transitionLabel, type ShowAction, type Showchart, type ShowCue } from './index'

export type CueSource = 'enter' | 'cue' | 'exit'

export interface CueRow {
  /** The cue number (Q). Assigned once and kept: it never changes when cues are added, moved or deleted. Null for enter/exit actions, which are not called. */
  num: number | null
  scene: string
  source: CueSource
  /** Seconds-from-scene-start in ms. Enter actions are 0; exit actions have no time. */
  at_ms: number | null
  length_ms?: number
  laneId?: string
  laneName: string
  dept: string
  cueId?: string
  label: string
  kind: string
  /** The action's main value (scene name, look, cue id...), for display and quick edit. */
  value: string
  field: string
}

export interface SceneBlock {
  id: string
  label?: string
  /** How the show leaves this scene: "NEXT → DROP", "after 12 s → PORTAL". */
  leaves: string[]
  rows: CueRow[]
}

/** Scenes in the order the show runs them: from the start along the transitions, then anything unreachable. */
export function sceneOrder(chart: Showchart): string[] {
  const seen: string[] = []
  const visit = (id: string) => {
    if (seen.includes(id) || !chart.states.some((s) => s.id === id)) return
    seen.push(id)
    chart.transitions.filter((t) => t.source === id).forEach((t) => visit(t.target))
  }
  if (chart.initial) visit(chart.initial)
  chart.states.forEach((s) => visit(s.id))
  return seen
}

const firstValue = (a: ShowAction): { field: string; value: string } => {
  const p = a.payload ?? {}
  const keys = Object.keys(p)
  const field = keys[0] ?? ''
  return { field, value: field && p[field] !== undefined && p[field] !== null ? String(p[field]) : '' }
}

export function buildCueSheet(chart: Showchart): SceneBlock[] {
  return sceneOrder(chart).map((id) => {
    const st = chart.states.find((s) => s.id === id)!
    const rows: CueRow[] = []
    const act = (source: CueSource, a: ShowAction, at: number | null) => {
      const { field, value } = firstValue(a)
      return { num: null, scene: id, source, at_ms: at, laneName: source === 'cue' ? '' : source === 'enter' ? 'on enter' : 'on leave', dept: deptFor(a.kind), label: '', kind: a.kind, value, field } as CueRow
    }
    for (const a of st.enter_actions ?? []) rows.push(act('enter', a, 0))
    const lanes = st.timeline?.lanes ?? []
    const cues = lanes
      .flatMap((l, li) => l.cues.map((c) => ({ l, li, c })))
      .sort((a, b) => a.c.at_ms - b.c.at_ms || a.li - b.li)
    for (const { l, c } of cues) {
      const r = act('cue', c.action, c.at_ms)
      r.laneId = l.id
      r.laneName = l.name || l.id
      r.dept = l.dept ?? deptFor(c.action.kind)
      r.cueId = c.id
      r.num = c.num ?? null
      r.label = c.label ?? ''
      r.length_ms = c.length_ms
      rows.push(r)
    }
    for (const a of st.exit_actions ?? []) rows.push(act('exit', a, null))
    const leaves = chart.transitions
      .filter((t) => t.source === id && t.target !== id)
      .map((t) => `${t.after_ms !== undefined ? `after ${t.after_ms / 1000} s` : transitionLabel(t)} → ${t.target}`)
    return { id, label: st.label, leaves, rows }
  })
}

/** Which department an action kind belongs to (so enter/exit actions get a colour too). */
function deptFor(kind: string): string {
  if (/^(scene|renderer|visual)/.test(kind)) return 'visual'
  if (/^lighting/.test(kind)) return 'lighting'
  if (/^laser/.test(kind)) return 'laser'
  if (/^audio/.test(kind)) return 'audio'
  return 'other'
}

type CuePatch = { at_ms?: number; length_ms?: number | undefined; label?: string; value?: string; num?: number }

/** A new chart with one cue changed. Unknown scene/cue returns the chart untouched. */
export function updateCue(chart: Showchart, scene: string, cueId: string, patch: CuePatch): Showchart {
  return {
    ...chart,
    states: chart.states.map((s) => {
      if (s.id !== scene || !s.timeline) return s
      return {
        ...s,
        timeline: {
          ...s.timeline,
          lanes: s.timeline.lanes.map((l) => {
            if (!l.cues.some((c) => c.id === cueId)) return l
            return {
              ...l,
              cues: l.cues.map((c): ShowCue => {
                if (c.id !== cueId) return c
                const next: ShowCue = { ...c }
                if (patch.at_ms !== undefined) next.at_ms = Math.max(0, Math.round(patch.at_ms))
                if ('length_ms' in patch) {
                  if (patch.length_ms === undefined || patch.length_ms <= 0) delete next.length_ms
                  else next.length_ms = Math.round(patch.length_ms)
                }
                if (patch.num !== undefined && Number.isFinite(patch.num) && patch.num > 0) next.num = Math.round(patch.num * 1000) / 1000
                if (patch.label !== undefined) {
                  if (patch.label.trim() === '') delete next.label
                  else next.label = patch.label
                }
                if (patch.value !== undefined) {
                  const field = Object.keys(c.action.payload ?? {})[0] ?? deptOf(l.dept).field
                  next.action = { ...c.action, payload: { ...(c.action.payload ?? {}), [field]: patch.value } }
                }
                return next
              }),
            }
          }),
        },
      }
    }),
  }
}

export function removeCue(chart: Showchart, scene: string, cueId: string): Showchart {
  return {
    ...chart,
    states: chart.states.map((s) =>
      s.id !== scene || !s.timeline ? s : { ...s, timeline: { ...s.timeline, lanes: s.timeline.lanes.map((l) => ({ ...l, cues: l.cues.filter((c) => c.id !== cueId) })) } },
    ),
  }
}

const fmtTime = (ms: number | null) => (ms === null ? '' : `${Math.floor(ms / 60000)}:${((ms % 60000) / 1000).toFixed(1).padStart(4, '0')}`)
export const cueTime = fmtTime

/** Spreadsheet-safe cell: quotes doubled, and a leading = + - @ neutralised so a cue named "=1+1" cannot run as a formula. */
const cell = (v: unknown) => {
  let s = String(v ?? '')
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s
}

export function cueSheetCsv(chart: Showchart): string {
  const head = ['Q', 'Scene', 'Time', 'When', 'Department', 'Lane', 'Action', 'Value', 'Length (s)', 'Note']
  const lines = [head.join(',')]
  for (const sc of buildCueSheet(chart)) {
    for (const r of sc.rows) {
      lines.push(
        [
          r.num === null ? '' : `Q${r.num}`,
          r.scene,
          fmtTime(r.at_ms),
          r.source === 'cue' ? 'in scene' : r.source === 'enter' ? 'on enter' : 'on leave',
          deptOf(r.dept).label,
          r.laneName,
          r.kind,
          r.value,
          r.length_ms ? r.length_ms / 1000 : '',
          r.label,
        ]
          .map(cell)
          .join(','),
      )
    }
  }
  return lines.join('\r\n') + '\r\n'
}

/**
 * Give every cue that has no number the next free one, in the order the show
 * runs. Numbers already there are never touched, and a number from a deleted
 * cue is not reused (the highest ever handed out is kept in `cue_seq`).
 * Returns the same object when there is nothing to do, so it is safe to call on every change.
 */
export function ensureCueNumbers(chart: Showchart): Showchart {
  const order = sceneOrder(chart)
  let top = chart.cue_seq ?? 0
  for (const s of chart.states) for (const l of s.timeline?.lanes ?? []) for (const c of l.cues) if (c.num !== undefined && c.num > top) top = Math.floor(c.num)
  const missing = chart.states.some((s) => (s.timeline?.lanes ?? []).some((l) => l.cues.some((c) => c.num === undefined)))
  if (!missing) return top === (chart.cue_seq ?? 0) || (chart.cue_seq ?? 0) > top ? chart : { ...chart, cue_seq: top }
  const given = new Map<string, number>() // scene/cue id -> new number
  for (const id of order) {
    const st = chart.states.find((s) => s.id === id)!
    const todo = (st.timeline?.lanes ?? [])
      .flatMap((l, li) => l.cues.map((c) => ({ c, li, key: `${id}/${l.id}/${c.id}` })))
      .filter((x) => x.c.num === undefined)
      .sort((a, b) => a.c.at_ms - b.c.at_ms || a.li - b.li)
    for (const x of todo) given.set(x.key, ++top)
  }
  return {
    ...chart,
    cue_seq: top,
    states: chart.states.map((s) =>
      !s.timeline ? s : { ...s, timeline: { ...s.timeline, lanes: s.timeline.lanes.map((l) => ({ ...l, cues: l.cues.map((c) => (c.num === undefined ? { ...c, num: given.get(`${s.id}/${l.id}/${c.id}`) } : c)) })) } },
    ),
  }
}

/** Cue numbers used by more than one cue (a call sheet should not have two Q12s). */
export function duplicateCueNumbers(chart: Showchart): Set<number> {
  const seen = new Set<number>()
  const dup = new Set<number>()
  for (const s of chart.states) for (const l of s.timeline?.lanes ?? []) for (const c of l.cues) if (c.num !== undefined) (seen.has(c.num) ? dup : seen).add(c.num)
  return dup
}
