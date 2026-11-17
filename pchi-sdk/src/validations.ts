// Everything that can be checked about a show while it is being edited, in one list with a place on each line.
// Pure functions, no runtime needed: the runtime's own doors check (preflight) goes further, but these are the
// things Studio can tell you the moment you make them. Nothing here blocks anything.
import { buildCueSheet, duplicateCueNumbers } from './cuesheet'
import { laneDevice, rigCheck, rigDept } from './rig'
import { validateShowchart, type PchiNode, type Showchart } from './index'

export interface Validation {
  /** "error": this will not work. "warn": look at it. */
  level: 'error' | 'warn'
  /** Where it is, so a click can go there. */
  scene?: string
  laneId?: string
  cueId?: string
  device?: string
  text: string
}

/** The program names a motion device allows: its `programs` parameter, without any `=number`. */
export function allowedPrograms(n: Pick<PchiNode, 'params'>): string[] {
  const p = n.params?.programs as unknown
  const list = Array.isArray(p) ? p : typeof p === 'string' ? p.split(/[,\n]/) : []
  return list.map((x) => String(x).split('=')[0].trim()).filter(Boolean)
}

/** Check a show against its rig. */
export function validateShow(chart: Showchart, nodes: PchiNode[]): Validation[] {
  const out: Validation[] = []

  for (const e of validateShowchart(chart)) {
    const scene = chart.states.find((s) => e.includes(`'${s.id}'`))?.id
    out.push({ level: e.includes('unreachable') ? 'warn' : 'error', scene, text: e })
  }

  for (const i of rigCheck(chart, nodes)) {
    if (i.kind === 'unused-device') out.push({ level: 'warn', device: i.deviceId, text: i.message })
    else out.push({ level: i.kind === 'ambiguous' ? 'warn' : 'error', scene: i.scene, laneId: i.laneId, text: i.message })
  }

  const dups = duplicateCueNumbers(chart)
  for (const s of chart.states)
    for (const l of s.timeline?.lanes ?? [])
      for (const c of l.cues)
        if (c.num !== undefined && dups.has(c.num)) out.push({ level: 'warn', scene: s.id, laneId: l.id, cueId: c.id, text: `${s.id}: cue ${c.num} has the same number as another cue` })

  for (const s of chart.states) {
    const duration = s.timeline?.duration_ms
    for (const l of s.timeline?.lanes ?? []) {
      const dev = laneDevice(nodes, l)
      for (const c of l.cues) {
        const at = { scene: s.id, laneId: l.id, cueId: c.id }
        const name = `${s.id} / ${l.name ?? l.id}: ${c.label || `cue at ${(c.at_ms / 1000).toFixed(2)} s`}`
        if (duration !== undefined && c.at_ms > duration) out.push({ level: 'warn', ...at, text: `${name} is after the end of the scene (${(duration / 1000).toFixed(1)} s), so it will never fire` })
        // a robot runs only programs its device allows: a cue for one that is not on the list is refused at run time
        if (l.dept === 'motion' && c.action.kind === 'motion.run' && dev && rigDept(dev) === 'motion') {
          const prog = String((c.action.payload as Record<string, unknown> | undefined)?.program ?? '').trim()
          const allowed = allowedPrograms(dev)
          if (!prog) out.push({ level: 'error', ...at, device: dev.id, text: `${name} names no program` })
          else if (allowed.length === 0) out.push({ level: 'error', ...at, device: dev.id, text: `${name}: ${dev.label} has no list of allowed programs, so nothing can be run on it. Add ${prog} to its programs in the Rig tab.` })
          else if (!allowed.includes(prog)) out.push({ level: 'error', ...at, device: dev.id, text: `${name}: ${prog} is not on ${dev.label}'s programs list (${allowed.join(', ')})` })
        }
      }
    }
  }

  // an action with nothing to say: an empty value goes nowhere useful
  for (const b of buildCueSheet(chart))
    for (const r of b.rows)
      if (r.source === 'cue' && r.kind !== 'motion.stop' && !r.value.trim() && r.cueId) out.push({ level: 'warn', scene: r.scene, laneId: r.laneId, cueId: r.cueId, text: `${r.scene} / ${r.laneName}: a ${r.kind} cue has no value` })

  return out
}

/** Count of problems per scene, per lane and per cue, for badges. */
export function validationIndex(v: Validation[]) {
  const by = (key: (x: Validation) => string | undefined) => {
    const m = new Map<string, { error: number; warn: number }>()
    for (const x of v) {
      const k = key(x)
      if (!k) continue
      const e = m.get(k) ?? { error: 0, warn: 0 }
      e[x.level]++
      m.set(k, e)
    }
    return m
  }
  return { scene: by((x) => x.scene), cue: by((x) => x.cueId), device: by((x) => x.device) }
}
