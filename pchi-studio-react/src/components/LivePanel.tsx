import { useEffect, useMemo, useRef, useState } from 'react'
import { ReactFlow, Background, Controls } from '@xyflow/react'
import { buildEdges, buildNodes, edgeTypes } from './showchartGraph'
import {
  step,
  timerEvent,
  transitionLabel,
  transitionTrigger,
  type PchiEvent,
  type ShowContext,
  type Showchart,
} from '@pchi/sdk'

type Status = 'connecting' | 'online' | 'offline'

interface Props {
  /** Mirrors every feed message into Studio's main event stream. */
  onFeed: (e: PchiEvent) => void
  /** Reports the feed's connection status, so the header can show the right one. */
  onStatus: (s: Status) => void
}

const feedUrl = () => `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`

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
  const onStatusRef = useRef(onStatus)
  onStatusRef.current = onStatus
  useEffect(() => onStatusRef.current(status), [status])
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
            setDef(msg.definition as Showchart)
            setState(String(msg.state))
            setCtx((msg.context as ShowContext) ?? {})
            setEnteredAt(Date.now() - Number(msg.elapsed_ms ?? 0))
            break
          case 'transition':
            if (msg.applied) {
              setState(String(msg.to))
              setEnteredAt(Date.now())
              setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `${msg.event}  ${msg.from} → ${msg.to}  (${msg.source})` }])
            } else {
              setTrail((t) => [...t.slice(-49), { ts: Number(msg.ts), text: `${msg.event} ignored in ${msg.from}  (${msg.source})` }])
            }
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

  // tick only while there is something to count down
  const counting = timerRows.some((r) => now - enteredAt <= r.ms + 500)
  useEffect(() => {
    if (!counting) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), 100)
    return () => window.clearInterval(id)
  }, [counting, enteredAt])

  const send = (m: PchiEvent) => ws.current?.readyState === WebSocket.OPEN && ws.current.send(JSON.stringify(m))

  const nodes = useMemo(() => (def ? buildNodes(def, state, false) : []), [def, state])
  const edges = useMemo(() => (def ? buildEdges(def, state) : []), [def, state])

  const available = (def?.transitions ?? []).filter((t) => t.source === state)

  return (
    <div className="showpanel live">
      <aside className="library">
        <h4>Runtime</h4>
        <p>
          <span className={`pill ${status}`}>feed: {status}</span>
        </p>
        {def && (
          <p className="muted">
            {def.name ?? def.id ?? 'show'} {def.version ? `v${def.version}` : ''}
          </p>
        )}
        <h5>Context</h5>
        <pre className="ctx">{JSON.stringify(ctx, null, 1)}</pre>
        <label>
          Set context value
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
      </aside>

      <main>
        {def ? (
          <ReactFlow nodes={nodes} edges={edges} edgeTypes={edgeTypes} nodesConnectable={false} nodesDraggable={false} colorMode="dark" fitView key={def.id ?? 'show'}>
            <Background />
            <Controls showInteractive={false} />
          </ReactFlow>
        ) : (
          <p className="muted pad">
            {status === 'online'
              ? 'Waiting for the show…'
              : 'Not connected. Start the runtime:  pchi-studio-api <show.pchi.json>  (port 3100)'}
          </p>
        )}
      </main>

      <aside className="inspector">
        <h4>State</h4>
        <p className="bigstate">{state || '—'}</p>
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
        <h5>Send event</h5>
        {available.length === 0 && <p className="muted">No events leave this state.</p>}
        {available.map((t, i) => {
          const trig = transitionTrigger(t)
          const ok = def && trig !== undefined && step(def, ctx, state, trig)?.target === t.target
          return (
            <button key={i} disabled={!ok || t.event === undefined} onClick={() => t.event && send({ type: 'input', event: t.event })} title={t.event === undefined ? 'timed: fires on its own' : ok ? '' : 'guard not satisfied'}>
              ▶ {transitionLabel(t)} → {t.target}
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
      </aside>
    </div>
  )
}
