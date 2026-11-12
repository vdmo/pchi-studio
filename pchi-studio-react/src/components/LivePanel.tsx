import { useEffect, useMemo, useRef, useState } from 'react'
import { ReactFlow, Background, Controls, type Edge, type Node } from '@xyflow/react'
import {
  layoutShowchart,
  step,
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
}

const feedUrl = () => `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`

/**
 * Live view of a show running in pchi-studio-api. The server is the source of
 * truth: this panel only renders the feed and sends inputs. It never runs the
 * engine itself, apart from asking "which events could fire from here" to
 * enable the right buttons.
 */
export default function LivePanel({ onFeed }: Props) {
  const [status, setStatus] = useState<Status>('connecting')
  const [def, setDef] = useState<Showchart | null>(null)
  const [state, setState] = useState('')
  const [ctx, setCtx] = useState<ShowContext>({})
  const [timers, setTimers] = useState<number[]>([])
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
            setTimers((msg.timers as number[]) ?? [])
            break
          case 'transition':
            if (msg.applied) {
              setState(String(msg.to))
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

  // timers shown on the current state come from the chart, so they stay right after each transition
  const pendingTimers = useMemo(
    () => (def ? def.transitions.filter((t) => t.source === state && t.after_ms !== undefined).map((t) => t.after_ms!) : timers),
    [def, state, timers],
  )

  const send = (m: PchiEvent) => ws.current?.readyState === WebSocket.OPEN && ws.current.send(JSON.stringify(m))

  const pos = useMemo(() => (def ? layoutShowchart(def) : {}), [def])
  const nodes: Node[] = (def?.states ?? []).map((s) => ({
    id: s.id,
    position: pos[s.id] ?? { x: 0, y: 0 },
    data: { label: s.label ? `${s.id}\n${s.label}` : s.id },
    draggable: false,
    style: {
      whiteSpace: 'pre-line',
      border: `2px solid ${s.id === state ? '#fbbf24' : '#3b82f6'}`,
      background: s.id === state ? '#fbbf24' : '#16181d',
      color: s.id === state ? '#000' : '#e6e6e6',
      borderRadius: 8,
      minWidth: 110,
    },
  }))
  const edges: Edge[] = (def?.transitions ?? []).map((t, i) => ({
    id: `${t.source}-${i}`,
    source: t.source,
    target: t.target,
    label: transitionLabel(t),
    animated: t.source === state,
    style: { stroke: t.source === state ? '#fbbf24' : '#60a5fa' },
    labelStyle: { fill: '#e6e6e6', fontSize: 11 },
  }))

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
          <ReactFlow nodes={nodes} edges={edges} nodesConnectable={false} nodesDraggable={false} colorMode="dark" fitView key={def.id ?? 'show'}>
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
        {pendingTimers.length > 0 && <p className="muted">⏱ timer pending: {pendingTimers.map((m) => `${m} ms`).join(', ')}</p>}
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
