// @pchi/sdk — the single source of truth for PCHI types.
// Studio, Core adapters, Vork adapter and Ghost bridge all import from here.
// Nothing in this package may depend on a UI framework.

// ───────────────────────── Runtime model ─────────────────────────
// Workspace > Project > Show > (Scene, Cue, Node, Connection, Output) + Asset

export interface Asset {
  id: string
  name: string
  kind: 'image' | 'video' | 'audio' | 'model' | 'shader' | 'other'
  uri: string
}

export interface Workspace {
  id: string
  name: string
  projects: string[] // project ids
  assets: Asset[] // shared asset library
}

export interface Project {
  id: string
  name: string
  shows: string[]
  venues: VenueModel[]
}

// ───────────────────────── Node taxonomy ─────────────────────────

export type NodeCategory =
  | 'control'
  | 'scene'
  | 'renderer'
  | 'distribution'
  | 'venue'
  | 'intelligence'

export type NodeType =
  // control
  | 'cue' | 'condition' | 'timer' | 'schedule' | 'trigger'
  // scene
  | 'scene' | 'camera' | 'lighting' | 'environment'
  // renderer
  | 'vork' | 'unreal' | 'touchdesigner' | 'notch'
  // distribution
  | 'ndi' | 'osc' | 'webrtc' | 'rtsp'
  // venue
  | 'ghost-arcade' | 'projector' | 'led-wall' | 'audio-zone' | 'laser' | 'lighting-desk' | 'audio-console'
  // intelligence
  | 'director-agent' | 'vision-agent' | 'audio-agent'

export type ParamValue = string | number | boolean

export interface NodeSpec {
  type: NodeType
  category: NodeCategory
  label: string
  defaults: Record<string, ParamValue>
}

export const CATEGORY_COLOR: Record<NodeCategory, string> = {
  control: '#f59e0b',
  scene: '#3b82f6',
  renderer: '#a855f7',
  distribution: '#10b981',
  venue: '#ec4899',
  intelligence: '#ef4444',
}

const spec = (
  category: NodeCategory,
  type: NodeType,
  label: string,
  defaults: Record<string, ParamValue> = {},
): NodeSpec => ({ type, category, label, defaults })

export const NODE_CATALOG: NodeSpec[] = [
  spec('control', 'cue', 'Cue', { trigger: 'manual' }),
  spec('control', 'condition', 'Condition', { expr: 'true' }),
  spec('control', 'timer', 'Timer', { seconds: 5 }),
  spec('control', 'schedule', 'Schedule', { cron: '' }),
  spec('control', 'trigger', 'Trigger', { source: 'osc' }),
  spec('scene', 'scene', 'Scene'),
  spec('scene', 'camera', 'Camera', { fov: 60 }),
  spec('scene', 'lighting', 'Lighting', { intensity: 1 }),
  spec('scene', 'environment', 'Environment', { fog: 0.2 }),
  spec('renderer', 'vork', 'Vork', { fps: 60 }),
  spec('renderer', 'unreal', 'Unreal', {}),
  spec('renderer', 'touchdesigner', 'TouchDesigner', {}),
  spec('renderer', 'notch', 'Notch', {}),
  spec('distribution', 'ndi', 'NDI', { stream: 'pchi-out' }),
  spec('distribution', 'osc', 'OSC', { host: '127.0.0.1', port: 9000 }),
  spec('distribution', 'webrtc', 'WebRTC', {}),
  spec('distribution', 'rtsp', 'RTSP', {}),
  spec('venue', 'ghost-arcade', 'Ghost Arcade', {}),
  spec('venue', 'projector', 'Projector', {}),
  spec('venue', 'led-wall', 'LED Wall', {}),
  spec('venue', 'audio-zone', 'Audio Zone', {}),
  spec('venue', 'laser', 'Laser', { host: '127.0.0.1', port: 8000 }),
  spec('venue', 'lighting-desk', 'Lighting Desk', { host: '127.0.0.1', port: 7700 }),
  spec('venue', 'audio-console', 'Audio Console', { host: '127.0.0.1', port: 10023 }),
  spec('intelligence', 'director-agent', 'Director Agent', {}),
  spec('intelligence', 'vision-agent', 'Vision Agent', {}),
  spec('intelligence', 'audio-agent', 'Audio Agent', {}),
]

export const specOf = (t: NodeType): NodeSpec =>
  NODE_CATALOG.find((s) => s.type === t) ?? NODE_CATALOG[0]

export const colorOf = (t: NodeType): string => CATEGORY_COLOR[specOf(t).category]

// ───────────────────────── Nodes, scenes, connections ─────────────────────────

/** Scene graph entries shown by the Scene Graph Inspector. */
export interface SceneItem {
  id: string
  kind: 'camera' | 'fog' | 'particles' | 'asset' | 'output' | 'trigger' | 'object'
  name: string
}

export interface PchiNode {
  id: string
  type: NodeType
  label: string
  position: { x: number; y: number }
  params: Record<string, ParamValue>
  /** Department this device serves ('visual' | 'lighting' | 'laser' | 'audio'); inferred from the type when unset. See rig.ts. */
  dept?: string
  /** Only meaningful when type === 'scene'. */
  sceneGraph?: SceneItem[]
}

/** What an edge *does*, not just that it exists. */
export type EdgeBehaviour = 'activate' | 'stream' | 'trigger' | 'control'

export const BEHAVIOUR_COLOR: Record<EdgeBehaviour, string> = {
  activate: '#60a5fa',
  stream: '#34d399',
  trigger: '#fbbf24',
  control: '#f472b6',
}

export interface PchiEdge {
  id: string
  source: string
  target: string
  behaviour: EdgeBehaviour
}

export interface PchiCue {
  id: string
  node: string
  start: number // seconds
  duration: number // seconds
}

// ───────────────────────── Show file (.pchi) ─────────────────────────

export interface PchiShowFile {
  version: '1.0'
  project: string
  name: string
  nodes: PchiNode[]
  edges: PchiEdge[]
  timeline: PchiCue[]
  assets: Asset[]
  venue?: VenueModel
  /** The executable show model. The graph is a view of this, not the other way round. */
  showchart?: Showchart
}

export interface PchiCapability {
  id: string
  nodeType: NodeType
  description?: string
}

// ───────────────────────── Venue twin ─────────────────────────

export interface VenueModel {
  venue: string
  surfaces: { id: string; name: string }[]
  projectors: { id: string; name: string; surface?: string }[]
  speakers: { id: string; name: string }[]
}

// ───────────────────────── Exchange (sharing) ─────────────────────────

export type ExchangePackageKind =
  | 'node-package'
  | 'scene-package'
  | 'venue-layout'
  | 'agent'
  | 'connector'
  | 'ghost-profile'
  | 'template'

/** Manifest every shareable package ships with (pchi-package.json). */
export interface ExchangePackage {
  id: string // e.g. "acme/nebula-scenes"
  kind: ExchangePackageKind
  name: string
  version: string
  description?: string
  provides: { nodes?: NodeType[]; scenes?: string[]; capabilities?: PchiCapability[] }
}

// ───────────────────────── Events ─────────────────────────

export interface PchiEvent {
  type: string // e.g. 'scene.activate', 'graph.update'
  [key: string]: unknown
}

export const newShow = (project = 'Untitled Project'): PchiShowFile => {
  const n = (id: string, type: NodeType, label: string, x: number): PchiNode => ({
    id,
    type,
    label,
    position: { x, y: 100 },
    params: { ...specOf(type).defaults },
    ...(type === 'scene'
      ? {
          sceneGraph: [
            { id: `${id}-cam`, kind: 'camera', name: 'Camera' },
            { id: `${id}-fog`, kind: 'fog', name: 'Fog' },
            { id: `${id}-par`, kind: 'particles', name: 'Particles' },
          ] as SceneItem[],
        }
      : {}),
  })
  return {
    version: '1.0',
    project,
    name: 'untitled-show',
    nodes: [
      n('cue-1', 'cue', 'Cue', 0),
      n('scene-nebula', 'scene', 'Nebula', 230),
      n('vork-main', 'vork', 'Vork', 460),
      n('ndi-out', 'ndi', 'NDI', 690),
    ],
    edges: [
      { id: 'e1', source: 'cue-1', target: 'scene-nebula', behaviour: 'trigger' },
      { id: 'e2', source: 'scene-nebula', target: 'vork-main', behaviour: 'activate' },
      { id: 'e3', source: 'vork-main', target: 'ndi-out', behaviour: 'stream' },
    ],
    timeline: [
      { id: 'c1', node: 'scene-nebula', start: 0, duration: 8 },
      { id: 'c2', node: 'vork-main', start: 2, duration: 10 },
    ],
    assets: [],
  }
}

// ───────────────────────── Event simulation ─────────────────────────

export interface SimStep {
  depth: number
  node: string
  viaEdge?: string
  behaviour?: EdgeBehaviour
}

/**
 * Pure, deterministic propagation of an event from a start node along edges
 * (breadth-first, cycle-safe). Studio animates the steps; Core and tests can
 * reuse the exact same function.
 */
export function simulate(show: Pick<PchiShowFile, 'nodes' | 'edges'>, start: string): SimStep[] {
  if (!show.nodes.some((n) => n.id === start)) return []
  const steps: SimStep[] = [{ depth: 0, node: start }]
  const seen = new Set([start])
  let frontier = [start]
  let depth = 0
  while (frontier.length) {
    depth++
    const next: string[] = []
    for (const id of frontier) {
      for (const e of show.edges.filter((x) => x.source === id)) {
        if (seen.has(e.target)) continue
        seen.add(e.target)
        steps.push({ depth, node: e.target, viaEdge: e.id, behaviour: e.behaviour })
        next.push(e.target)
      }
    }
    frontier = next
  }
  return steps
}

// ───────────────────────── Showchart (mirrors crates/pchi-showchart) ─────────────────────────
// A show as an executable state machine. Keep step()/validateShowchart()
// behaviourally identical to the Rust engine; the same portal.json must give
// the same result in both.

export interface ShowAction {
  kind: string
  payload?: Record<string, unknown>
}

/** A cue on a lane: an action that fires `at_ms` after the state is entered. */
export interface ShowCue {
  id: string
  at_ms: number
  /** Display length only; the action fires once, at at_ms. */
  length_ms?: number
  label?: string
  /** Cue number as called by the stage manager: assigned once, then kept (see ensureCueNumbers). */
  num?: number
  action: ShowAction
}

export interface ShowLane {
  id: string
  name?: string
  /** 'visual' | 'lighting' | 'laser' | 'audio' | 'other' (see DEPTS) */
  dept?: string
  /** Id of the rig device (a PchiNode) that handles this lane. See rig.ts. */
  device?: string
  cues: ShowCue[]
}

/** Lanes of cues that play while a state is active (executed by the runtime). */
export interface ShowTimeline {
  duration_ms?: number
  lanes: ShowLane[]
}

export interface ShowState {
  id: string
  label?: string
  enter_actions?: ShowAction[]
  exit_actions?: ShowAction[]
  timeline?: ShowTimeline
}

export interface ShowTransition {
  source: string
  target: string
  /** Fires on this event name... */
  event?: string
  /** ...or after this delay (the scheduler injects `after:<ms>`). */
  after_ms?: number
  /** A name from `Showchart.guards`, or an inline expression. */
  guard?: string
}

export type ShowContext = Record<string, unknown>

export interface Showchart {
  /** Where each state sits on the canvas (saved with the show). Missing states are laid out automatically. */
  layout?: Record<string, { x: number; y: number }>
  id?: string
  name?: string
  version?: string
  initial: string
  context?: ShowContext
  guards?: Record<string, string>
  states: ShowState[]
  transitions: ShowTransition[]
  /** Highest cue number handed out; never reused after a delete. */
  cue_seq?: number
}

export const timerEvent = (ms: number) => `after:${ms}`
export const transitionTrigger = (t: ShowTransition): string | undefined =>
  t.event ?? (t.after_ms !== undefined ? timerEvent(t.after_ms) : undefined)
export const transitionLabel = (t: ShowTransition): string => {
  const base = t.event ?? (t.after_ms !== undefined ? `after ${t.after_ms}ms` : '(no trigger)')
  return t.guard ? `${base} [${t.guard}]` : base
}

// ── Guards (mirror of crates/pchi-showchart/src/guard.rs) ──
// key | !key | key OP value, OP in == != >= <= > <

const GUARD_OPS = ['==', '!=', '>=', '<=', '>', '<'] as const
type GuardOp = (typeof GUARD_OPS)[number]
type GuardExpr =
  | { k: 'truthy'; key: string }
  | { k: 'not'; key: string }
  | { k: 'cmp'; key: string; op: GuardOp; value: unknown }

const isKey = (s: string) => /^[A-Za-z0-9_.]+$/.test(s)

function parseGuard(src: string): GuardExpr | string {
  const s = src.trim()
  if (!s) return 'empty guard expression'
  if (s.startsWith('!')) {
    const k = s.slice(1).trim()
    return isKey(k) ? { k: 'not', key: k } : `cannot parse guard '${src}'`
  }
  for (const op of GUARD_OPS) {
    const i = s.indexOf(op)
    if (i < 0) continue
    const l = s.slice(0, i).trim()
    const r = s.slice(i + op.length).trim()
    if (!isKey(l) || !r) return `cannot parse guard '${src}'`
    let value: unknown
    try {
      value = JSON.parse(r)
    } catch {
      value = r
    }
    return { k: 'cmp', key: l, op, value }
  }
  return isKey(s) ? { k: 'truthy', key: s } : `cannot parse guard '${src}'`
}

const lookup = (ctx: ShowContext, key: string): unknown => {
  let cur: unknown = ctx
  for (const p of key.split('.')) {
    if (cur === null || typeof cur !== 'object' || !(p in (cur as object))) return undefined
    cur = (cur as Record<string, unknown>)[p]
  }
  return cur
}

const isTruthy = (v: unknown) =>
  v === undefined || v === null
    ? false
    : typeof v === 'boolean'
      ? v
      : typeof v === 'number'
        ? v !== 0
        : typeof v === 'string'
          ? v.length > 0
          : Array.isArray(v)
            ? v.length > 0
            : Object.keys(v as object).length > 0

/** False when the expression does not parse (same as the Rust engine). */
export function evalGuard(expr: string, ctx: ShowContext): boolean {
  const g = parseGuard(expr)
  if (typeof g === 'string') return false
  const v = lookup(ctx, g.key)
  if (g.k === 'truthy') return isTruthy(v)
  if (g.k === 'not') return !isTruthy(v)
  if (v === undefined) return g.op === '!='
  if (typeof v === 'number' && typeof g.value === 'number') {
    switch (g.op) {
      case '==': return v === g.value
      case '!=': return v !== g.value
      case '>': return v > g.value
      case '>=': return v >= g.value
      case '<': return v < g.value
      case '<=': return v <= g.value
    }
  }
  const same = JSON.stringify(v) === JSON.stringify(g.value)
  return g.op === '==' ? same : g.op === '!=' ? !same : false
}

const guardExpr = (c: Showchart, g: string) => c.guards?.[g] ?? g

export const newShowchart = (): Showchart => ({
  initial: 'PRESHOW',
  states: [
    { id: 'PRESHOW', label: 'Doors open' },
    { id: 'INTRO', label: 'Intro' },
    {
      id: 'PORTAL',
      label: 'Portal',
      enter_actions: [
        { kind: 'scene.activate', payload: { scene: 'portal' } },
        { kind: 'renderer.activate', payload: { renderer: 'vork' } },
      ],
    },
    { id: 'FINALE', label: 'Finale' },
  ],
  transitions: [
    { source: 'PRESHOW', target: 'INTRO', event: 'START' },
    { source: 'INTRO', target: 'PORTAL', event: 'PORTAL_OPEN' },
    { source: 'PORTAL', target: 'FINALE', event: 'END' },
  ],
})

/** Pure transition. null = event ignored. */
export function step(
  chart: Showchart,
  ctx: ShowContext,
  current: string,
  event: string,
): { target: string; actions: ShowAction[] } | null {
  const t = chart.transitions.find(
    (x) =>
      x.source === current &&
      transitionTrigger(x) === event &&
      (x.guard === undefined || evalGuard(guardExpr(chart, x.guard), ctx)),
  )
  if (!t) return null
  const from = chart.states.find((s) => s.id === current)
  const to = chart.states.find((s) => s.id === t.target)
  return {
    target: t.target,
    actions: [...(from?.exit_actions ?? []), ...(to?.enter_actions ?? [])],
  }
}

export function enterActions(chart: Showchart, state: string): ShowAction[] {
  return chart.states.find((s) => s.id === state)?.enter_actions ?? []
}

export function validateShowchart(c: Showchart): string[] {
  const errs: string[] = []
  const ids = new Set<string>()
  for (const s of c.states) {
    if (ids.has(s.id)) errs.push(`duplicate state id '${s.id}'`)
    ids.add(s.id)
  }
  if (!ids.has(c.initial)) errs.push(`initial state '${c.initial}' does not exist`)
  const seen = new Set<string>()
  for (const t of c.transitions) {
    const trig = transitionTrigger(t)
    if (!ids.has(t.source)) errs.push(`transition on '${trig}' has unknown source '${t.source}'`)
    if (!ids.has(t.target)) errs.push(`transition on '${trig}' has unknown target '${t.target}'`)
    if (trig === undefined) errs.push(`transition ${t.source} -> ${t.target} has neither an event nor after_ms`)
    if (t.guard !== undefined) {
      const g = parseGuard(guardExpr(c, t.guard))
      if (typeof g === 'string') errs.push(`transition on '${trig}' has an invalid guard '${t.guard}': ${g}`)
    } else if (trig !== undefined) {
      const k = `${t.source}\u0000${trig}`
      if (seen.has(k)) errs.push(`state '${t.source}' has two unguarded transitions on '${trig}' (the second can never fire)`)
      seen.add(k)
    }
  }
  if (ids.has(c.initial)) {
    const reach = new Set<string>()
    const stack = [c.initial]
    while (stack.length) {
      const s = stack.pop()!
      if (reach.has(s)) continue
      reach.add(s)
      c.transitions.filter((t) => t.source === s).forEach((t) => stack.push(t.target))
    }
    c.states.filter((s) => !reach.has(s.id)).forEach((s) => errs.push(`state '${s.id}' is unreachable from the initial state`))
  }
  return errs
}

/** The PCHI event for an action, in the flat shape adapters accept. */
export const actionToEvent = (a: ShowAction): PchiEvent => ({
  ...(a.payload ?? {}),
  type: a.kind,
})

/** Left-to-right layout by distance from the initial state. */
export function layoutShowchart(c: Showchart): Record<string, { x: number; y: number }> {
  const depth = new Map<string, number>([[c.initial, 0]])
  const queue = [c.initial]
  while (queue.length) {
    const s = queue.shift()!
    for (const t of c.transitions.filter((x) => x.source === s)) {
      if (!depth.has(t.target)) {
        depth.set(t.target, depth.get(s)! + 1)
        queue.push(t.target)
      }
    }
  }
  let extra = Math.max(0, ...depth.values()) + 1
  const rows = new Map<number, number>()
  const out: Record<string, { x: number; y: number }> = {}
  for (const s of c.states) {
    const d = depth.get(s.id) ?? extra++
    const r = rows.get(d) ?? 0
    rows.set(d, r + 1)
    out[s.id] = { x: d * 320, y: r * 170 }
  }
  return out
}

/** null if the guard expression parses, otherwise why not. */
export function guardError(expr: string): string | null {
  const g = parseGuard(expr)
  return typeof g === 'string' ? g : null
}

export * from './commands'
export * from './live'
export * from './cuesheet'
export * from './capture'
export * from './rig'

// ───────────────────────── Timelines (mirrors crates/pchi-showchart/src/timeline.rs) ─────────────────────────

export const DEPTS = [
  { id: 'visual', label: 'Visuals', color: '#a855f7', kind: 'scene.activate', field: 'scene' },
  { id: 'lighting', label: 'Lighting', color: '#f59e0b', kind: 'lighting.look', field: 'look' },
  { id: 'laser', label: 'Lasers', color: '#ef4444', kind: 'laser.cue', field: 'cue' },
  { id: 'audio', label: 'Audio', color: '#10b981', kind: 'audio.snapshot', field: 'snapshot' },
  { id: 'other', label: 'Other', color: '#7a7f92', kind: 'cue.trigger', field: 'cue' },
] as const

export const deptOf = (id?: string) => DEPTS.find((d) => d.id === id) ?? DEPTS[4]

/** The standard set of lanes a new timeline starts with. */
export const standardLanes = (): ShowLane[] =>
  DEPTS.slice(0, 4).map((d) => ({ id: d.id, name: d.label, dept: d.id, cues: [] }))

/** When the last cue ends (or the declared duration), for sizing the ruler. */
export function timelineEnd(t?: ShowTimeline): number {
  if (!t) return 0
  const cues = t.lanes.flatMap((l) => l.cues)
  const last = cues.reduce((m, c) => Math.max(m, c.at_ms + (c.length_ms ?? 0)), 0)
  return Math.max(t.duration_ms ?? 0, last)
}

/** Every cue in firing order, like ShowEngine::pending_cues in Rust. */
export function scheduledCues(t?: ShowTimeline): { at_ms: number; lane: string; cue: string; action: ShowAction }[] {
  if (!t) return []
  return t.lanes
    .flatMap((l) => l.cues.map((c) => ({ at_ms: c.at_ms, lane: l.id, cue: c.id, action: c.action })))
    .sort((a, b) => a.at_ms - b.at_ms) // stable sort: ties keep lane order
}
