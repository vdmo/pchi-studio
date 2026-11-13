import { useEffect, useMemo, useState, type MouseEvent } from 'react'
import {
  Background,
  BackgroundVariant,
  Panel,
  ReactFlow,
  ReactFlowProvider,
  useNodesState,
  useReactFlow,
  type Connection,
  type Node,
} from '@xyflow/react'
import {
  EditorCtx,
  ZoomControls,
  buildEdges,
  buildNodes,
  edgeTypes,
  nodeTypes,
  type EditorCtxValue,
  type StateNodeData,
} from './showchartGraph'
import CommandBar from './CommandBar'
import StartScreen from './StartScreen'
import TimelineStrip, { useStripLayout } from './TimelineStrip'
import { sceneInfo } from './sceneInfo'
import Card from './Card'
import ActionList from './ActionList'
import type { History } from './useHistory'
import {
  CommandError,
  actionToEvent,
  applyCommand,
  enterActions,
  step,
  transitionLabel,
  transitionTrigger,
  validateShowchart,
  type PchiEvent,
  type ShowAction,
  type ShowContext,
  type Showchart,
  type RigState,
} from '@pchi/sdk'

interface Props {
  chart: Showchart
  /** The app-wide undo history (shared with the Timeline tab). */
  history: History
  /** Logs a simulated event to the event stream. */
  note: (e: PchiEvent) => void
  /** The scene being looked at, shared with the Timeline tab. */
  scene: string | null
  onScene: (id: string | null) => void
  /** Jump to the Timeline tab to edit the selected scene. */
  onOpenTimeline: () => void
  /** The rig, so the command box can add and change systems ("add qlab"). */
  rig?: RigState
  onRig?: (rig: RigState) => void
}

type Tool = 'select' | 'pan'
type RightTab = 'simulate' | 'state' | 'transition'

const toId = (s: string) => s.trim().replace(/[^A-Za-z0-9]+/g, '_').replace(/^_+|_+$/g, '').toUpperCase()

export default function ShowchartPanel(props: Props) {
  // the provider lets the editor use the React Flow instance (screen → canvas coordinates)
  return (
    <ReactFlowProvider>
      <Editor {...props} />
    </ReactFlowProvider>
  )
}

function Editor({ chart, history, note, scene, onScene, onOpenTimeline, rig, onRig }: Props) {
  const rf = useReactFlow()
  const { commit } = history

  const [current, setCurrent] = useState(chart.initial)
  const [trail, setTrail] = useState<string[]>([chart.initial])
  const selected = scene
  const setSelected = onScene
  const [selectedT, setSelectedT] = useState<number | null>(null)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [ctx, setCtx] = useState<ShowContext>(chart.context ?? {})
  const [ctxText, setCtxText] = useState(JSON.stringify(chart.context ?? {}, null, 1))
  const [ctxBad, setCtxBad] = useState(false)
  const [tool, setTool] = useState<Tool>('select')
  const [outline, setOutline] = useState(false)
  const [right, setRight] = useState<RightTab | null>(null)

  const issues = useMemo(() => validateShowchart(chart), [chart])
  const hard = issues.filter((i) => !i.includes('unreachable'))
  const blocked = hard.length > 0

  useEffect(() => {
    setCurrent(chart.initial)
    setTrail([chart.initial])
  }, [chart.initial])

  // a selection that no longer exists (after undo, delete or a command) is dropped
  useEffect(() => {
    if (selected && !chart.states.some((s) => s.id === selected)) setSelected(null)
    if (selectedT !== null && selectedT >= chart.transitions.length) setSelectedT(null)
  }, [chart, selected, selectedT])

  useEffect(() => {
    if (!notice) return
    const id = window.setTimeout(() => setNotice(null), 4000)
    return () => window.clearTimeout(id)
  }, [notice])

  // Controlled nodes so dragging is smooth; positions are written back to the chart on drop.
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<StateNodeData>>([])
  useEffect(() => {
    // Keep each node's measured size when rebuilding from the chart: React Flow hides a node
    // until it has measured it, and a rebuild that dropped the measurement would leave a
    // freshly added state invisible (and un-focusable).
    setNodes((prev) =>
      buildNodes(chart, current, tool === 'select', selected).map((n) => {
        const old = prev.find((p) => p.id === n.id)
        return old ? { ...n, measured: old.measured, width: old.width, height: old.height } : n
      }),
    )
  }, [chart, current, tool, selected, setNodes])
  const edges = useMemo(() => buildEdges(chart, current), [chart, current])

  // ── edits ──
  const patchState = (id: string, patch: Partial<Showchart['states'][number]>, key?: string) =>
    commit({ ...chart, states: chart.states.map((s) => (s.id === id ? { ...s, ...patch } : s)) }, key)
  const patchT = (i: number, patch: Partial<Showchart['transitions'][number]>, key?: string) =>
    commit({ ...chart, transitions: chart.transitions.map((x, j) => (j === i ? { ...x, ...patch } : x)) }, key)

  const deleteTransition = (i: number) => {
    commit({ ...chart, transitions: chart.transitions.filter((_, j) => j !== i) })
    setSelectedT(null)
    setNotice('Transition deleted. Ctrl+Z to undo.')
  }
  const deleteState = (id: string) => {
    try {
      commit(applyCommand(chart, { op: 'removeState', id }).chart)
      setSelected(null)
      setNotice(`Deleted ${id} and its transitions. Ctrl+Z to undo.`)
    } catch (e) {
      setNotice(e instanceof CommandError ? e.message : 'Could not delete that state.')
    }
  }

  const rename = (id: string, to: string): string | null => {
    const newId = toId(to)
    setRenaming(null)
    if (!newId) {
      setNotice('A state needs a name.')
      return 'empty'
    }
    try {
      const r = applyCommand(chart, { op: 'renameState', from: id, to: newId })
      const layout = r.chart.layout ? { ...r.chart.layout } : undefined
      if (layout?.[id]) {
        layout[newId] = layout[id]
        delete layout[id]
      }
      commit(layout ? { ...r.chart, layout } : r.chart)
      if (selected === id) setSelected(newId)
      if (current === id) setCurrent(newId)
      return null
    } catch (e) {
      const m = e instanceof CommandError ? e.message : 'Could not rename.'
      setNotice(m)
      return m
    }
  }

  const onConnect = (c: Connection) => {
    if (!c.source || !c.target) return
    const taken = new Set(chart.transitions.filter((t) => t.source === c.source && !t.guard).map((t) => t.event))
    let name = 'EVENT'
    let n = 1
    while (taken.has(name)) name = `EVENT_${++n}`
    commit({ ...chart, transitions: [...chart.transitions, { source: c.source, target: c.target, event: name }] })
    setSelectedT(chart.transitions.length)
    setSelected(null)
    setRight('transition')
  }

  const addStateAt = (e: MouseEvent) => {
    if (!(e.target as HTMLElement).classList.contains('react-flow__pane')) return
    const p = rf.screenToFlowPosition({ x: e.clientX, y: e.clientY })
    let id = 'STATE'
    let n = 1
    while (chart.states.some((s) => s.id === id)) id = `STATE_${++n}`
    const next: Showchart = {
      ...chart,
      states: [...chart.states, { id }],
      layout: { ...(chart.layout ?? {}), [id]: { x: Math.round(p.x), y: Math.round(p.y) } },
    }
    if (!chart.states.some((s) => s.id === chart.initial)) next.initial = id
    commit(next)
    setSelected(id)
    setSelectedT(null)
    setRenaming(id)
  }

  // ── keyboard: undo/redo, delete, rename, escape ──
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable)) return
      const mod = e.ctrlKey || e.metaKey
      const k = e.key.toLowerCase()
      if (mod && k === 'z') {
        e.preventDefault()
        if (!(e.shiftKey ? history.redo() : history.undo())) setNotice(e.shiftKey ? 'Nothing to redo.' : 'Nothing to undo.')
      } else if (mod && k === 'y') {
        e.preventDefault()
        history.redo()
      } else if (e.key === 'Delete' || e.key === 'Backspace') {
        if (selectedT !== null) {
          e.preventDefault()
          deleteTransition(selectedT)
        } else if (selected) {
          e.preventDefault()
          deleteState(selected)
        }
      } else if (e.key === 'F2' && selected) {
        e.preventDefault()
        setRenaming(selected)
      } else if (e.key === 'Escape') {
        setSelected(null)
        setSelectedT(null)
      }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, selectedT, chart, history.undo, history.redo])

  // ── simulation ──
  const run = (actions: ShowAction[]) => actions.forEach((a) => note({ ...actionToEvent(a), simulated: true }))

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
  const sel = chart.states.find((s) => s.id === selected)
  const selT = selectedT !== null ? chart.transitions[selectedT] : undefined

  const selectState = (id: string | null) => {
    setSelected(id)
    setSelectedT(null)
    if (id) setRight('state')
  }
  const editTransition = (i: number) => {
    setSelectedT(i)
    setSelected(null)
    setRight('transition')
  }

  const editor = useMemo<EditorCtxValue>(
    () => ({
      selectedTransition: selectedT,
      editTransition,
      renaming,
      startRename: (id) => {
        setSelected(id)
        setRenaming(id)
      },
      rename,
      cancelRename: () => setRenaming(null),
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selectedT, renaming, chart, selected, current],
  )

  // the selected scene's timeline sits under the canvas, so the chart and what happens inside each scene are seen together
  const selState = chart.states.find((s) => s.id === selected)
  const stripOn = !!selState && chart.states.length > 0
  const strip = useStripLayout(selState?.timeline?.lanes.length ?? 0)
  const stripH = stripOn ? strip.h : 0
  const stateOptions = chart.states.map((s) => <option key={s.id}>{s.id}</option>)

  return (
    <EditorCtx.Provider value={editor}>
      <div className={`cv ${outline ? 'has-left' : ''} ${right ? 'has-right' : ''} ${stripOn ? 'has-tl' : ''}`} style={{ ['--tl-h' as string]: `${stripH}px` }}>
        <div className="cv-canvas" onDoubleClick={addStateAt}>
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            edgeTypes={edgeTypes}
            nodeOrigin={[0.5, 0.5]}
            onNodesChange={onNodesChange}
            nodesConnectable={tool === 'select'}
            nodesDraggable={tool === 'select'}
            onConnect={onConnect}
            onNodeClick={(_, n) => selectState(n.id)}
            onEdgeClick={(_, e) => typeof e.data?.index === 'number' && editTransition(e.data.index)}
            onPaneClick={() => {
              setSelected(null)
              setSelectedT(null)
            }}
            onNodeDragStop={(_, n) =>
              commit({ ...chart, layout: { ...(chart.layout ?? {}), [n.id]: { x: Math.round(n.position.x), y: Math.round(n.position.y) } } })
            }
            zoomOnDoubleClick={false}
            deleteKeyCode={null}
            // React Flow re-focuses the selected node, which would steal focus from the rename box
            nodesFocusable={renaming === null}
            connectionLineStyle={{ stroke: '#3b82f6', strokeWidth: 2 }}
            colorMode="dark"
            minZoom={0.2}
            fitView
            fitViewOptions={{ padding: 0.18 }}
            // remount when the canvas is resized by a docked panel or states are added, so it re-fits
            key={`${chart.states.length}-${outline}-${right !== null}-${stripOn ? strip.settled : 0}`}
          >
            <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} color="#2b2e3a" />

            <Panel position="top-center" className="p-palette">
              <CommandBar chart={chart} onApply={commit} rig={rig} onRig={onRig} onUndo={history.undo} onRedo={history.redo} canUndo={history.canUndo} canRedo={history.canRedo} stateContext={selected ?? undefined} />
            </Panel>

            <Panel position="top-left" className="p-crumb">
              <span className="crumb">
                <span className="muted">root</span> <span aria-hidden>›</span> <b>{chart.name ?? chart.id ?? 'show'}</b>
              </span>
              {chart.states.length > 0 && <button className={`status ${blocked ? 'bad' : 'good'}`} onClick={() => setRight('simulate')} title={blocked ? hard.join('\n') : 'No problems'}>
                {blocked ? `⚠ ${hard.length} problem${hard.length > 1 ? 's' : ''}` : '✓ valid'}
              </button>}
            </Panel>

            <Panel position="top-right" className="p-top-right">
              <button className={outline ? 'on' : ''} onClick={() => setOutline((o) => !o)} aria-pressed={outline}>☰ Outline</button>
            </Panel>

            <Panel position="bottom-left">
              <ZoomControls />
            </Panel>

            <Panel position="bottom-center">
              {notice && (
                <div className="toast" role="status">
                  {notice}
                </div>
              )}
              <div className="toolbar" role="toolbar" aria-label="Canvas tools">
                <button className={tool === 'select' ? 'on' : ''} onClick={() => setTool('select')} aria-pressed={tool === 'select'} title="Select, move and connect states">↖</button>
                <button className={tool === 'pan' ? 'on' : ''} onClick={() => setTool('pan')} aria-pressed={tool === 'pan'} title="Pan the canvas">✋</button>
                <button className={right === 'simulate' ? 'on' : ''} onClick={() => setRight((r) => (r === 'simulate' ? null : 'simulate'))} aria-pressed={right === 'simulate'} title="Simulate the show">▶</button>
              </div>
            </Panel>

          </ReactFlow>
          {chart.states.length === 0 && <StartScreen chart={chart} onApply={commit} />}
        </div>

        {stripOn && <TimelineStrip state={selState!.id} tl={selState!.timeline} exit={sceneInfo(chart, selState!.id).exit} onOpen={onOpenTimeline} layout={strip} />}

        {outline && (
          <Card title="Outline" className="left" onClose={() => setOutline(false)}>
            <h5>States</h5>
            {chart.states.map((s) => (
              <button key={s.id} className={`row-btn ${s.id === selected ? 'sel' : ''}`} onClick={() => selectState(s.id)}>
                {s.id === chart.initial ? '● ' : ''}
                {s.id}
                {s.label ? <span className="muted"> · {s.label}</span> : null}
              </button>
            ))}
            <h5>Transitions</h5>
            {chart.transitions.map((t, i) => (
              <button key={i} className={`row-btn ${i === selectedT ? 'sel' : ''}`} onClick={() => editTransition(i)}>
                {t.source} → {t.target} <span className="muted">· {transitionLabel(t)}</span>
              </button>
            ))}
            <p className="muted">Tip: drag from a state’s edge to another state to connect them. Double-click a state to rename it.</p>
          </Card>
        )}

        {right && (
          <Card
            title={right === 'simulate' ? 'Simulate' : right === 'transition' ? 'Transition' : sel ? sel.id : 'State'}
            className="right"
            onClose={() => setRight(null)}
            tabs={
              <span className="seg" role="tablist">
                <button role="tab" aria-selected={right === 'simulate'} className={right === 'simulate' ? 'on' : ''} onClick={() => setRight('simulate')}>Run</button>
                <button role="tab" aria-selected={right === 'state'} className={right === 'state' ? 'on' : ''} onClick={() => setRight('state')}>State</button>
                <button role="tab" aria-selected={right === 'transition'} className={right === 'transition' ? 'on' : ''} onClick={() => setRight('transition')}>Link</button>
              </span>
            }
          >
            {right === 'simulate' ? (
              <>
                <p className="bigstate">{current || '—'}</p>
                {blocked && <p className="warn">Fix the problems below before running.</p>}
                {available.length === 0 && <p className="muted">No events leave this state.</p>}
                {available.map((t, i) => {
                  const trig = transitionTrigger(t)
                  const ok = trig !== undefined && step(chart, ctx, current, trig)?.target === t.target
                  return (
                    <button key={i} className="event-btn" disabled={blocked || !ok} onClick={() => trig && fire(trig)} title={ok ? '' : 'guard not satisfied (or shadowed by an earlier transition)'}>
                      <span>▶ {transitionLabel(t)}</span>
                      <span className="muted">→ {t.target}</span>
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
              </>
            ) : right === 'transition' ? (
              selT && selectedT !== null ? (
                <>
                  <label>
                    From
                    <select value={selT.source} onChange={(e) => patchT(selectedT, { source: e.target.value })}>{stateOptions}</select>
                  </label>
                  <label>
                    To
                    <select value={selT.target} onChange={(e) => patchT(selectedT, { target: e.target.value })}>{stateOptions}</select>
                  </label>
                  <label>
                    Event
                    <input
                      value={selT.event ?? ''}
                      placeholder="e.g. START (leave empty for a timer)"
                      onChange={(e) => patchT(selectedT, { event: e.target.value.toUpperCase().replace(/\s+/g, '_') || undefined }, `t${selectedT}:event`)}
                    />
                  </label>
                  <label>
                    Delay (ms)
                    <input
                      type="number"
                      min={0}
                      value={selT.after_ms ?? ''}
                      placeholder="fires on its own after this long"
                      onChange={(e) => patchT(selectedT, { after_ms: e.target.value === '' ? undefined : Number(e.target.value) }, `t${selectedT}:after`)}
                    />
                  </label>
                  <label>
                    Only if
                    <input
                      value={selT.guard ?? ''}
                      placeholder="energy > 0.7 or a guard name"
                      onChange={(e) => patchT(selectedT, { guard: e.target.value || undefined }, `t${selectedT}:guard`)}
                    />
                  </label>
                  <button className="danger" onClick={() => deleteTransition(selectedT)}>Delete transition</button>
                </>
              ) : (
                <p className="muted">Click a transition’s label on the canvas to edit it, or drag from one state to another to create one.</p>
              )
            ) : sel ? (
              <>
                <label>
                  Label
                  <input value={sel.label ?? ''} placeholder="a friendly name" onChange={(e) => patchState(sel.id, { label: e.target.value || undefined }, `label:${sel.id}`)} />
                </label>
                <div className="btnrow">
                  <button onClick={() => setRenaming(sel.id)} title="Rename (F2)">Rename</button>
                  <button onClick={() => commit({ ...chart, initial: sel.id })} disabled={chart.initial === sel.id}>Make initial</button>
                </div>
                <ActionList title="On enter" actions={sel.enter_actions} onChange={(a) => patchState(sel.id, { enter_actions: a }, `act:${sel.id}:enter`)} />
                <ActionList title="On exit" actions={sel.exit_actions} onChange={(a) => patchState(sel.id, { exit_actions: a }, `act:${sel.id}:exit`)} />
                <button className="danger" onClick={() => deleteState(sel.id)}>Delete state</button>
              </>
            ) : (
              <p className="muted">Click a state on the canvas to edit it.</p>
            )}
          </Card>
        )}
      </div>
    </EditorCtx.Provider>
  )
}
