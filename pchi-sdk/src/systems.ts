// Systems (rig devices) by sentence: "add qlab", "add x32 called FOH desk at 192.168.1.20",
// "set qlab host to 10.0.0.5", "remove qlab". Pure functions over the rig, like commands.ts is
// over the showchart; runStudioCommand() runs a sentence that may touch either.
import { CommandError, runCommandText, splitCommandFragments, type CommandResult } from './commands'
import { hintFor } from './suggest'
import { specOf, type NodeType, type PchiEdge, type PchiNode, type Showchart } from './index'
import { rigDept, setLaneDevice } from './rig'

export interface RigState {
  nodes: PchiNode[]
  edges: PchiEdge[]
}

/** What people call each tool, longest names first within a type. */
const SYSTEMS: { type: NodeType; names: string[] }[] = [
  { type: 'resolume', names: ['resolume arena', 'resolume avenue', 'resolume', 'arena'] },
  { type: 'grandma3', names: ['grand ma 3', 'grandma 3', 'grandma3', 'grandma2', 'grandma', 'ma lighting', 'ma 3', 'ma3'] },
  { type: 'eos', names: ['etc eos', 'eos family', 'eos'] },
  { type: 'x32', names: ['behringer x32', 'midas m32', 'behringer', 'x-air', 'x32', 'm32'] },
  { type: 'beyond', names: ['pangolin beyond', 'pangolin', 'beyond'] },
  { type: 'qlab', names: ['q-lab', 'q lab', 'qlab'] },
  { type: 'magicq', names: ['chamsys magicq', 'magic q', 'magicq', 'chamsys'] },
  { type: 'laser', names: ['laser projector', 'lasers', 'laser'] },
  { type: 'lighting-desk', names: ['lighting console', 'lighting desk', 'light console', 'light desk'] },
  { type: 'audio-console', names: ['audio console', 'sound console', 'sound desk', 'mixing desk', 'mixer'] },
  { type: 'touchdesigner', names: ['touch designer', 'touchdesigner'] },
  { type: 'unreal', names: ['unreal engine', 'unreal'] },
  { type: 'notch', names: ['notch'] },
  { type: 'vork', names: ['vork'] },
  { type: 'projector', names: ['projector'] },
  { type: 'led-wall', names: ['led screen', 'led wall'] },
  { type: 'audio-zone', names: ['speaker zone', 'audio zone'] },
  { type: 'ndi', names: ['ndi'] },
  { type: 'osc', names: ['osc'] },
]

const DEPT_ALIAS: Record<string, string> = { visual: 'visual', visuals: 'visual', video: 'visual', vj: 'visual', lighting: 'lighting', lights: 'lighting', light: 'lighting', laser: 'laser', lasers: 'laser', audio: 'audio', sound: 'audio' }

const squash = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '')
const HOST = /^(?:\d{1,3}(?:\.\d{1,3}){3}|[A-Za-z][\w.-]*)(?::(\d{2,5}))?$/
const VERBS = '(add|create|new|connect|patch(?:\\s+in)?|plug\\s+in|set\\s*up|use)'

/** A system name at the start of `text` (longest first): its type and what is left over. */
function leadingSystem(text: string): { type: NodeType; rest: string } | null {
  const t = text.trim()
  const all = SYSTEMS.flatMap((s) => s.names.map((n) => ({ type: s.type, n }))).sort((a, b) => b.n.length - a.n.length)
  for (const { type, n } of all) {
    const m = t.match(new RegExp(`^${n.replace(/[-\s]/g, '[-\\s]?')}(?![\\w])\\s*(.*)$`, 'i'))
    if (m) return { type, rest: m[1].trim() }
  }
  return null
}

/** The device a phrase names: its exact label, else its tool name (when only one of that tool is in the rig). */
function findNode(rig: RigState, phrase: string): PchiNode | 'ambiguous' | null {
  const p = squash(phrase.replace(/^(?:the|my|our)\s+/i, ''))
  if (!p) return null
  const exact = rig.nodes.filter((n) => squash(n.label) === p)
  if (exact.length === 1) return exact[0]
  const lead = leadingSystem(phrase.replace(/^(?:the|my|our)\s+/i, ''))
  if (lead && !lead.rest) {
    const same = rig.nodes.filter((n) => n.type === lead.type)
    if (same.length === 1) return same[0]
    if (same.length > 1) return 'ambiguous'
  }
  return null
}

function uniqueLabel(rig: RigState, base: string) {
  let label = base
  for (let n = 2; rig.nodes.some((x) => squash(x.label) === squash(label)); n++) label = `${base} ${n}`
  return label
}

export interface SystemResult {
  rig: RigState
  /** Set when the sentence also changed the show (which device a lane uses). */
  chart?: Showchart
  summary: string
  notes: string[]
  /** A sentence to try next, using real names from the show. */
  next?: string
}

/**
 * One sentence about a system, or null when it is not about one (the caller then tries the show
 * commands). Throws CommandError when it clearly is, but cannot be done.
 */
export function parseSystemCommand(text: string, rig: RigState, chart: Showchart, newId: () => string = () => Math.random().toString(36).slice(2, 8)): SystemResult | null {
  let m: RegExpMatchArray | null
  const isScene = (s: string) => chart.states.some((x) => squash(x.id) === squash(s))
  const first = chart.states[0]?.id.toLowerCase() ?? 'intro'

  // ── add ──
  const added = ((): SystemResult | null => {
  if ((m = text.match(new RegExp(`^${VERBS}\\s+(?:a\\s+|an\\s+|the\\s+|my\\s+|our\\s+)?(?:new\\s+)?(?:system\\s+|device\\s+|tool\\s+)?(.+)$`, 'i')))) {
    const verb = m[1].toLowerCase()
    m = [m[0], m[2]] as unknown as RegExpMatchArray
    const count = m[1].match(/^(\d{1,2})\s+(?:x\s*)?(.+)$/)
    if (count && Number(count[1]) >= 2 && leadingSystem(count[2])) {
      const n = Math.min(Number(count[1]), 8)
      let cur = rig
      const summaries: string[] = []
      for (let i = 0; i < n; i++) {
        const r = parseSystemCommand(`add ${count[2].replace(/^(?:a|an)\s+/i, '').replace(/^(.+?)s$/i, (all, x) => (leadingSystem(all) ? all : x))}`, cur, chart, newId)
        if (!r) return null
        cur = r.rig
        summaries.push(r.summary.replace(/^Add /, ''))
      }
      return { rig: cur, summary: `Add ${n} devices: ${summaries.join('; ')}`, notes: ['all start at the same address: set each one with "set <name> host to ..."'] }
    }
    const lead = leadingSystem(m[1])
    // "use qlab for audio" with a QLab already in the rig means "use the one I have", not "add another"
    if (verb === 'use' && lead && rig.nodes.some((n) => n.type === lead.type)) return null
    if (lead && !isScene(m[1])) {
      let rest = lead.rest
      let name: string | undefined
      let host: string | undefined
      let port: number | undefined
      let listen: number | undefined
      let dept: string | undefined
      // consume the pieces we know; anything left over means this is not a system sentence ("add a laser cue ...")
      for (let guard = 0; rest && guard < 8; guard++) {
        let r: RegExpMatchArray | null
        if ((r = rest.match(/^(?:called|named|as)\s+(?:"([^"]+)"|'([^']+)'|(.+?))(?=\s+(?:at|on|@|port|listen|listening|feedback|for|serves?)\b|$)\s*(.*)$/i))) {
          name = (r[1] ?? r[2] ?? r[3]).trim()
          rest = r[4].trim()
        } else if ((r = rest.match(/^(?:at|on|@)\s+(\S+)\s*(.*)$/i)) && HOST.test(r[1])) {
          const hp = r[1].match(/^(.*?)(?::(\d{2,5}))?$/)!
          host = hp[1]
          if (hp[2]) port = Number(hp[2])
          rest = r[2].trim()
        } else if ((r = rest.match(/^port\s+(\d{2,5})\s*(.*)$/i))) {
          port = Number(r[1])
          rest = r[2].trim()
        } else if ((r = rest.match(/^(?:listen(?:ing)?(?:\s+(?:on|port))?|feedback(?:\s+port)?)\s+(\d{2,5})\s*(.*)$/i))) {
          listen = Number(r[1])
          rest = r[2].trim()
        } else if ((r = rest.match(/^(?:for|serves?)\s+(visuals?|video|vj|lighting|lights?|lasers?|audio|sound)\s*(.*)$/i))) {
          dept = DEPT_ALIAS[r[1].toLowerCase()]
          rest = r[2].trim()
        } else return null
      }
      if (rest) return null
      const spec = specOf(lead.type)
      const label = uniqueLabel(rig, name ?? spec.label)
      const params = { ...spec.defaults, ...(host ? { host } : {}), ...(port ? { port } : {}), ...(listen ? { listen_port: listen } : {}) }
      const n = rig.nodes.length
      const node: PchiNode = {
        id: `${lead.type}-${newId()}`,
        type: lead.type,
        label,
        position: { x: 120 + n * 40, y: 120 + (n % 4) * 110 },
        params,
        ...(dept && dept !== rigDept({ type: lead.type }) ? { dept } : {}),
      }
      const where = params.host !== undefined ? `${params.host}${params.port !== undefined ? `:${params.port}` : ''}${listen ? `, listening on ${listen}` : ''}` : ''
      const d = rigDept(node)
      const sample: Record<string, string> = {
        visual: `at 0 seconds scene 1 in ${first}`,
        lighting: `at 0 seconds lighting look 1 in ${first}`,
        laser: `at 0 seconds laser cue sweep in ${first}`,
        audio: `at 0 seconds audio snapshot 1 in ${first}`,
      }
      const notes: string[] = []
      if (params.host === '127.0.0.1') notes.push('its address is 127.0.0.1 (this computer): if the tool runs on another machine, say "set ' + label.toLowerCase() + ' host to 192.168.x.x"')
      if (d === 'laser') notes.push('lasers stay disarmed until you arm them on the Live tab')
      return {
        rig: { ...rig, nodes: [...rig.nodes, node] },
        summary: `Add ${spec.label}${label !== spec.label ? ` "${label}"` : ''}${where ? ` at ${where}` : ''}${d ? `, serving ${d === 'visual' ? 'visuals' : d}` : ''}`,
        notes,
        next: d ? sample[d] : undefined,
      }
    }
  }

  return null
  })()
  if (added) return added

  // ── remove ──
  if ((m = text.match(/^(?:remove|delete|unplug|disconnect)\s+(?:the\s+|my\s+|our\s+)?(?:system\s+|device\s+|tool\s+)?(.+)$/i)) && !isScene(m[1])) {
    const found = findNode(rig, m[1])
    if (found === 'ambiguous') throw new CommandError(`There is more than one "${m[1].trim()}" in the rig.`, `Say which: ${rig.nodes.filter((n) => leadingSystem(m![1])?.type === n.type).map((n) => n.label).join(', ')}`)
    if (found) return { rig: { nodes: rig.nodes.filter((n) => n.id !== found.id), edges: rig.edges.filter((e) => e.source !== found.id && e.target !== found.id) }, summary: `Remove ${found.label} from the rig`, notes: ['lanes that used it will say "removed"; this cannot be undone with Ctrl+Z yet'] }
    if (leadingSystem(m[1])) throw new CommandError(`There is no ${m[1].trim()} in the rig.`, rig.nodes.length ? `The rig has: ${rig.nodes.map((n) => n.label).join(', ')}` : 'The rig is empty: say "add qlab" to start')
  }

  // ── set host / port / listen port ──
  if ((m = text.match(/^(?:set|change|update)\s+(?:the\s+)?(.+?)\s+(host|address|ip|port|listen\s+port|feedback\s+port)\s+(?:to\s+|=\s*)?(\S+)$/i)) && !isScene(m[1])) {
    const found = findNode(rig, m[1])
    if (found === 'ambiguous') throw new CommandError(`There is more than one "${m[1].trim()}" in the rig.`, `Say which: ${rig.nodes.filter((n) => leadingSystem(m![1])?.type === n.type).map((n) => n.label).join(', ')}`)
    if (!found) {
      if (leadingSystem(m[1])) throw new CommandError(`There is no ${m[1].trim()} in the rig.`, 'Say "add ' + m[1].trim().toLowerCase() + '" first')
      return null
    }
    const field = m[2].toLowerCase().replace(/\s+/g, ' ')
    const value = m[3]
    const params = { ...found.params }
    let what: string
    if (field === 'host' || field === 'address' || field === 'ip') {
      const hp = value.match(HOST)
      if (!hp) throw new CommandError(`"${value}" does not look like an address.`, 'Use an IP address or name, for example 192.168.1.20 or 192.168.1.20:7000')
      const [h, p] = value.split(/:(?=\d+$)/)
      params.host = h
      if (p) params.port = Number(p)
      what = `address of ${found.label} to ${h}${p ? `:${p}` : ''}`
    } else {
      if (!/^\d{2,5}$/.test(value) || Number(value) > 65535) throw new CommandError(`"${value}" is not a port number.`, 'A port is a number between 1 and 65535')
      const key = field === 'port' ? 'port' : 'listen_port'
      params[key] = Number(value)
      what = `${key === 'port' ? 'port' : 'listen port'} of ${found.label} to ${value}`
    }
    return { rig: { ...rig, nodes: rig.nodes.map((n) => (n.id === found.id ? { ...n, params } : n)) }, summary: `Set ${what}`, notes: [] }
  }

  // ── which department a device serves ──
  if ((m = text.match(/^(?:make\s+)?(?:the\s+)?(.+?)\s+(?:serves?|serving|for|is for)\s+(visuals?|video|vj|lighting|lights?|lasers?|audio|sound)$/i)) && !isScene(m[1])) {
    const found = findNode(rig, m[1])
    if (found && found !== 'ambiguous') {
      const dept = DEPT_ALIAS[m[2].toLowerCase()]
      const inferred = rigDept({ type: found.type })
      const { dept: _drop, ...rest } = found
      void _drop
      const next: PchiNode = dept === inferred ? rest : { ...rest, dept }
      return { rig: { ...rig, nodes: rig.nodes.map((n) => (n.id === found.id ? next : n)) }, summary: `${found.label} now serves ${dept === 'visual' ? 'visuals' : dept}`, notes: [] }
    }
  }

  // ── which device handles a department: "use FOH desk for audio" / "use qlab for audio in intro" ──
  if ((m = text.match(/^(?:use|send)\s+(?:the\s+)?(.+?)\s+for\s+(?:the\s+)?(visuals?|video|vj|lighting|lights?|lasers?|audio|sound)(?:\s+(?:cues\s+)?(?:in|for)\s+(.+))?$/i))) {
    const found = findNode(rig, m[1])
    if (found === 'ambiguous') throw new CommandError(`There is more than one "${m[1].trim()}" in the rig.`, `Say which: ${rig.nodes.filter((n) => leadingSystem(m![1])?.type === n.type).map((n) => n.label).join(', ')}`)
    if (!found) {
      if (leadingSystem(m[1])) throw new CommandError(`There is no ${m[1].trim()} in the rig.`, `Say "add ${m[1].trim().toLowerCase()}" first`)
      return null
    }
    const dept = DEPT_ALIAS[m[2].toLowerCase()]
    const word = dept === 'visual' ? 'visuals' : dept
    if (rigDept(found) !== dept) throw new CommandError(`${found.label} serves ${rigDept(found) ?? 'no department'}, not ${word}.`, `Say "${found.label.toLowerCase()} for ${word}" first`)
    const only = m[3] ? chart.states.find((s) => squash(s.id) === squash(m![3]) || (s.label && squash(s.label) === squash(m![3]))) : undefined
    if (m[3] && !only) throw new CommandError(`I don't know a scene called "${m[3].trim()}".`, `Scenes: ${chart.states.map((s) => s.id).join(', ') || 'none yet'}`)
    let next = chart
    let lanes = 0
    for (const s of chart.states) {
      if (only && s.id !== only.id) continue
      for (const l of s.timeline?.lanes ?? []) {
        if (l.dept !== dept) continue
        next = setLaneDevice(next, s.id, l.id, found.id)
        lanes++
      }
    }
    if (!lanes) throw new CommandError(`There is no ${word} lane ${only ? `in ${only.id}` : 'in any scene'} yet.`, `Add a cue first, for example: at 0 seconds ${dept === 'audio' ? 'audio snapshot 1' : dept === 'lighting' ? 'lighting look 1' : dept === 'laser' ? 'laser cue sweep' : 'scene 1'} in ${only?.id.toLowerCase() ?? first}`)
    return { rig, chart: next, summary: `${found.label} now handles ${word} ${only ? `in ${only.id}` : 'in every scene'} (${lanes} lane${lanes > 1 ? 's' : ''})`, notes: [] }
  }

  // ── rename a device ──
  if ((m = text.match(/^rename\s+(?:the\s+)?(.+?)\s+to\s+(.+)$/i)) && !isScene(m[1])) {
    const found = findNode(rig, m[1])
    if (found && found !== 'ambiguous') {
      const label = m[2].trim().replace(/^["'“”]+|["'“”]+$/g, '')
      if (rig.nodes.some((n) => n.id !== found.id && squash(n.label) === squash(label))) throw new CommandError(`Something in the rig is already called "${label}".`)
      return { rig: { ...rig, nodes: rig.nodes.map((n) => (n.id === found.id ? { ...n, label } : n)) }, summary: `Rename ${found.label} to ${label}`, notes: [] }
    }
  }
  return null
}

export interface StudioStep {
  summary: string
  /** The show-command op for steps that came from commands.ts; 'system' for rig changes. */
  op: string
}
export type StudioResult =
  | { ok: true; chart: Showchart; rig: RigState; steps: StudioStep[]; notes: string[]; next?: string; rigChanged: boolean }
  | { ok: false; error: string; hint?: string }

/**
 * A sentence about the show OR its systems (or both, separated by ";"), applied together:
 * all of it or none of it. Anything not understood gets the closest working command as a hint.
 */
export function runStudioCommand(text: string, chart: Showchart, rig: RigState, opts: { state?: string; newId?: () => string } = {}): StudioResult {
  const fragments = splitCommandFragments(text)
  if (!fragments.length) return { ok: false, error: 'Type or say a command.', hint: hintFor('', chart, rig.nodes) }
  let w = chart
  let r = rig
  let rigChanged = false
  const steps: StudioStep[] = []
  const notes: string[] = []
  let next: string | undefined
  for (const frag of fragments) {
    try {
      const sys = parseSystemCommand(frag, r, w, opts.newId)
      if (sys) {
        r = sys.rig
        if (sys.chart) w = sys.chart
        else rigChanged = true
        steps.push({ summary: sys.summary, op: 'system' })
        notes.push(...sys.notes.map((n) => `: ${n}`))
        next = sys.next ?? next
        continue
      }
    } catch (e) {
      if (e instanceof CommandError) return { ok: false, error: e.message, hint: e.hint }
      throw e
    }
    const res: CommandResult = runCommandText(frag, w, { state: opts.state })
    if (!res.ok) {
      const unknown = /^I didn't understand/.test(res.error)
      return { ok: false, error: res.error, hint: unknown ? hintFor(frag, w, r.nodes) : res.hint }
    }
    w = res.chart
    steps.push(...res.steps.map((s) => ({ summary: s.summary, op: s.command.op })))
    notes.push(...res.notes)
  }
  return { ok: true, chart: w, rig: r, steps, notes, next, rigChanged }
}

/** The quick-add list for the "＋" menu: the tools PCHI has profiles for, plus generic devices. Each is a command. */
export const QUICK_SYSTEMS: { type: NodeType; label: string; command: string }[] = [
  { type: 'resolume', label: 'Resolume Arena', command: 'add resolume' },
  { type: 'grandma3', label: 'grandMA3', command: 'add grandma3' },
  { type: 'eos', label: 'ETC Eos', command: 'add eos' },
  { type: 'x32', label: 'Behringer X32 / M32', command: 'add x32' },
  { type: 'qlab', label: 'QLab', command: 'add qlab' },
  { type: 'magicq', label: 'ChamSys MagicQ', command: 'add magicq' },
  { type: 'beyond', label: 'Pangolin BEYOND', command: 'add beyond' },
  { type: 'laser', label: 'Laser (generic)', command: 'add laser' },
  { type: 'lighting-desk', label: 'Lighting desk (generic)', command: 'add lighting desk' },
  { type: 'audio-console', label: 'Audio console (generic)', command: 'add audio console' },
]
