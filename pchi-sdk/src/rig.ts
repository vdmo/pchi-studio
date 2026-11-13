// Linking the rig (who can do it) to the showchart (what happens): which
// department a device serves, which device a lane uses, and what does not add up.
// Pure functions; the Studio only decides how to show the results.
import type { PchiNode, Showchart } from './index'

/** The four departments a rig device can serve (cue lanes use the same ids). */
export const RIG_DEPTS = ['visual', 'lighting', 'laser', 'audio'] as const

const TYPE_DEPT: Record<string, string> = {
  vork: 'visual', unreal: 'visual', touchdesigner: 'visual', notch: 'visual',
  projector: 'visual', 'led-wall': 'visual',
  lighting: 'lighting', 'lighting-desk': 'lighting',
  laser: 'laser',
  resolume: 'visual', grandma3: 'lighting', eos: 'lighting', x32: 'audio', beyond: 'laser', qlab: 'audio', magicq: 'lighting',  'audio-zone': 'audio', 'audio-console': 'audio', 'audio-agent': 'audio',
}

/** The department a device serves: the one set on it, else inferred from its type, else none. */
export function rigDept(n: Pick<PchiNode, 'type' | 'dept'>): string | undefined {
  if (n.dept) return n.dept
  return TYPE_DEPT[n.type]
}

/** Devices that serve `dept`. */
export const devicesFor = (nodes: PchiNode[], dept?: string) => nodes.filter((n) => dept !== undefined && rigDept(n) === dept)

/**
 * The device a lane uses: the one chosen on the lane, otherwise the only device
 * for its department. With several candidates and none chosen, nothing is assumed.
 */
export function laneDevice(nodes: PchiNode[], lane: { dept?: string; device?: string }): PchiNode | undefined {
  if (lane.device) return nodes.find((n) => n.id === lane.device)
  const c = devicesFor(nodes, lane.dept)
  return c.length === 1 ? c[0] : undefined
}

export type RigIssue =
  | { kind: 'no-device'; scene: string; laneId: string; laneName: string; dept: string; message: string }
  | { kind: 'ambiguous'; scene: string; laneId: string; laneName: string; dept: string; message: string }
  | { kind: 'missing-device'; scene: string; laneId: string; laneName: string; dept: string; message: string }
  | { kind: 'unused-device'; deviceId: string; message: string }

/**
 * What does not add up between the show and the rig. Only lanes that have cues
 * are checked (an empty lane is harmless). A lane with no department is skipped.
 * Nothing here blocks a show: they are things to look at before doors.
 */
export function rigCheck(chart: Showchart, nodes: PchiNode[]): RigIssue[] {
  const out: RigIssue[] = []
  const used = new Set<string>()
  for (const s of chart.states)
    for (const l of s.timeline?.lanes ?? []) {
      if (l.cues.length === 0 || !l.dept || l.dept === 'other') continue
      const name = l.name ?? l.id
      const base = { scene: s.id, laneId: l.id, laneName: name, dept: l.dept }
      if (l.device) {
        const d = nodes.find((n) => n.id === l.device)
        if (!d) out.push({ kind: 'missing-device', ...base, message: `${s.id} / ${name}: the device it points at was removed from the rig` })
        else used.add(d.id)
        continue
      }
      const c = devicesFor(nodes, l.dept)
      if (c.length === 0) out.push({ kind: 'no-device', ...base, message: `${s.id} / ${name}: no ${l.dept} device in the rig` })
      else if (c.length > 1) out.push({ kind: 'ambiguous', ...base, message: `${s.id} / ${name}: ${c.length} ${l.dept} devices, pick one` })
      else used.add(c[0].id)
    }
  for (const n of nodes) {
    if (rigDept(n) && !used.has(n.id)) out.push({ kind: 'unused-device', deviceId: n.id, message: `${n.label}: no cue uses this device` })
  }
  return out
}

/** Point a lane at a device (or clear it with undefined). */
export function setLaneDevice(chart: Showchart, scene: string, laneId: string, device: string | undefined): Showchart {
  return {
    ...chart,
    states: chart.states.map((s) =>
      s.id !== scene || !s.timeline
        ? s
        : {
            ...s,
            timeline: {
              ...s.timeline,
              lanes: s.timeline.lanes.map((l) => {
                if (l.id !== laneId) return l
                const { device: _drop, ...rest } = l
                void _drop
                return device ? { ...rest, device } : rest
              }),
            },
          },
    ),
  }
}

/** Forget a removed device everywhere in the show, so no lane points at nothing. */
export function clearDevice(chart: Showchart, deviceId: string): Showchart {
  let changed = false
  const states = chart.states.map((s) => {
    if (!s.timeline || !s.timeline.lanes.some((l) => l.device === deviceId)) return s
    changed = true
    return { ...s, timeline: { ...s.timeline, lanes: s.timeline.lanes.map((l) => (l.device === deviceId ? setOne(l) : l)) } }
  })
  return changed ? { ...chart, states } : chart
}
const setOne = <T extends { device?: string }>(l: T): T => {
  const { device: _d, ...rest } = l
  void _d
  return rest as T
}

/**
 * Who must be ready for `scene`: the departments and devices that have cues in it.
 * Returns board keys: a device id where the rig has devices for the lane (all
 * candidates when the lane has not chosen one), otherwise the department id.
 * An empty set means nobody is needed, so no "on hold" should hold up the GO.
 */
export function requiredFor(chart: Showchart, scene: string | undefined, nodes: PchiNode[]): Set<string> {
  const need = new Set<string>()
  const lanes = chart.states.find((s) => s.id === scene)?.timeline?.lanes ?? []
  for (const l of lanes) {
    if (l.cues.length === 0 || !l.dept || l.dept === 'other') continue
    if (l.device) {
      need.add(l.device)
      continue
    }
    const c = devicesFor(nodes, l.dept)
    if (c.length === 0) need.add(l.dept)
    else c.forEach((n) => need.add(n.id))
  }
  return need
}
