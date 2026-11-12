import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  addEdge,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
} from '@xyflow/react'
import {
  BEHAVIOUR_COLOR,
  CATEGORY_COLOR,
  NODE_CATALOG,
  colorOf,
  newShow,
  newShowchart,
  simulate,
  specOf,
  type EdgeBehaviour,
  type NodeCategory,
  type NodeType,
  type PchiEvent,
  type PchiNode,
  type PchiShowFile,
} from '@pchi/sdk'
import { CoreClient, coreUrl, type Status } from './lib/core'
import Inspector, { type EdgeSel } from './components/Inspector'
import EventConsole, { type LogEntry } from './components/EventConsole'
import Timeline from './components/Timeline'
import ShowchartPanel from './components/ShowchartPanel'
import LivePanel from './components/LivePanel'

type FNode = Node<{ node: PchiNode }>
type FEdge = Edge<{ behaviour: EdgeBehaviour }>

const nodeStyle = (t: NodeType, active = false) => ({
  border: `2px solid ${colorOf(t)}`,
  background: active ? colorOf(t) : '#16181d',
  color: active ? '#000' : '#e6e6e6',
  borderRadius: 8,
  padding: 8,
  minWidth: 110,
  transition: 'background 0.15s',
})

const toFNode = (n: PchiNode): FNode => ({
  id: n.id,
  position: n.position,
  data: { node: n },
  style: nodeStyle(n.type),
})

const toFEdge = (e: { id: string; source: string; target: string; behaviour: EdgeBehaviour }, live = false): FEdge => ({
  id: e.id,
  source: e.source,
  target: e.target,
  label: e.behaviour,
  data: { behaviour: e.behaviour },
  animated: live || e.behaviour === 'stream',
  style: { stroke: BEHAVIOUR_COLOR[e.behaviour], strokeWidth: live ? 4 : 2 },
  labelStyle: { fill: BEHAVIOUR_COLOR[e.behaviour], fontSize: 11 },
})

const CATEGORIES = Object.keys(CATEGORY_COLOR) as NodeCategory[]

export default function App() {
  const initial = useMemo(() => newShow(), [])
  const [mode, setMode] = useState<'design' | 'run' | 'show' | 'live'>('design')
  const [showchart, setShowchart] = useState(initial.showchart ?? newShowchart())
  const [meta, setMeta] = useState({ project: initial.project, name: initial.name })
  const [timeline, setTimeline] = useState(initial.timeline)
  const [nodes, setNodes, onNodesChange] = useNodesState<FNode>(initial.nodes.map(toFNode))
  const [edges, setEdges, onEdgesChange] = useEdgesState<FEdge>(initial.edges.map((e) => toFEdge(e)))
  const [selNode, setSelNode] = useState<string | null>(null)
  const [selEdge, setSelEdge] = useState<string | null>(null)
  const [status, setStatus] = useState<Status>('connecting')
  const [log, setLog] = useState<LogEntry[]>([])
  const client = useRef<CoreClient>()
  const seq = useRef(1)
  const simTimers = useRef<number[]>([])

  const pushLog = useCallback((dir: LogEntry['dir'], event: PchiEvent) => {
    setLog((l) => [...l.slice(-199), { id: seq.current++, t: Date.now(), dir, event }])
  }, [])

  useEffect(() => {
    const c = new CoreClient(coreUrl(), (e) => pushLog('in', e), setStatus)
    client.current = c
    c.connect()
    return () => c.close()
  }, [pushLog])

  const buildShow = useCallback(
    (): PchiShowFile => ({
      version: '1.0',
      project: meta.project,
      name: meta.name,
      nodes: nodes.map((n) => ({ ...n.data.node, position: n.position })),
      edges: edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        behaviour: e.data?.behaviour ?? 'activate',
      })),
      timeline,
      assets: [],
      showchart,
    }),
    [meta, nodes, edges, timeline, showchart],
  )

  const emit = useCallback(
    (e: PchiEvent) => {
      client.current?.send(e)
      pushLog('out', e)
    },
    [pushLog],
  )

  const onConnect = useCallback(
    (c: Connection) =>
      setEdges((eds) =>
        addEdge(toFEdge({ id: `e-${Date.now().toString(36)}`, source: c.source!, target: c.target!, behaviour: 'activate' }), eds),
      ),
    [setEdges],
  )

  const addNode = (type: NodeType) => {
    const s = specOf(type)
    const id = `${type}-${Date.now().toString(36)}`
    const n: PchiNode = {
      id,
      type,
      label: s.label,
      position: { x: 80 + nodes.length * 24, y: 220 + nodes.length * 12 },
      params: { ...s.defaults },
      ...(type === 'scene' ? { sceneGraph: [] } : {}),
    }
    setNodes((ns) => [...ns, toFNode(n)])
  }

  const patchNode = (patch: Partial<PchiNode>) =>
    setNodes((ns) =>
      ns.map((n) => (n.id === selNode ? { ...n, data: { node: { ...n.data.node, ...patch } } } : n)),
    )

  const patchEdge = (behaviour: EdgeBehaviour) =>
    setEdges((es) => es.map((e) => (e.id === selEdge ? toFEdge({ id: e.id, source: e.source, target: e.target, behaviour }) : e)))

  // ── Event simulation: animate propagation through the graph ──
  const runSimulation = (start: string) => {
    simTimers.current.forEach(clearTimeout)
    simTimers.current = []
    const show = buildShow()
    const steps = simulate(show, start)
    pushLog('sim', { type: 'sim.start', node: start, steps: steps.length })
    steps.forEach((s) => {
      const t = window.setTimeout(() => {
        pushLog('sim', { type: 'sim.hop', node: s.node, via: s.viaEdge ?? null, behaviour: s.behaviour ?? null })
        setNodes((ns) => ns.map((n) => (n.id === s.node ? { ...n, style: nodeStyle(n.data.node.type, true) } : n)))
        if (s.viaEdge) setEdges((es) => es.map((e) => (e.id === s.viaEdge ? toFEdge({ ...e, behaviour: e.data!.behaviour }, true) : e)))
      }, s.depth * 600)
      simTimers.current.push(t)
    })
    const end = window.setTimeout(() => {
      setNodes((ns) => ns.map((n) => ({ ...n, style: nodeStyle(n.data.node.type) })))
      setEdges((es) => es.map((e) => toFEdge({ ...e, behaviour: e.data!.behaviour })))
    }, ((steps.at(-1)?.depth ?? 0) + 1) * 600 + 400)
    simTimers.current.push(end)
  }

  const save = () => {
    const blob = new Blob([JSON.stringify(buildShow(), null, 2)], { type: 'application/json' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${meta.name}.pchi`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  const load = async (file: File) => {
    const s = JSON.parse(await file.text()) as PchiShowFile
    setMeta({ project: s.project, name: s.name })
    setNodes(s.nodes.map(toFNode))
    setEdges(s.edges.map((e) => toFEdge(e)))
    setTimeline(s.timeline ?? [])
    setShowchart(s.showchart ?? newShowchart())
  }

  const node = nodes.find((n) => n.id === selNode)?.data.node ?? null
  const edgeObj = edges.find((e) => e.id === selEdge)
  const edge: EdgeSel | null = edgeObj
    ? { id: edgeObj.id, source: edgeObj.source, target: edgeObj.target, behaviour: edgeObj.data?.behaviour ?? 'activate' }
    : null

  return (
    <div className={`app ${mode}`}>
      <header>
        <strong>PCHI Studio</strong>
        <div className="tabs">
          <button className={mode === 'design' ? 'on' : ''} onClick={() => setMode('design')}>Design</button>
          <button className={mode === 'run' ? 'on' : ''} onClick={() => setMode('run')}>Run</button>
          <button className={mode === 'show' ? 'on' : ''} onClick={() => setMode('show')}>Showchart</button>
          <button className={mode === 'live' ? 'on' : ''} onClick={() => setMode('live')}>Live</button>
        </div>
        <input value={meta.project} onChange={(e) => setMeta({ ...meta, project: e.target.value })} title="project" />
        <input value={meta.name} onChange={(e) => setMeta({ ...meta, name: e.target.value })} title="show" />
        <span className={`pill ${status}`}>core: {status}</span>
        <span className="spacer" />
        <button onClick={() => emit({ type: 'graph.update', graph: buildShow() })}>Publish graph</button>
        <button onClick={save}>Save .pchi</button>
        <label className="btn">
          Open
          <input type="file" accept=".pchi,.json" hidden onChange={(e) => e.target.files?.[0] && load(e.target.files[0])} />
        </label>
      </header>

      {mode === 'live' && <LivePanel onFeed={(e) => pushLog('in', e)} />}

      {mode === 'show' && (
        <ShowchartPanel chart={showchart} onChange={setShowchart} emit={emit} note={(e) => pushLog('sim', e)} />
      )}

      {mode === 'design' && (
        <aside className="library">
          <h4>Node library</h4>
          {CATEGORIES.map((c) => (
            <div key={c}>
              <h5 style={{ color: CATEGORY_COLOR[c] }}>{c}</h5>
              {NODE_CATALOG.filter((s) => s.category === c).map((s) => (
                <button key={s.type} style={{ borderLeft: `4px solid ${CATEGORY_COLOR[c]}` }} onClick={() => addNode(s.type)}>
                  + {s.label}
                </button>
              ))}
            </div>
          ))}
        </aside>
      )}

      {mode !== 'show' && mode !== 'live' && <main>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          nodesDraggable={mode === 'design'}
          nodesConnectable={mode === 'design'}
          onSelectionChange={({ nodes: n, edges: e }) => {
            setSelNode(n[0]?.id ?? null)
            setSelEdge(n.length ? null : (e[0]?.id ?? null))
          }}
          colorMode="dark"
          fitView
        >
          <Background />
          <Controls />
          <MiniMap pannable zoomable />
        </ReactFlow>
      </main>}

      {mode !== 'show' && mode !== 'live' && <aside className="inspector">
        <Inspector
          node={node}
          edge={edge}
          onNode={patchNode}
          onEdge={patchEdge}
          onSimulate={() => selNode && runSimulation(selNode)}
          onActivate={() => node && emit({ type: 'scene.activate', scene: node.label, node: node.id })}
        />
      </aside>}

      <footer>
        {mode === 'live' ? (
          <section className="timeline">
            <h4>Live</h4>
            <p className="muted">
              Observing a show running in pchi-studio-api. Every feed message appears in the event stream.
            </p>
          </section>
        ) : mode === 'show' ? (
          <section className="timeline">
            <h4>Showchart</h4>
            <p className="muted">
              Saved in the .pchi file as <code>showchart</code>. Untick “Send actions to Core” to simulate only.
            </p>
          </section>
        ) : mode === 'design' ? (
          <Timeline clips={timeline} onChange={setTimeline} nodeIds={nodes.map((n) => n.id)} />
        ) : (
          <section className="timeline">
            <h4>Run mode</h4>
            <p className="muted">
              Select a node and press “Simulate from here” to animate an event through the graph. Live telemetry,
              replay and the venue map arrive in later alphas.
            </p>
          </section>
        )}
        <EventConsole log={log} onClear={() => setLog([])} />
      </footer>
    </div>
  )
}
