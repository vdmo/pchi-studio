import { useEffect, useMemo, useRef, useState } from 'react'
import { Background, BackgroundVariant, Panel, ReactFlow } from '@xyflow/react'
import { ZoomControls, buildEdges, buildNodes, edgeTypes, nodeTypes } from './showchartGraph'
import Card from './Card'
import CommandBar from './CommandBar'
import TimelineStrip, { stripHeight } from './TimelineStrip'
import { sceneInfo } from './sceneInfo'
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
}

/** The runtime may require a token when it is shared over the network: Studio reads it from this browser's storage. */
const feedToken = () => {
  try {
    return localStorage.getItem('pchi.token') ?? ''
  } catch {
    return ''
  }
}
const feedUrl = () => {
  const t = feedToken()
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws${t ? `?token=${encodeURIComponent(t)}` : ''}`
}

type Ready = { ready: boolean; note?: string; ts: number; source?: string }

/**
 * Live view of a show running in pchi-studio-api. The server is the source of
 * truth: this panel only renders the feed and sends inputs. It never runs the
 * engine itself, apart from asking "which events could fire from here" to
 * enable the right buttons.
 */
export default function LivePanel({ onFeed, onStatus }: Props) {
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
            setReadiness((msg.readiness as Record<string, Ready>) ?? {})
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
            setReadiness((r) => ({ ...r, [String(msg.dept)]: { ready: !!msg.ready, note: String(msg.note ?? ''), ts: Number(msg.ts), source: String(msg.source ?? '') } }))
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
  }, [])

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
  const tlHeight = def && state ? stripHeight(lanes.length) : 0

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
  const autoNext = timerRows.filter((r) => r.open).sort((a, b) => a.ms - b.ms)[0]
  const autoLeft = autoNext ? Math.max(0, autoNext.ms - elapsed) : undefined
  const nextCue = lanes
    .flatMap((l) => l.cues.map((c) => ({ ...c, lane: l.name || l.id, dept: l.dept })))
    .filter((c) => !fired.has(c.id))
    .sort((a, b) => a.at_ms - b.at_ms)[0]
  const onHold = Object.entries(readiness).filter(([, v]) => !v.ready).map(([k]) => k)

  const pressGo = () => {
    if (!go) return
    if (onHold.length > 0 && !armed) {
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
  const boardDepts = [...new Set([...DEPTS.filter((d) => d.id !== 'other').map((d) => d.id as string), ...Object.keys(readiness)])]

  return (
    <div className={`cv has-right ${tlHeight ? 'has-tl' : ''}`} style={{ ['--tl-h' as string]: `${tlHeight}px` }}>
      {def && tlHeight > 0 && <TimelineStrip state={state} tl={tl} elapsed={elapsed} fired={fired} exit={exit} />}
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
          {status === 'online' ? 'Waiting for the show…' : 'Not connected. Start the runtime:  pchi-studio-api <show.pchi.json>  (port 3100)'}
        </p>
      )}

      <Card title="Show" className="right">
        <p className="bigstate">{state || '—'}</p>

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
          <small>{!go ? (autoNext ? `moves on by itself in ${fmt(autoLeft ?? 0)}` : 'nothing to send') : onHold.length ? `${onHold.join(', ')} on hold${armed ? ': press again to go' : ''}` : 'Space'}</small>
        </button>

        <h5>Departments</h5>
        <div className="ready-board">
          {boardDepts.map((id) => {
            const d = deptOf(id)
            const r = readiness[id]
            const st = !r ? 'nolink' : r.ready ? 'ready' : 'hold'
            return (
              <button
                key={id}
                className={`ready-chip ${st}`}
                onClick={() => setReady(id, !r || !r.ready ? true : false)}
                title={`${d.label}: ${st === 'nolink' ? 'has not reported' : st === 'ready' ? 'standing by' : 'on hold'}${r?.note ? ' – ' + r.note : ''}${r?.source ? ` (via ${r.source})` : ''}. Click to ${r?.ready ? 'hold' : 'mark ready'}.`}
              >
                <i style={{ background: d.color }} aria-hidden />
                <span>{d.label}</span>
                <b>{st === 'nolink' ? 'no link' : st === 'ready' ? 'READY' : 'HOLD'}</b>
              </button>
            )
          })}
        </div>
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
