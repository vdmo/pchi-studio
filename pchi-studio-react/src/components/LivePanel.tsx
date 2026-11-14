import { useEffect, useMemo, useRef, useState } from 'react'
import { Background, BackgroundVariant, Panel, ReactFlow } from '@xyflow/react'
import { ZoomControls, buildEdges, buildNodes, edgeTypes, nodeTypes } from './showchartGraph'
import Card from './Card'
import CommandBar from './CommandBar'
import TimelineStrip, { useStripLayout } from './TimelineStrip'
import { sceneInfo } from './sceneInfo'
import LiveCallSheet from './LiveCallSheet'
import RuntimeTools, { armLaser, decideProposal, getInputs, signIn, signOut, stopLasers, testFire, tryCue, whoami, type OscInput, type WhoAmI } from './RuntimeTools'
import { STALE_MS, feedUrl, type Ready } from './liveFeed'
import { fmt } from './TimelineStrip'
import {
  DEPTS,
  deptOf,
  LIVE_EXAMPLES,
  parseLiveCommand,
  step,
  timelineEnd,
  timerEvent,
  transitionLabel,
  transitionTrigger,
  type LiveAction,
  type PchiEvent,
  type ShowContext,
  type Showchart,
  requiredFor,
  rigDept,
  type PchiNode,
} from '@pchi/sdk'

/** Cues already due after `elapsed` ms in `state` (used when joining mid-state). */
const dueCues = (def: Showchart, state: string, elapsed: number) =>
  new Set(
    (def.states.find((s) => s.id === state)?.timeline?.lanes ?? [])
      .flatMap((l) => l.cues)
      .filter((c) => c.at_ms <= elapsed)
      .map((c) => c.id),
  )

type Status = 'connecting' | 'online' | 'offline'

interface Props {
  /** Mirrors every feed message into Studio's main event stream. */
  onFeed: (e: PchiEvent) => void
  /** Reports the feed's connection status, so the header can show the right one. */
  onStatus: (s: Status) => void
  /** Devices in Studio's rig, to show which one stands behind each department. */
  rig?: PchiNode[]
  /** The show as Studio has it now (showchart + rig), to send to the runtime. */
  getFile?: () => unknown
  /** Add a cue a tool was heard firing to the show being edited. Returns what happened, in words. */
  onAddCapture?: (c: Capture) => string
}

/** A cue a tool fired by itself (the runtime heard it), and where in the show that was. */
export type Capture = { id: number; ts: number; device: string; label: string; dept: string; kind: string; value: string; address: string; scene: string; at_ms: number; basis: string }

/** A rig device as the running show knows it. */
type Link = { state: 'linked' | 'lost' | 'heard' | 'silent' | 'none'; heard_ms_ago?: number | null; /** when this browser last knew the device spoke (local clock) */ at?: number; /** the runtime asks this tool to answer, so silence means something */ probed?: boolean }
type RunDevice = { id: string; label: string; dept: string; armed?: boolean; link?: Link }
/** Something a device sent back (OSC), newest last. */
type Feedback = { ts: number; device: string; label?: string; address: string; args: unknown[] }
/** What the runtime did with one cue: sent, dry-run, no-device or failed. */
type Delivery = { ts: number; kind: string; status: 'sent' | 'dry-run' | 'no-device' | 'failed' | 'held'; device?: string; label?: string; dept?: string; target?: string; error?: string; lane?: string }

const linkText = (l: Link | undefined, now: number) => {
  if (!l || l.state === 'none') return ''
  const age = l.at ? ` ${Math.max(0, Math.round((now - l.at) / 1000))}s ago` : ''
  return l.state === 'linked' ? ' · ● linked' : l.state === 'lost' ? ` · ○ no reply (last${age})` : l.state === 'heard' ? ` · heard${age}` : ' · waiting to hear'
}

/**
 * Live view of a show running in pchi-studio-api. The server is the source of
 * truth: this panel only renders the feed and sends inputs. It never runs the
 * engine itself, apart from asking "which events could fire from here" to
 * enable the right buttons.
 */
export default function LivePanel({ onFeed, onStatus, rig = [], getFile, onAddCapture }: Props) {
  const [status, setStatus] = useState<Status>('connecting')
  const [def, setDef] = useState<Showchart | null>(null)
  const [state, setState] = useState('')
  const [ctx, setCtx] = useState<ShowContext>({})
  // When the current state was entered, on THIS machine's clock. The server
  // sends elapsed time (not a timestamp), so clock skew between the two
  // machines cannot throw the countdown off.
  const [enteredAt, setEnteredAt] = useState(() => Date.now())
  const [now, setNow] = useState(() => Date.now())
  // cue ids the server has fired in the current state
  const [fired, setFired] = useState<Set<string>>(new Set())
  const onStatusRef = useRef(onStatus)
  onStatusRef.current = onStatus
  useEffect(() => onStatusRef.current(status), [status])
  // what each department has said: standing by, on hold, or nothing yet
  const [readiness, setReadiness] = useState<Record<string, Ready>>({})
  // the rig the running show was started with, and what became of each cue sent to it
  const [runDevices, setRunDevices] = useState<RunDevice[] | null>(null)
  const [output, setOutput] = useState<'live' | 'dry-run' | null>(null)
  const [last, setLast] = useState<Record<string, Delivery>>({})
  const [gaps, setGaps] = useState<Delivery[]>([]) // cues no device could take
  const [beat, setBeat] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setBeat(Date.now()), 5000)
    return () => window.clearInterval(id)
  }, [])
  const [view, setView] = useState<'operate' | 'calls'>('operate')
  const [testing, setTesting] = useState<string | null>(null)
  const [heard, setHeard] = useState<Feedback[]>([])
  const [laserNote, setLaserNote] = useState('')
  const [tryDev, setTryDev] = useState('')
  const [tryVal, setTryVal] = useState('')
  const [tryOut, setTryOut] = useState('')
  const [locked, setLocked] = useState(false) // the runtime wants an operator key for edits
  const [undoDepth, setUndoDepth] = useState(0) // sends that can still be undone
  const [proposals, setProposals] = useState<{ id: number; actor: string; status: string; changes: string[]; notes: string[]; said?: string; rehearsal?: { different: boolean; verdict: string; lines: string[] } | null }[]>([]) // edits an agent proposed
  const [governed, setGoverned] = useState(false) // the runtime writes signed receipts
  const [captures, setCaptures] = useState<Capture[]>([]) // cues the tools fired themselves
  const [gone, setGone] = useState<Set<number>>(new Set()) // captures added or dismissed
  const [capNote, setCapNote] = useState('')
  const [tc, setTc] = useState<{ state: string; position: string | null; fps: string | null; source: string } | null>(null) // timecode being heard
  const [oscIn, setOscIn] = useState<OscInput[]>([]) // OSC inputs listening for button presses
  const [me, setMe] = useState<WhoAmI | null>(null) // who the runtime takes this tab to be
  const [authTick, setAuthTick] = useState(0) // bumped on sign in / out: the feed reconnects so the runtime knows who is pressing GO
  const [signErr, setSignErr] = useState('')
  const [decideErr, setDecideErr] = useState('')
  const [armed, setArmed] = useState(false) // GO pressed once while a department is on hold
  const [trail, setTrail] = useState<{ ts: number; text: string }[]>([])
  const [key, setKey] = useState('energy')
  const [val, setVal] = useState('0.3')
  const ws = useRef<WebSocket>()
  const onFeedRef = useRef(onFeed)
  onFeedRef.current = onFeed

  useEffect(() => {
    let closed = false
    let timer: number | undefined
    const connect = () => {
      setStatus('connecting')
      const s = new WebSocket(feedUrl())
      ws.current = s
      s.onopen = () => setStatus('online')
      s.onmessage = (m) => {
        let msg: PchiEvent
        try {
          msg = JSON.parse(String(m.data))
        } catch {
          return
        }
        onFeedRef.current(msg)
        switch (msg.type) {
          case 'hello':
            {
              // how old each report is, by the server's own clock: this browser's clock may differ
              const serverNow = Number(msg.server_ts ?? 0)
              const here = Date.now()
              const raw = (msg.readiness as Record<string, Omit<Ready, 'seen'>>) ?? {}
              setReadiness(Object.fromEntries(Object.entries(raw).map(([k, v]) => [k, { ...v, seen: serverNow ? here - Math.max(0, serverNow - v.ts) : here }])))
            }
            setRunDevices(Array.isArray(msg.devices) ? (msg.devices as RunDevice[]).map((x) => (x.link?.heard_ms_ago != null ? { ...x, link: { ...x.link, at: Date.now() - x.link.heard_ms_ago } } : x)) : null)
            setOutput(msg.output === 'live' || msg.output === 'dry-run' ? msg.output : null)
            setLast({})
            setGaps([])
            setHeard([])
            setLocked(!!msg.edit_locked)
            setUndoDepth(Number(msg.undo_depth ?? 0))
            setGoverned(!!msg.governed)
            setOscIn(Array.isArray(msg.inputs) ? (msg.inputs as OscInput[]) : [])
            setTc((msg.timecode as typeof tc) ?? null)
            setCaptures(Array.isArray(msg.captures) ? (msg.captures as Capture[]) : [])
            setProposals(Array.isArray(msg.proposals) ? (msg.proposals as typeof proposals) : [])
            setDef(msg.definition as Showchart)
            setState(String(msg.state))
            setCtx((msg.context as ShowContext) ?? {})
            setEnteredAt(Date.now() - Number(msg.elapsed_ms ?? 0))
            // joined mid-state: the cues already due must have fired
            setFired(dueCues(msg.definition as Showchart, String(msg.state), Number(msg.elapsed_ms ?? 0)))
            break
          case 'action':
            if (typeof msg.cue === 'string') setFired((f) => new Set(f).add(String(msg.cue)))
            break
          case 'transition':
            if (msg.applied) {
              setState(String(msg.to))
              setEnteredAt(Date.now())
              setFired(new Set())
              setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `${msg.event}  ${msg.from} → ${msg.to}  (${msg.source})` }])
            } else {
              setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `${msg.event} ignored in ${msg.from}  (${msg.source})` }])
            }
            break
          case 'readiness':
            setReadiness((r) => ({ ...r, [String(msg.dept)]: { ready: !!msg.ready, note: String(msg.note ?? ''), ts: Number(msg.ts), source: String(msg.source ?? ''), seen: Date.now() } }))
            break
          case 'delivery': {
            const d = msg as unknown as Delivery
            if (d.device) setLast((l) => ({ ...l, [d.device!]: d }))
            else if (d.status === 'no-device') setGaps((g) => [...g.filter((x) => !(x.dept === d.dept && x.kind === d.kind)), d].slice(-8))
            break
          }
          case 'link':
            setRunDevices((ds) => ds && ds.map((x) => (x.id === msg.device ? { ...x, link: { ...x.link, state: msg.state as Link['state'], heard_ms_ago: msg.heard_ms_ago as number | null, at: msg.heard_ms_ago != null ? Date.now() - Number(msg.heard_ms_ago) : x.link?.at } } : x)))
            break
          case 'feedback':
            setHeard((h) => [...h.slice(-7), msg as unknown as Feedback])
            setRunDevices((ds) => ds && ds.map((x) => (x.id === msg.device && x.link ? { ...x, link: { ...x.link, at: Date.now() } } : x)))
            break
          case 'arming':
            setRunDevices((ds) => ds && ds.map((x) => (x.id === msg.device ? { ...x, armed: !!msg.armed } : x)))
            setLast((l) => (l[String(msg.device)]?.status === 'held' ? Object.fromEntries(Object.entries(l).filter(([k]) => k !== String(msg.device))) : l)) // a stale "held" no longer applies
            break
          case 'show':
            setDef(msg.definition as Showchart)
            setUndoDepth(Number(msg.undo_depth ?? 0))
            setRunDevices(Array.isArray(msg.devices) ? (msg.devices as RunDevice[]) : null)
            break
          case 'proposal': {
            const p = msg.proposal as (typeof proposals)[number] | null
            if (p) setProposals((ps) => [...ps.filter((x) => x.id !== p.id), p].sort((a, b) => a.id - b.id))
            break
          }
          case 'capture':
            setCaptures((cs) => [...cs.filter((c) => c.id !== Number(msg.id)), msg as unknown as Capture].slice(-30))
            break
          case 'timecode':
            setTc({ state: String(msg.state), position: (msg.position as string) ?? null, fps: (msg.fps as string) ?? null, source: String(msg.source ?? '') })
            break
          case 'tc-jump':
            setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `timecode jumped ${(Number(msg.from_ms) / 1000).toFixed(1)} → ${(Number(msg.to_ms) / 1000).toFixed(1)} s in ${msg.scene}${Number(msg.skipped) ? `: ${msg.skipped} cue(s) jumped over, not fired` : ''}` }])
            break
          case 'held':
            setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `${msg.event} held in ${msg.scene}: ${(Number(msg.remaining_ms) / 1000).toFixed(1)} s more (${msg.source})` }])
            break
          case 'context':
            setCtx((c) => ({ ...c, [String(msg.key)]: msg.value }))
            break
        }
      }
      s.onclose = () => {
        setStatus('offline')
        if (!closed) timer = window.setTimeout(connect, 2000)
      }
    }
    connect()
    return () => {
      closed = true
      window.clearTimeout(timer)
      ws.current?.close()
    }
  }, [authTick])

  useEffect(() => {
    if (status === 'online') whoami().then(setMe)
  }, [status, authTick])

  // the OSC inputs' counters change without any message on the feed: look again now and then
  useEffect(() => {
    if (status === 'online' && oscIn.length) getInputs().then((i) => i.length && setOscIn(i))
  }, [status, beat, oscIn.length])

  // timed transitions leaving the current state, with whether each would fire right now
  const timerRows = useMemo(
    () =>
      (def?.transitions ?? [])
        .filter((t) => t.source === state && t.after_ms !== undefined)
        .map((t) => ({
          ms: t.after_ms!,
          target: t.target,
          guard: t.guard,
          open: !!def && step(def, ctx, state, timerEvent(t.after_ms!))?.target === t.target,
        })),
    [def, state, ctx],
  )

  // the running state's timeline, if it has one
  const tl = def?.states.find((s) => s.id === state)?.timeline
  const lanes = tl?.lanes ?? []
  const elapsed = Math.max(0, now - enteredAt)
  const exit = def && state ? sceneInfo(def, state).exit : undefined
  const tlTotal = Math.max(timelineEnd(tl) + 1000, 5000, exit ? exit.ms + 500 : 0)
  const strip = useStripLayout(lanes.length)
  const tlHeight = def && state ? strip.h : 0

  // tick only while there is something to count down or a timeline is still playing
  const counting = lanes.some((l) => l.cues.some((c) => !fired.has(c.id))) || timerRows.some((r) => now - enteredAt <= r.ms + 500) || (lanes.length > 0 && elapsed <= tlTotal + 500)
  useEffect(() => {
    if (!counting) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), 100)
    return () => window.clearInterval(id)
  }, [counting, enteredAt])

  const send = (m: PchiEvent) => ws.current?.readyState === WebSocket.OPEN && ws.current.send(JSON.stringify(m))

  const liveApi = useMemo(
    () => ({
      run: (text: string) => parseLiveCommand(text, def, state, ctx),
      apply: (a: LiveAction) =>
        send(
          a.kind === 'event'
            ? { type: 'input', event: a.event }
            : a.kind === 'reset'
              ? { type: 'reset' }
              : a.kind === 'ready'
                ? { type: 'ready', dept: a.dept, ready: a.ready, note: 'said in Studio' }
                : { type: 'context', key: a.key, value: a.value },
        ),
      examples: LIVE_EXAMPLES,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [def, state, ctx],
  )

  const nodes = useMemo(() => (def ? buildNodes(def, state, false) : []), [def, state])
  const edges = useMemo(() => (def ? buildEdges(def, state) : []), [def, state])

  const available = (def?.transitions ?? []).filter((t) => t.source === state)

  // ---- what happens next, and the GO button that does it ----
  // GO takes the first way out that would actually fire now; the others stay available as smaller buttons.
  const goOptions = available.filter((t) => t.event !== undefined && !!def && step(def, ctx, state, t.event)?.target === t.target)
  const go = goOptions[0]
  // the scene's own minimum: until it is met a press is held by the runtime (and only the clock here is shown)
  const sceneMin = def?.states.find((x) => x.id === state)?.min_ms
  const minLeft = sceneMin ? Math.max(0, sceneMin - (now - enteredAt)) : 0
  const autoNext = timerRows.filter((r) => r.open).sort((a, b) => a.ms - b.ms)[0]
  const autoLeft = autoNext ? Math.max(0, autoNext.ms - elapsed) : undefined
  const nextCue = lanes
    .flatMap((l) => l.cues.map((c) => ({ ...c, lane: l.name || l.id, dept: l.dept })))
    .filter((c) => !fired.has(c.id))
    .sort((a, b) => a.at_ms - b.at_ms)[0]
  // One chip per department, or per device when the rig has devices for it. A device's own
  // report counts first; with none, the department's word stands for it.
  const stale = (r?: Ready) => !!r && r.source === 'dept-page' && beat - r.seen > STALE_MS
  const devices: RunDevice[] = runDevices ?? rig.flatMap((n) => { const d = rigDept(n); return d ? [{ id: n.id, label: n.label, dept: d }] : [] })
  const boardDepts = [...new Set([...DEPTS.filter((d) => d.id !== 'other').map((d) => d.id as string), ...Object.keys(readiness).filter((k) => !devices.some((x) => x.id === k))])]
  const live = (r?: Ready) => (stale(r) ? undefined : r)
  const board = boardDepts.flatMap((dept) => {
    const own = devices.filter((x) => x.dept === dept)
    if (own.length === 0) return [{ key: dept, dept, label: deptOf(dept).label, device: undefined as RunDevice | undefined, r: live(readiness[dept]) }]
    return own.map((x) => ({ key: x.id, dept, label: x.label, device: x as RunDevice | undefined, r: live(readiness[x.id]) ?? live(readiness[dept]) }))
  })
  // Only the departments the NEXT scene has cues for can hold up the GO; the others are shown but do not warn.
  const nextScene = (go ?? autoNext)?.target
  const needed = def ? requiredFor(def, nextScene, devices.map((d) => ({ id: d.id, label: d.label, dept: d.dept, type: 'cue' as const, position: { x: 0, y: 0 }, params: {} }))) : new Set<string>()
  const onHold = board.filter((b) => b.r && !b.r.ready && needed.has(b.key)).map((b) => b.label)
  // a needed device that should answer and does not is worth a second look too (it never blocks)
  const noReply = board.filter((b) => b.device && needed.has(b.key) && (b.device.link?.state === 'lost' || (b.device.link?.state === 'silent' && b.device.link.probed))).map((b) => `${b.label} (no reply)`)
  const heldElsewhere = board.filter((b) => b.r && !b.r.ready && !needed.has(b.key)).map((b) => b.label)

  const pressGo = () => {
    if (!go) return
    if (onHold.length + noReply.length > 0 && !armed) {
      setArmed(true) // a second press within a few seconds goes anyway
      window.setTimeout(() => setArmed(false), 4000)
      return
    }
    setArmed(false)
    send({ type: 'input', event: go.event! })
  }
  const goRef = useRef(pressGo)
  goRef.current = pressGo
  // Space is GO, unless the cursor is in a box or on a button (where Space already means something)
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (e.code !== 'Space' || e.repeat || (el && /^(INPUT|TEXTAREA|SELECT|BUTTON)$/.test(el.tagName)) || el?.isContentEditable) return
      e.preventDefault()
      goRef.current()
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [])

  const setReady = (dept: string, ready: boolean) => send({ type: 'ready', dept, ready, note: ready ? 'set in Studio' : 'held in Studio' })

  return (
    <div className={`cv has-right ${tlHeight ? 'has-tl' : ''}`} style={{ ['--tl-h' as string]: `${tlHeight}px` }}>
      {def && tlHeight > 0 && <TimelineStrip state={state} tl={tl} elapsed={elapsed} fired={fired} exit={exit} layout={strip} />}
      {def ? (
        <div className="cv-canvas">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          nodeOrigin={[0.5, 0.5]}
          nodesConnectable={false}
          nodesDraggable={false}
          colorMode="dark"
          minZoom={0.2}
          fitView
          fitViewOptions={{ padding: 0.18 }}
          key={def.id ?? 'show'}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} color="#2b2e3a" />
          <Panel position="top-center" className="p-palette">
            <CommandBar live={liveApi} placeholder={`Operate the show, e.g. "go to ${(available.find((t) => t.event)?.target ?? 'drop').toLowerCase()}"`} />
          </Panel>
          <Panel position="top-left" className="p-crumb">
            <span className="crumb">
              <span className="muted">root</span> <span aria-hidden>›</span> <b>{def.name ?? def.id ?? 'show'}</b>
              {def.version ? <span className="muted"> v{def.version}</span> : null}
            </span>
            <span className={`pill ${status}`} role="status">feed: {status}</span>
          </Panel>
          <Panel position="bottom-left">
            <ZoomControls />
          </Panel>
        </ReactFlow>
        </div>
      ) : (
        <p className="muted pad">
          <span className={`pill ${status}`} role="status">feed: {status}</span>{' '}
          {status === 'online' ? 'Waiting for the show…' : 'Not connected. Save your show from Studio (Save), then start the runtime on that file:  pchi-studio-api my-show.pchi  (port 3100; add --send to really send cues to the rig). This page reconnects by itself.'}
        </p>
      )}

      <Card title="Show" className="right">
        <p className="bigstate">{state || '—'}</p>
        <div className="seg lv-seg" role="group" aria-label="Live view">
          <button className={view === 'operate' ? 'on' : ''} aria-pressed={view === 'operate'} onClick={() => setView('operate')}>Operate</button>
          <button className={view === 'calls' ? 'on' : ''} aria-pressed={view === 'calls'} onClick={() => setView('calls')}>Call sheet</button>
        </div>

        <div className="nextup" aria-live="off">
          <span className="muted">Next up</span>
          {go ? (
            <b>{go.target}</b>
          ) : autoNext ? (
            <b>{autoNext.target}</b>
          ) : (
            <b className="muted">nothing: the show ends here</b>
          )}
          {autoLeft !== undefined && !go && <span className="nu-time">{autoLeft > 0 ? fmt(autoLeft) : 'now'}</span>}
          {autoLeft !== undefined && go && <span className="muted">or automatically in {fmt(autoLeft)}</span>}
          {nextCue && (
            <span className="muted nu-cue">
              next cue: {nextCue.label ?? nextCue.action.kind} · {nextCue.lane} · {nextCue.at_ms > elapsed ? `in ${fmt(nextCue.at_ms - elapsed)}` : 'due'}
            </span>
          )}
        </div>
        <button className={`go ${armed ? 'armed' : onHold.length ? 'warn' : ''}`} disabled={!go} onClick={pressGo} title="Send the next event (Space)">
          {!go ? (autoNext ? 'WAITING' : 'GO') : armed ? `GO ANYWAY → ${go.target}` : `GO → ${go.target}`}
          <small>{go && minLeft > 0 ? `held for ${fmt(minLeft)}: ${state} must run ${fmt(sceneMin ?? 0)} first` : !go ? (autoNext ? `moves on by itself in ${fmt(autoLeft ?? 0)}` : 'nothing to send') : onHold.length + noReply.length ? `${[...onHold, ...noReply].join(', ')} needed in ${nextScene}${armed ? ': press again to go' : ''}` : heldElsewhere.length ? `Space · ${heldElsewhere.join(', ')} on hold but not needed in ${nextScene}` : 'Space'}</small>
        </button>

        {view === 'calls' && def && (
          <LiveCallSheet
            def={def}
            scene={state}
            elapsed={elapsed}
            fired={fired}
            devices={devices.map((d) => ({ id: d.id, label: d.label, dept: d.dept, type: 'cue', position: { x: 0, y: 0 }, params: {} }))}
            nextScene={(go ?? autoNext)?.target}
          />
        )}
        <h5>
          Departments{' '}
          {output && (
            <span className={`out-badge ${output}`} title={output === 'live' ? 'The runtime is sending cues to the rig devices (UDP/OSC)' : 'Dry run: nothing leaves the runtime. Start it with --send to really send cues.'}>
              {output === 'live' ? 'sending' : 'dry run'}
            </span>
          )}
        </h5>
        <div className="ready-board">
          {board.map(({ key, dept, label, device, r }) => {
            const d = deptOf(dept)
            const st = !r ? 'nolink' : r.ready ? 'ready' : 'hold'
            const dv = device ? last[device.id] : undefined
            const usedHere = (def?.states ?? []).some((s) => (s.timeline?.lanes ?? []).some((l) => l.dept === dept && l.cues.length > 0))
            const nodev = !device && devices.length > 0 && usedHere
            return (
              <button
                key={key}
                className={`ready-chip ${st} ${device?.armed ? 'armed' : ''} ${device?.link?.state === 'lost' ? 'nolinkdev' : ''} ${nodev ? 'nodev' : ''} ${dv?.status === 'failed' ? 'failed' : ''} ${needed.has(key) ? 'needed' : ''}`}
                onClick={() => setReady(key, !r || !r.ready ? true : false)}
                title={`${device ? `${label} (${d.label})` : d.label}${nodev ? ' (no device in the rig)' : ''}: ${st === 'nolink' ? 'has not reported' : st === 'ready' ? 'standing by' : 'on hold'}${r?.note ? ' – ' + r.note : ''}${r?.source ? ` (via ${r.source})` : ''}${dv ? `. Last cue: ${dv.status}${dv.error ? ' – ' + dv.error : ''}` : ''}. Click to ${r?.ready ? 'hold' : 'mark ready'}.`}
              >
                <i style={{ background: d.color }} aria-hidden />
                <span>
                  {label}
                  {device && <small className="dev">{d.label}{device.dept === 'laser' ? (device.armed ? ' · ARMED' : ' · disarmed') : ''}{linkText(device.link, beat)}{dv ? ` · ${dv.status === 'failed' ? '⚠ failed' : dv.status}` : ''}</small>}
                  {nodev && <small className="dev">⚠ no device</small>}
                  {needed.has(key) && nextScene && <small className="dev need">needed in {nextScene}</small>}
                </span>
                <b>{st === 'nolink' ? 'no link' : st === 'ready' ? 'READY' : 'HOLD'}</b>
              </button>
            )
          })}
        </div>
        {heard.length > 0 && (
          <details className="heard">
            <summary>Heard from devices ({heard.length})</summary>
            <ul>
              {[...heard].reverse().map((h, i) => (
                <li key={i}>
                  <b>{h.label ?? h.device}</b> {h.address} <span className="muted">{h.args.map((a) => JSON.stringify(a)).join(' ')}</span>
                </li>
              ))}
            </ul>
          </details>
        )}
        {(gaps.length > 0 || Object.values(last).some((l) => l.status === 'failed')) && (
          <ul className="delivery-warn" role="status">
            {gaps.map((g, i) => (
              <li key={`g${i}`}>⚠ {g.kind}: {g.error}</li>
            ))}
            {Object.values(last).filter((l) => l.status === 'failed').map((l) => (
              <li key={l.device}>⚠ {l.label ?? l.device}: {l.error}</li>
            ))}
          </ul>
        )}
        {devices.some((x) => x.dept === 'laser') && (
          <div className="laser-bar" role="group" aria-label="Laser safety">
            <button
              className="stop-lasers"
              onClick={async () => {
                const r = await stopLasers()
                setLaserNote(r.ok ? `Lasers stopped and disarmed.${r.notes.length ? ' ' + r.notes.join(' ') : ''}` : r.error ?? 'Stop failed')
              }}
              title="Disarm every laser and tell the laser software to black out and disable output"
            >
              STOP LASERS
            </button>
            {devices.filter((x) => x.dept === 'laser').map((x) => (
              <button
                key={x.id}
                className={`dev-arm ${x.armed ? 'on' : ''}`}
                onClick={async () => {
                  if (x.armed) {
                    const r = await armLaser(x.id, false)
                    setLaserNote(r.ok ? `${x.label} disarmed.` : r.error ?? 'Failed')
                    return
                  }
                  if (!confirm(`Arm ${x.label}?\n\nCues in the show will be sent to this laser. Arming does NOT switch the laser's output on: that stays in the laser software.\nCheck the area is clear.`)) return
                  const r = await armLaser(x.id, true)
                  setLaserNote(r.ok ? `${x.label} armed.` : r.error ?? 'Failed')
                }}
              >
                {x.armed ? `Disarm ${x.label}` : `Arm ${x.label}`}
              </button>
            ))}
            {laserNote && <span className="muted" role="status">{laserNote}</span>}
          </div>
        )}
        {devices.length > 0 && (
          <div className="dev-tests">
            {devices.map((x) => (
              <button
                key={x.id}
                className="dev-test"
                disabled={testing === x.id}
                title={output === 'live' ? 'Send one /pchi/test message to this device' : 'Dry run: reports where the test would go'}
                onClick={async () => {
                  if (output === 'live' && !confirm(`Send a test message to ${x.label}?${x.dept === 'laser' ? ' This is a laser: make sure the area is clear and the safety shutter is in your control.' : ''}`)) return
                  setTesting(x.id)
                  const r = await testFire(x.id)
                  setTesting(null)
                  setLast((l) => ({ ...l, [x.id]: { ts: Date.now(), kind: 'test', status: r.status as Delivery['status'], device: x.id, label: x.label, dept: x.dept, error: r.error } }))
                }}
              >
                Test {x.label}
              </button>
            ))}
          </div>
        )}
        {devices.length > 0 && (
          <form
            className="try-cue"
            onSubmit={async (e) => {
              e.preventDefault()
              const id = tryDev || devices[0].id
              if (!tryVal.trim()) return
              const dev = devices.find((x) => x.id === id)
              if (output === 'live' && !confirm(`Send "${tryVal.trim()}" to ${dev?.label} now?${dev?.dept === 'laser' ? ' It is a laser: check the area is clear.' : ''}`)) return
              setTryOut('sending…')
              const r = await tryCue(id, tryVal.trim())
              setTryOut(r.ok ? `→ ${r.sentAs} (${r.status}${r.error ? ': ' + r.error : ''})` : `✗ ${r.error}`)
            }}
          >
            <h5>Try a cue</h5>
            <div className="trow2">
              <select value={tryDev || devices[0].id} onChange={(e) => setTryDev(e.target.value)} aria-label="Device to send to">
                {devices.map((x) => (
                  <option key={x.id} value={x.id}>{x.label}</option>
                ))}
              </select>
              <input value={tryVal} onChange={(e) => setTryVal(e.target.value)} placeholder="e.g. 3 or 3/9" aria-label="Cue value" spellCheck={false} />
              <button type="submit" disabled={!tryVal.trim()}>Send</button>
            </div>
            {tryOut && <p className="muted try-out" role="status">{tryOut}</p>}
          </form>
        )}
        {getFile && <RuntimeTools getFile={getFile} locked={locked} undoDepth={undoDepth} />}
        {(locked || me?.signed_in) && (
          <p className="muted gov-badge who" role="status">
            {me?.signed_in ? (
              <>
                Signed in as <b>{me.name}</b>
                {me.can.length ? ' · may ' + me.can.join(', ') : ' · look only'}{' '}
                <button className="ghost" onClick={() => { signOut(); setMe(null); setAuthTick((t) => t + 1) }}>Sign out</button>
              </>
            ) : (
              <>
                Not signed in{' '}
                <button
                  onClick={async () => {
                    const k = window.prompt('Your key (from whoever runs the show):')
                    if (!k) return
                    const who = await signIn(k)
                    setSignErr(who ? '' : 'That key is not known to this runtime.')
                    if (who) { setMe(who); setAuthTick((t) => t + 1) }
                  }}
                >
                  Sign in
                </button>
              </>
            )}
            {signErr && <span role="alert"> {signErr}</span>}
          </p>
        )}
        {onAddCapture && captures.filter((c) => !gone.has(c.id)).length > 0 && (
          <section className="captures" aria-label="Cues the tools fired">
            <h4>Fired on the tools</h4>
            {captures
              .filter((c) => !gone.has(c.id))
              .slice(-5)
              .reverse()
              .map((c) => (
                <div key={c.id} className="capture">
                  <span>
                    <b>{c.label}</b> fired {c.kind.split('.')[0]} <b>{c.value}</b> · {c.scene} at {fmt(c.at_ms)}
                    {c.basis !== 'vendor' && <span className="muted" title="The rule that read this message rests on a community source, not the vendor's documentation"> · {c.basis} source</span>}
                  </span>
                  <button
                    onClick={() => {
                      setCapNote(onAddCapture(c))
                      setGone((g) => new Set(g).add(c.id))
                    }}
                  >
                    Add to show
                  </button>
                  <button className="ghost" aria-label="Dismiss" onClick={() => setGone((g) => new Set(g).add(c.id))}>×</button>
                </div>
              ))}
            {capNote && <p className="muted" role="status">{capNote}</p>}
          </section>
        )}
        {(() => {
          const lockedTc = def?.states.find((x) => x.id === state)?.timeline?.tc_start
          const usesTc = !!def && (def.states.some((x) => x.timeline?.tc_start) || def.transitions.some((x) => x.event?.startsWith('tc:')))
          if (!usesTc && (!tc || tc.state === 'none')) return null
          return (
            <p className={`muted gov-badge ${usesTc && tc?.state !== 'running' ? 'tc-warn' : ''}`} role="status" title="Cues locked to timecode fire as the timecode passes them; a jump fires nothing">
              Timecode · {!tc || tc.state === 'none' ? 'none heard yet' : `${tc.position ?? '?'} · ${tc.fps} fps · ${tc.state}${tc.source ? ` · from ${tc.source}` : ''}`}
              {lockedTc ? ` · ${state} is locked to ${lockedTc}` : ''}
            </p>
          )
        })()}
        {oscIn.map((i) => (
          <p key={i.name} className="muted gov-badge" title="Presses arriving by OSC. It can only press GO and the show's own events, set a value, or hit STOP LASERS.">
            OSC in · <b>{i.name}</b> {i.listen.split(':').pop() ? ':' + i.listen.split(':').pop() : ''} · {i.bindings} binding{i.bindings === 1 ? '' : 's'} ·{' '}
            {i.heard_ms_ago === null ? 'nothing heard yet' : `last ${i.last_address} ${Math.round(i.heard_ms_ago / 1000)} s ago`} · {i.pressed} pressed
            {i.unlisted_sender + i.dropped + i.refused > 0 && <span> · ignored {i.unlisted_sender} from other senders, {i.dropped} over the rate cap, {i.refused} refused</span>}
          </p>
        ))}
        {governed && <p className="muted gov-badge" title="Every edit, arming, GO and refusal is written to a signed, hash-chained log you can verify offline">Signed receipts are on</p>}
        {proposals.some((p) => p.status === 'pending') && (
          <section className="proposals" aria-label="Edits waiting for a person">
            <h4>Waiting for you</h4>
            {proposals
              .filter((p) => p.status === 'pending')
              .map((p) => (
                <div key={p.id} className="proposal">
                  <p>
                    <b>{p.actor}</b> proposes an edit
                  </p>
                  {p.said && <p className="said">“{p.said}”</p>}
                  <ul>
                    {p.changes.map((c, i) => (
                      <li key={i}>{c}</li>
                    ))}
                    {p.notes.map((c, i) => (
                      <li key={'n' + i} className="muted">{c}</li>
                    ))}
                  </ul>
                  {p.rehearsal && (
                    <div className="rehearsal-note">
                      <b>Against the run so far:</b> {p.rehearsal.verdict}
                      {p.rehearsal.lines.length > 0 && (
                        <ul>
                          {p.rehearsal.lines.map((l, i) => (
                            <li key={i}>{l}</li>
                          ))}
                        </ul>
                      )}
                      <span className="muted"> (worked out when it was proposed)</span>
                    </div>
                  )}
                  <div className="proposal-actions">
                    <button
                      onClick={async () => {
                        const r = await decideProposal(p.id, 'approve')
                        setDecideErr(r.ok ? '' : (r.error ?? 'Could not apply it.'))
                      }}
                    >
                      Approve and apply
                    </button>
                    <button
                      className="ghost"
                      onClick={async () => {
                        const r = await decideProposal(p.id, 'reject')
                        setDecideErr(r.ok ? '' : (r.error ?? 'Could not reject it.'))
                      }}
                    >
                      Reject
                    </button>
                  </div>
                </div>
              ))}
            {decideErr && <p className="muted" role="alert">{decideErr}</p>}
          </section>
        )}
        {timerRows.map((r, i) => {
          const elapsed = Math.max(0, now - enteredAt)
          const left = Math.max(0, r.ms - elapsed)
          const note = left > 0 ? `${(left / 1000).toFixed(1)} s` : r.open ? 'due' : 'skipped'
          return (
            <div key={i} className={`timer ${r.open ? '' : 'blocked'}`} title={r.open ? '' : `guard '${r.guard}' is not satisfied: this timer will not move the show`}>
              <div className="timer-head">
                <span>⏱ after {r.ms / 1000} s → {r.target}</span>
                <b>{note}</b>
              </div>
              <div className="timer-bar">
                <div className="timer-fill" style={{ width: `${Math.min(100, (elapsed / r.ms) * 100)}%` }} />
              </div>
              {!r.open && <span className="muted">guard {r.guard} blocks it</span>}
            </div>
          )
        })}
        {available.filter((t) => t !== go).length > 0 && <h5>Other ways on</h5>}
        {available.length === 0 && <p className="muted">No events leave this state.</p>}
        {available.filter((t) => t !== go).map((t, i) => {
          const trig = transitionTrigger(t)
          const ok = def && trig !== undefined && step(def, ctx, state, trig)?.target === t.target
          return (
            <button key={i} className="event-btn" disabled={!ok || t.event === undefined} onClick={() => t.event && send({ type: 'input', event: t.event })} title={t.event === undefined ? 'timed: fires on its own' : ok ? '' : 'guard not satisfied'}>
              <span>▶ {transitionLabel(t)}</span>
              <span className="muted">→ {t.target}</span>
            </button>
          )
        })}
        <button onClick={() => send({ type: 'reset' })}>↺ Restart show</button>
        <h5>Transitions</h5>
        <div className="trail">
          {trail.length === 0 && <p className="muted">None yet.</p>}
          {[...trail].reverse().map((t, i) => (
            <div key={i} className="muted">
              {new Date(t.ts).toLocaleTimeString()} {t.text}
            </div>
          ))}
        </div>
        <h5>Context</h5>
        <pre className="ctx">{JSON.stringify(ctx, null, 1)}</pre>
        <label>
          Set a value
          <div className="trow2">
            <input value={key} onChange={(e) => setKey(e.target.value)} placeholder="key" />
            <input
              value={val}
              onChange={(e) => setVal(e.target.value)}
              placeholder="value"
              onKeyDown={(e) => {
                if (e.key !== 'Enter') return
                let v: unknown = val
                try {
                  v = JSON.parse(val)
                } catch {
                  /* plain text */
                }
                send({ type: 'context', key, value: v })
              }}
            />
          </div>
          <span className="muted">Enter to send</span>
        </label>

      </Card>
    </div>
  )
}
