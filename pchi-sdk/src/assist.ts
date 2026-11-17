// The AI panel's logic, with no network in it: what is sent, how a reply is read, and what a proposal changes.
//
// The rules it keeps (they are what make an AI helper acceptable next to a robot):
//  - The model sees the scenes and cues, and for each device only its id, label, type, department and allowed
//    program NAMES. Never an address, a port or a password.
//  - It can only propose a new showchart. The rig is never changed by it, and the proposal goes into the editor
//    like any other edit (one undo step). Sending to the runtime is a separate step with its own diff and key.
//  - A proposal that adds a problem the show did not already have cannot be applied.
import { ensureCueNumbers, type PchiNode, type Showchart } from './index'
import { allowedPrograms, validateShow, type Validation } from './validations'
import { rigDept } from './rig'

export const ASSIST_SYSTEM = `You edit show files for PCHI Studio, a show-control tool for live events.

A show is a "showchart": scenes ("states") with transitions between them, and inside each scene lanes of timed cues.
You are given the current showchart and the rig (the devices that exist), then an instruction. Reply with ONE JSON object and nothing else:
{"summary": "one or two plain sentences saying what you changed", "showchart": { ...the complete new showchart... }}

Rules:
- Return the whole showchart, not a patch. Keep every id, cue number ("num") and field you were not asked to change.
- Cues live in state.timeline.lanes[].cues[]: {"id","at_ms","action":{"kind","payload"}, "num"?, "label"?, "length_ms"?}. at_ms counts from the start of the scene.
- A lane has a "dept" (visual, lighting, laser, audio, motion, other) and may have a "device" (an id from the rig). Use only device ids that are in the rig. Never invent a device.
- Action kinds: scene.activate {scene}, lighting.look {look}, laser.cue {cue}, audio.snapshot {snapshot}, motion.run {program}, cue.trigger {cue}.
- A motion.run cue may name only a program that is on that device's "programs" list in the rig. If the instruction needs a program that is not on the list, do not add the cue: say so in the summary.
- You cannot change the rig, arm anything, or send anything. If asked to, say you cannot, and return the showchart unchanged.
- If the instruction is unclear or cannot be done, return the showchart unchanged and explain in the summary.`

/** What the model is shown of the rig: nothing that says where a device is, or how to log into it. */
export function redactRig(nodes: PchiNode[]) {
  return nodes
    .filter((n) => rigDept(n))
    .map((n) => {
      const programs = allowedPrograms(n)
      return { id: n.id, label: n.label, type: n.type, dept: rigDept(n), ...(programs.length ? { programs } : {}) }
    })
}

/** The user message: the show, the (redacted) rig and the instruction. The editor-only layout is left out. */
export function assistPrompt(chart: Showchart, nodes: PchiNode[], instruction: string): string {
  const { layout: _layout, ...rest } = chart
  void _layout
  return `Current showchart:\n${JSON.stringify(rest)}\n\nRig:\n${JSON.stringify(redactRig(nodes))}\n\nInstruction:\n${instruction.trim()}`
}

export interface Proposal {
  summary: string
  chart: Showchart
  /** Plain-language list of what differs from the current show. */
  changes: string[]
  /** Problems the proposal has that the current show does not. */
  added: Validation[]
  /** Problems the current show has that the proposal fixes. */
  fixed: Validation[]
}

const sig = (v: Validation) => `${v.level}|${v.scene ?? ''}|${v.cueId ?? ''}|${v.text}`

/** What differs between two showcharts, in words a person would use. */
export function describeChanges(a: Showchart, b: Showchart): string[] {
  const out: string[] = []
  const ids = (c: Showchart) => new Set(c.states.map((s) => s.id))
  const [ia, ib] = [ids(a), ids(b)]
  for (const id of ib) if (!ia.has(id)) out.push(`Adds scene ${id}`)
  for (const id of ia) if (!ib.has(id)) out.push(`Removes scene ${id}`)
  if (a.initial !== b.initial) out.push(`The show now starts in ${b.initial} (was ${a.initial})`)
  const tr = (c: Showchart) => new Set(c.transitions.map((t) => `${t.source} -> ${t.target} (${t.event ?? (t.after_ms !== undefined ? `after ${t.after_ms / 1000} s` : '?')})`))
  const [ta, tb] = [tr(a), tr(b)]
  for (const t of tb) if (!ta.has(t)) out.push(`Adds transition ${t}`)
  for (const t of ta) if (!tb.has(t)) out.push(`Removes transition ${t}`)
  const cues = (c: Showchart) => {
    const m = new Map<string, { scene: string; lane: string; at: number; what: string }>()
    for (const s of c.states)
      for (const l of s.timeline?.lanes ?? [])
        for (const q of l.cues) m.set(`${s.id}/${q.id}`, { scene: s.id, lane: l.name ?? l.id, at: q.at_ms, what: `${q.action.kind} ${JSON.stringify(q.action.payload ?? {})}` })
    return m
  }
  const [ca, cb] = [cues(a), cues(b)]
  const show = (x: { scene: string; lane: string; at: number; what: string }) => `${x.scene} / ${x.lane} at ${(x.at / 1000).toFixed(2)} s: ${x.what}`
  for (const [k, v] of cb) {
    const old = ca.get(k)
    if (!old) out.push(`Adds cue ${show(v)}`)
    else if (old.at !== v.at || old.what !== v.what || old.lane !== v.lane) out.push(`Changes cue ${show(old)}  →  ${show(v)}`)
  }
  for (const [k, v] of ca) if (!cb.has(k)) out.push(`Removes cue ${show(v)}`)
  return out
}

/** Read a model reply into a proposal, or say why it cannot be used. */
export function readProposal(reply: string, current: Showchart, nodes: PchiNode[]): { ok: true; proposal: Proposal } | { ok: false; error: string } {
  const text = reply.trim().replace(/^```(?:json)?\s*/i, '').replace(/\s*```$/, '')
  const start = text.indexOf('{')
  const end = text.lastIndexOf('}')
  if (start < 0) return { ok: false, error: 'The reply was not a show: it had no JSON in it.' }
  if (end <= start) return { ok: false, error: 'The reply was cut off before it finished (try a shorter show or one scene at a time).' }
  let obj: { summary?: unknown; showchart?: unknown }
  try {
    obj = JSON.parse(text.slice(start, end + 1))
  } catch {
    return { ok: false, error: 'The reply was cut off or was not valid JSON.' }
  }
  const sc = obj.showchart as Partial<Showchart> | undefined
  if (!sc || typeof sc !== 'object' || !Array.isArray(sc.states) || !Array.isArray(sc.transitions) || typeof sc.initial !== 'string') {
    return { ok: false, error: 'The reply had no complete showchart (it needs initial, states and transitions).' }
  }
  const summary = typeof obj.summary === 'string' && obj.summary.trim() ? obj.summary.trim() : 'No explanation was given.'
  // a scene with no id, or a lane with no cue list, cannot even be checked: refuse it before anything reads it
  if (sc.states.some((s) => !s || typeof s.id !== 'string' || (s.timeline && (!Array.isArray(s.timeline.lanes) || s.timeline.lanes.some((l) => !l || !Array.isArray(l.cues) || l.cues.some((c) => !c || typeof c.id !== 'string' || typeof c.at_ms !== 'number' || !c.action || typeof c.action.kind !== 'string'))))))
    return { ok: false, error: 'The reply had a scene or cue that is not shaped like a show (a scene needs an id, a cue needs an id, a time and an action).' }
  // the canvas layout is the editor's, not the model's; and a cue number once handed out is never reused, whatever counter the model sent
  const seq = Math.max(current.cue_seq ?? 0, Number(sc.cue_seq) || 0)
  const chart = ensureCueNumbers({ ...(sc as Showchart), ...(seq ? { cue_seq: seq } : {}), ...(current.layout ? { layout: current.layout } : {}) })
  const before = validateShow(current, nodes)
  const after = validateShow(chart, nodes)
  const had = new Set(before.map(sig))
  const now = new Set(after.map(sig))
  return {
    ok: true,
    proposal: { summary, chart, changes: describeChanges(current, chart), added: after.filter((v) => !had.has(sig(v))), fixed: before.filter((v) => !now.has(sig(v))) },
  }
}

/** A proposal may be applied only if it adds no problem the show did not already have. */
export const canApply = (p: Proposal): boolean => p.added.length === 0

/**
 * Where a proposal stands against the draft as it is NOW (identity, not content: any edit makes a new chart).
 *  - "ready":   the draft is exactly what the model was shown, so Apply replaces nothing the model did not see.
 *  - "applied": the draft is exactly what Apply made it.
 *  - "stale":   the draft changed since (before or after Apply). Applying would overwrite those edits, so it is not offered.
 * A proposal that was applied and then undone is "ready" again, because the draft is back to what the model was shown.
 */
export function proposalStatus(current: Showchart, base: Showchart, proposed: Showchart): 'ready' | 'applied' | 'stale' {
  if (current === proposed && current !== base) return 'applied'
  if (current === base) return 'ready'
  return 'stale'
}
