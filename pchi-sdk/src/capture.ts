// Rehearsal capture: drop cues at the playhead while a scene plays, then
// tidy their timing onto a grid. Pure functions over the showchart, so the
// Studio panel only decides WHEN, and the result is the same cue a typed
// command or a drag would have made.
import { deptOf, type ShowCue, type ShowLane, type Showchart } from './index'

/** Captured times are kept to 50 ms: fine enough to feel exact, coarse enough to read. */
export const CAPTURE_STEP_MS = 50

export interface Captured {
  chart: Showchart
  laneId: string
  cueId: string
  /** True when the department had no lane yet and one was added. */
  laneAdded: boolean
}

/**
 * Add a blank cue for `dept` at `at_ms` in `scene`. The lane for that
 * department is used if there is one (the first, if several), otherwise one is
 * created. Returns null if the scene does not exist.
 */
export function captureCue(chart: Showchart, scene: string, dept: string, at_ms: number, newId: () => string): Captured | null {
  const st = chart.states.find((s) => s.id === scene)
  if (!st) return null
  const d = deptOf(dept)
  const lanes = st.timeline?.lanes ?? []
  let lane = lanes.find((l) => l.dept === d.id)
  let laneAdded = false
  if (!lane) {
    let id: string = d.id
    for (let n = 2; lanes.some((l) => l.id === id); n++) id = `${d.id}-${n}`
    lane = { id, name: d.label, dept: d.id, cues: [] } satisfies ShowLane
    laneAdded = true
  }
  const cue: ShowCue = {
    id: newId(),
    at_ms: Math.max(0, Math.round(at_ms / CAPTURE_STEP_MS) * CAPTURE_STEP_MS),
    length_ms: 2000,
    action: { kind: d.kind, payload: { [d.field]: '' } },
  }
  const target = lane
  const next: ShowLane = { ...target, cues: [...target.cues, cue] }
  const newLanes = laneAdded ? [...lanes, next] : lanes.map((l) => (l.id === target.id ? next : l))
  return {
    chart: { ...chart, states: chart.states.map((s) => (s.id === scene ? { ...s, timeline: { ...(s.timeline ?? {}), lanes: newLanes } } : s)) },
    laneId: target.id,
    cueId: cue.id,
    laneAdded,
  }
}

/** Milliseconds per beat at `bpm` (divided into `per` parts: 2 = half a beat). */
export const beatMs = (bpm: number, per = 1) => 60000 / Math.min(400, Math.max(20, bpm)) / per

/**
 * Move every cue in `scene` (or only those in `only`) to the nearest multiple of
 * `gridMs`. Cues already on the grid are left alone. `moved` counts the cues that changed.
 */
export function quantizeCues(chart: Showchart, scene: string, gridMs: number, only?: ReadonlySet<string>): { chart: Showchart; moved: number } {
  if (!(gridMs > 0)) return { chart, moved: 0 }
  let moved = 0
  const states = chart.states.map((s) => {
    if (s.id !== scene || !s.timeline) return s
    return {
      ...s,
      timeline: {
        ...s.timeline,
        lanes: s.timeline.lanes.map((l) => ({
          ...l,
          cues: l.cues.map((c) => {
            if (only && !only.has(c.id)) return c
            const at = Math.max(0, Math.round(Math.round(c.at_ms / gridMs) * gridMs))
            if (at === c.at_ms) return c
            moved++
            return { ...c, at_ms: at }
          }),
        })),
      },
    }
  })
  return { chart: moved ? { ...chart, states } : chart, moved }
}
