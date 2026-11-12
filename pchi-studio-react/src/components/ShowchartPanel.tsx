import { useEffect, useMemo, useState } from 'react'
import { ReactFlow, Background, Controls, type Edge, type Node } from '@xyflow/react'
import {
  actionToEvent,
  enterActions,
  layoutShowchart,
  step,
  transitionLabel,
  transitionTrigger,
  validateShowchart,
  type PchiEvent,
  type ShowContext,
  type ShowAction,
  type Showchart,
} from '@pchi/sdk'

interface Props {
  chart: Showchart
  onChange: (c: Showchart) => void
  /** Sends an event to PCHI Core and logs it. */
  emit: (e: PchiEvent) => void
  /** Logs a local (simulated) event without sending it. */
  note: (e: PchiEvent) => void
}

const parseActions = (text: string): ShowAction[] => {
  try {
    const v = JSON.parse(text || '[]')
    return Array.isArray(v) ? v : []
  } catch {
    return []
  }
}

export default function ShowchartPanel({ chart, onChange, emit, note }: Props) {
  const [current, setCurrent] = useState(chart.initial)
  const [live, setLive] = useState(false) // false = simulate only, true = also send to Core
  const [trail, setTrail] = useState<string[]>([chart.initial])
  const [selected, setSelected] = useState<string | null>(null)
  const [ctx, setCtx] = useState<ShowContext>(chart.context ?? {})
  const [ctxText, setCtxText] = useState(JSON.stringify(chart.context ?? {}, null, 1))
  const [ctxBad, setCtxBad] = useState(false)
  const issues = useMemo(() => validateShowchart(chart), [chart])
  const blocked = issues.some((i) => !i.includes('unreachable'))

  useEffect(() => {
    setCurrent(chart.initial)
    setTrail([chart.initial])
  }, [chart.initial])

  const run = (actions: ShowAction[]) =>
    actions.forEach((a) => {
      const ev = actionToEvent(a)
      if (live) emit(ev)
      else note({ ...ev, simulated: true })
    })

  const reset = () => {
    setCtx(chart.context ?? {})
    setCtxText(JSON.stringify(chart.context ?? {}, null, 1))
    setCtxBad(false)
    setCurrent(chart.initial)
    setTrail([chart.initial])
    run(enterActions(chart, chart.initial))
  }

  const fire = (event: string) => {
    const s = step(chart, ctx, current, event)
    if (!s) return
    setCurrent(s.target)
    setTrail((t) => [...t, `${event} → ${s.target}`])
    run(s.actions)
  }

  const available = chart.transitions.filter((t) => t.source === current)
  const patchT = (i: number, patch: Partial<Showchart['transitions'][number]>) =>
    onChange({ ...chart, transitions: chart.transitions.map((x, j) => (j === i ? { ...x, ...patch } : x)) })
  const pos = useMemo(() => layoutShowchart(chart), [chart])

  const nodes: Node[] = chart.states.map((s) => ({
    id: s.id,
    position: pos[s.id] ?? { x: 0, y: 0 },
    data: { label: s.label ? `${s.id}\n${s.label}` : s.id },
    style: {
      whiteSpace: 'pre-line',
      border: `2px solid ${s.id === current ? '#fbbf24' : s.id === chart.initial ? '#34d399' : '#3b82f6'}`,
      background: s.id === current ? '#fbbf24' : '#16181d',
      color: s.id === current ? '#000' : '#e6e6e6',
      borderRadius: 8,
      minWidth: 110,
    },
  }))
  const edges: Edge[] = chart.transitions.map((t, i) => ({
    id: `${t.source}-${t.event}-${i}`,
    source: t.source,
    target: t.target,
    label: t.event,
    animated: t.source === current,
    style: { stroke: t.source === current ? '#fbbf24' : '#60a5fa' },
    labelStyle: { fill: '#e6e6e6', fontSize: 11 },
  }))

  const patchState = (id: string, patch: Partial<Showchart['states'][number]>) =>
    onChange({ ...chart, states: chart.states.map((s) => (s.id === id ? { ...s, ...patch } : s)) })

  const sel = chart.states.find((s) => s.id === selected)

  return (
    <div className="showpanel">
      <aside className="library">
        <h4>States</h4>
        {chart.states.map((s) => (
          <button
            key={s.id}
            className={s.id === selected ? 'sel' : ''}
            onClick={() => setSelected(s.id)}
            title={s.id === chart.initial ? 'initial state' : ''}
          >
            {s.id === chart.initial ? '● ' : ''}
            {s.id}
          </button>
        ))}
        <button
          onClick={() => {
            const id = prompt('New state id (e.g. FINALE)')?.trim().toUpperCase()
            if (id && !chart.states.some((s) => s.id === id)) onChange({ ...chart, states: [...chart.states, { id }] })
          }}
        >
          + state
        </button>
        <h4>Transitions</h4>
        {chart.transitions.map((t, i) => (
          <div key={i} className="tgroup">
            <div className="trow">
              <select value={t.source} onChange={(e) => patchT(i, { source: e.target.value })}>
                {chart.states.map((s) => <option key={s.id}>{s.id}</option>)}
              </select>
              <input value={t.event ?? ''} placeholder="event" onChange={(e) => patchT(i, { event: e.target.value || undefined })} />
              <select value={t.target} onChange={(e) => patchT(i, { target: e.target.value })}>
                {chart.states.map((s) => <option key={s.id}>{s.id}</option>)}
              </select>
              <button className="x" onClick={() => onChange({ ...chart, transitions: chart.transitions.filter((_, j) => j !== i) })}>×</button>
            </div>
            <div className="trow2">
              <input value={t.guard ?? ''} placeholder="guard (name or energy > 0.7)" onChange={(e) => patchT(i, { guard: e.target.value || undefined })} />
              <input type="number" min={0} value={t.after_ms ?? ''} placeholder="after ms" onChange={(e) => patchT(i, { after_ms: e.target.value === '' ? undefined : Number(e.target.value) })} />
            </div>
          </div>
        ))}
        <button
          onClick={() =>
            chart.states.length &&
            onChange({ ...chart, transitions: [...chart.transitions, { source: chart.states[0].id, target: chart.states[0].id, event: 'EVENT' }] })
          }
        >
          + transition
        </button>
      </aside>

      <main>
        <ReactFlow nodes={nodes} edges={edges} nodesConnectable={false} onNodeClick={(_, n) => setSelected(n.id)} colorMode="dark" fitView>
          <Background />
          <Controls />
        </ReactFlow>
      </main>

      <aside className="inspector">
        <h4>Run</h4>
        <p>
          Current: <b>{current}</b>
        </p>
        <label className="row">
          <input type="checkbox" checked={live} onChange={(e) => setLive(e.target.checked)} />
          Send actions to Core
        </label>
        {blocked && <p className="warn">Fix errors before running.</p>}
        {available.length === 0 && <p className="muted">No events available from here.</p>}
        {available.map((t, i) => {
          const trig = transitionTrigger(t)
          const ok = trig !== undefined && step(chart, ctx, current, trig)?.target === t.target
          return (
            <button key={i} disabled={blocked || !ok} onClick={() => trig && fire(trig)} title={ok ? '' : 'guard not satisfied (or shadowed by an earlier transition)'}>
              ▶ {transitionLabel(t)} → {t.target}
            </button>
          )
        })}
        <button onClick={reset} disabled={blocked}>↺ Reset</button>
        <p className="muted">{trail.join('  ·  ')}</p>

        <label>
          Context (JSON, read by guards)
          <textarea
            rows={4}
            value={ctxText}
            style={ctxBad ? { borderColor: '#ef4444' } : undefined}
            onChange={(e) => {
              setCtxText(e.target.value)
              try {
                const v = JSON.parse(e.target.value || '{}')
                if (v === null || typeof v !== 'object' || Array.isArray(v)) throw new Error()
                setCtx(v)
                setCtxBad(false)
              } catch {
                setCtxBad(true)
              }
            }}
          />
        </label>

        {issues.length > 0 && (
          <>
            <h5>Problems</h5>
            {issues.map((i) => (
              <p key={i} className={i.includes('unreachable') ? 'muted' : 'warn'}>{i}</p>
            ))}
          </>
        )}

        {sel && (
          <>
            <h4>{sel.id}</h4>
            <label>
              Label
              <input value={sel.label ?? ''} onChange={(e) => patchState(sel.id, { label: e.target.value })} />
            </label>
            <button onClick={() => onChange({ ...chart, initial: sel.id })} disabled={chart.initial === sel.id}>
              Make initial
            </button>
            <ActionsField
              key={`${sel.id}-enter`}
              title="Enter actions (JSON)"
              value={sel.enter_actions}
              onValid={(a) => patchState(sel.id, { enter_actions: a })}
            />
            <ActionsField
              key={`${sel.id}-exit`}
              title="Exit actions (JSON)"
              value={sel.exit_actions}
              onValid={(a) => patchState(sel.id, { exit_actions: a })}
            />
          </>
        )}
      </aside>
    </div>
  )
}

function ActionsField({ title, value, onValid }: { title: string; value?: ShowAction[]; onValid: (a: ShowAction[]) => void }) {
  const [text, setText] = useState(JSON.stringify(value ?? [], null, 1))
  const [bad, setBad] = useState(false)
  return (
    <label>
      {title}
      <textarea
        rows={5}
        value={text}
        style={bad ? { borderColor: '#ef4444' } : undefined}
        onChange={(e) => {
          setText(e.target.value)
          try {
            const v = JSON.parse(e.target.value || '[]')
            if (!Array.isArray(v)) throw new Error()
            setBad(false)
            onValid(parseActions(e.target.value))
          } catch {
            setBad(true)
          }
        }}
      />
    </label>
  )
}
