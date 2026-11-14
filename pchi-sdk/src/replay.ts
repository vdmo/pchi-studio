// Reading a recorded run (the runtime's GET /replay): where the show was at any moment, and what to draw.
// Pure functions; the Studio only decides how to show the results. The runtime builds the view (see crates/pchi-studio-api/src/recording.rs).

export type ReplayFrame = {
  /** ms from the start of the run */
  t: number
  kind: 'scene' | 'ignored' | 'context' | 'cue' | 'delivery' | 'edit' | 'decision' | 'held' | 'resume' | 'fenced'
  scene: string
  text: string
  key?: string
  value?: unknown
  bad?: boolean
  by?: string
  drift_ms?: number
}

export type ReplayScene = { scene: string; from_ms: number; to_ms: number | null; via: string }

export type ReplayView = {
  show?: string
  started_ts: number
  ended_ts: number
  length_ms: number
  initial_context: Record<string, unknown>
  scenes: ReplayScene[]
  frames: ReplayFrame[]
  summary: { scenes_visited: number; path: string[]; cues: number; worst_cue_drift_ms: number; deliveries_wrong: number; edits: number; decisions: number; who: string[]; line: string }
  verify?: { matches: boolean; inputs_checked?: number; differences?: string[]; verdict: string }
}

/** Where the show was at `t` ms into the run. */
export type ReplayAt = {
  t: number
  scene: string
  /** how long the scene had run */
  inScene: number
  context: Record<string, unknown>
  /** everything that had happened by t, newest last */
  happened: ReplayFrame[]
  /** the cues fired in the scene so far */
  cues: ReplayFrame[]
  /** the next moment after t, if any */
  next?: ReplayFrame
}

export function replayAt(v: ReplayView, t: number): ReplayAt {
  const at = Math.max(0, Math.min(t, v.length_ms))
  const visit = [...v.scenes].reverse().find((s) => s.from_ms <= at) ?? v.scenes[0]
  const happened = v.frames.filter((f) => f.t <= at)
  const context: Record<string, unknown> = { ...(v.initial_context ?? {}) }
  for (const f of happened) if (f.kind === 'context' && f.key) context[f.key] = f.value
  const from = visit?.from_ms ?? 0
  return {
    t: at,
    scene: visit?.scene ?? '',
    inScene: at - from,
    context,
    happened,
    cues: happened.filter((f) => f.kind === 'cue' && f.t >= from),
    next: v.frames.find((f) => f.t > at),
  }
}

/** Moments worth a mark on the timeline: edits, decisions, problems, a takeover or a restart. */
export const notable = (v: ReplayView): ReplayFrame[] =>
  v.frames.filter((f) => f.kind === 'edit' || f.kind === 'decision' || f.kind === 'resume' || f.kind === 'fenced' || (f.kind === 'delivery' && f.bad) || f.kind === 'held')

/** "1:23.4" for a time in ms. */
export function clock(ms: number): string {
  const s = Math.max(0, ms) / 1000
  const m = Math.floor(s / 60)
  return `${m}:${(s - m * 60).toFixed(1).padStart(4, '0')}`
}

/** Where on a 0..1 timeline a moment sits. */
export const position = (v: ReplayView, t: number): number => (v.length_ms > 0 ? Math.max(0, Math.min(1, t / v.length_ms)) : 0)
