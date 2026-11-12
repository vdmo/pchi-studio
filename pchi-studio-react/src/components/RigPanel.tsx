import { useEffect, useMemo, useState } from 'react'
import {
  Background,
  BackgroundVariant,
  Handle,
  Panel,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react'
import { MarkerType } from '@xyflow/react'
import Card from './Card'
import { EditorCtx, ZoomControls, edgeTypes, type EditorCtxValue } from './showchartGraph'
import {
  BEHAVIOUR_COLOR,
  CATEGORY_COLOR,
  NODE_CATALOG,
  colorOf,
  specOf,
  type EdgeBehaviour,
  type NodeCategory,
  type NodeType,
  type ParamValue,
  type PchiEdge,
  type PchiNode,
} from '@pchi/sdk'

/**
 * The rig: which systems take part in the show (renderers, lighting, lasers,
 * audio, outputs ...) and how they connect. States and timelines describe what
 * happens; the rig describes who can do it.
 */

export interface Rig {
  nodes: PchiNode[]
  edges: PchiEdge[]
}

const CATEGORIES = Object.keys(CATEGORY_COLOR) as NodeCategory[]
const BEHAVIOURS = Object.keys(BEHAVIOUR_COLOR) as EdgeBehaviour[]

type RigNodeData = { node: PchiNode } & Record<string, unknown>

function RigNode({ data, selected }: NodeProps<Node<RigNodeData>>) {
  const n = data.node
  const entries = Object.entries(n.params).slice(0, 3)
  return (
    <div className={`sn rig ${selected ? 'selected' : ''}`} style={{ borderLeft: `5px solid ${colorOf(n.type)}` }}>
      <Handle type="target" position={Position.Left} className="sn-h" />
      <div className="sn-label">{n.label}</div>
      <div className="sn-sub">{specOf(n.type).label}</div>
      {entries.length > 0 && (
        <ul className="sn-actions">
          {entries.map(([k, v]) => (
            <li key={k}>
              {k}: {String(v)}
            </li>
          ))}
        </ul>
      )}
      <Handle type="source" position={Position.Right} className="sn-h" />
    </div>
  )
}
const nodeTypes = { rig: RigNode }

export default function RigPanel(props: { rig: Rig; onChange: (r: Rig) => void }) {
  return (
    <ReactFlowProvider>
      <RigEditor {...props} />
    </ReactFlowProvider>
  )
}

function RigEditor({ rig, onChange }: { rig: Rig; onChange: (r: Rig) => void }) {
  const [selected, setSelected] = useState<string | null>(null)
  const [selectedEdge, setSelectedEdge] = useState<number | null>(null)
  const [library, setLibrary] = useState(false)

  const [nodes, setNodes, onNodesChange] = useNodesState<Node<RigNodeData>>([])
  useEffect(() => {
    // keep measured sizes across rebuilds (see ShowchartPanel)
    setNodes((prev) =>
      rig.nodes.map((n) => {
        const old = prev.find((p) => p.id === n.id)
        return {
          id: n.id,
          type: 'rig',
          position: n.position,
          selected: n.id === selected,
          data: { node: n },
          ...(old ? { measured: old.measured, width: old.width, height: old.height } : {}),
        }
      }),
    )
  }, [rig.nodes, selected, setNodes])

  const edges = useMemo<Edge[]>(
    () =>
      rig.edges.map((e, i) => ({
        id: e.id,
        type: 'show',
        source: e.source,
        target: e.target,
        markerEnd: { type: MarkerType.ArrowClosed, color: '#7a7f92', width: 16, height: 16 },
        data: { index: i, event: e.behaviour, k: 0, active: false },
      })),
    [rig.edges],
  )

  const node = rig.nodes.find((n) => n.id === selected)
  const edge = selectedEdge !== null ? rig.edges[selectedEdge] : undefined

  useEffect(() => {
    if (selected && !rig.nodes.some((n) => n.id === selected)) setSelected(null)
    if (selectedEdge !== null && selectedEdge >= rig.edges.length) setSelectedEdge(null)
  }, [rig, selected, selectedEdge])

  const addDevice = (type: NodeType) => {
    const s = specOf(type)
    const id = `${type}-${Date.now().toString(36)}`
    const n: PchiNode = {
      id,
      type,
      label: s.label,
      position: { x: 120 + rig.nodes.length * 40, y: 120 + (rig.nodes.length % 4) * 110 },
      params: { ...s.defaults },
    }
    onChange({ ...rig, nodes: [...rig.nodes, n] })
    setSelected(id)
    setSelectedEdge(null)
  }
  const patchNode = (id: string, patch: Partial<PchiNode>) => onChange({ ...rig, nodes: rig.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)) })
  const removeNode = (id: string) => {
    onChange({ nodes: rig.nodes.filter((n) => n.id !== id), edges: rig.edges.filter((e) => e.source !== id && e.target !== id) })
    setSelected(null)
  }
  const onConnect = (c: Connection) => {
    if (!c.source || !c.target) return
    const e: PchiEdge = { id: `e-${Date.now().toString(36)}`, source: c.source, target: c.target, behaviour: 'activate' }
    onChange({ ...rig, edges: [...rig.edges, e] })
    setSelectedEdge(rig.edges.length)
    setSelected(null)
  }

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT')) return
      if (e.key !== 'Delete' && e.key !== 'Backspace') return
      if (selectedEdge !== null) {
        e.preventDefault()
        onChange({ ...rig, edges: rig.edges.filter((_, i) => i !== selectedEdge) })
        setSelectedEdge(null)
      } else if (selected) {
        e.preventDefault()
        removeNode(selected)
      }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, selectedEdge, rig])

  const ctx = useMemo<EditorCtxValue>(
    () => ({
      selectedTransition: selectedEdge,
      editTransition: (i) => {
        setSelectedEdge(i)
        setSelected(null)
      },
      renaming: null,
      startRename: () => {},
      rename: () => null,
      cancelRename: () => {},
    }),
    [selectedEdge],
  )

  const open = Boolean(node || edge)

  return (
    <EditorCtx.Provider value={ctx}>
      <div className={`cv ${library ? 'has-left' : ''} ${open ? 'has-right' : ''}`}>
        <div className="cv-canvas">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            edgeTypes={edgeTypes}
            nodeOrigin={[0.5, 0.5]}
            onNodesChange={onNodesChange}
            onConnect={onConnect}
            onNodeClick={(_, n) => {
              setSelected(n.id)
              setSelectedEdge(null)
            }}
            onEdgeClick={(_, e) => {
              setSelectedEdge(e.data?.index as number)
              setSelected(null)
            }}
            onPaneClick={() => {
              setSelected(null)
              setSelectedEdge(null)
            }}
            onNodeDragStop={(_, n) => patchNode(n.id, { position: { x: Math.round(n.position.x), y: Math.round(n.position.y) } })}
            deleteKeyCode={null}
            zoomOnDoubleClick={false}
            connectionLineStyle={{ stroke: '#3b82f6', strokeWidth: 2 }}
            colorMode="dark"
            minZoom={0.2}
            fitView
            fitViewOptions={{ padding: 0.18 }}
            key={`${rig.nodes.length}-${library}-${open}`}
          >
            <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} color="#2b2e3a" />
            <Panel position="top-left" className="p-crumb">
              <span className="crumb">
                <span className="muted">root</span> <span aria-hidden>›</span> <b>Rig</b>
              </span>
              <button className={library ? 'on' : ''} onClick={() => setLibrary((l) => !l)} aria-pressed={library}>＋ Add a system</button>
            </Panel>
            <Panel position="bottom-left">
              <ZoomControls />
            </Panel>
            {rig.nodes.length === 0 && (
              <Panel position="top-center" className="empty-hint">
                <p>The rig lists the systems in your show: renderers, lighting, lasers, audio, outputs.</p>
                <p>Press <b>＋ Add a system</b>, then drag from one to another to connect them.</p>
              </Panel>
            )}
          </ReactFlow>
        </div>

        {library && (
          <Card title="Add a system" className="left" onClose={() => setLibrary(false)}>
            {CATEGORIES.map((c) => (
              <div key={c}>
                <h5 style={{ color: CATEGORY_COLOR[c] }}>{c}</h5>
                <div className="chips">
                  {NODE_CATALOG.filter((s) => s.category === c).map((s) => (
                    <button key={s.type} className="chip" style={{ borderColor: CATEGORY_COLOR[c] }} onClick={() => addDevice(s.type)}>
                      + {s.label}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </Card>
        )}

        {node && (
          <Card title={specOf(node.type).label} className="right" onClose={() => setSelected(null)}>
            <label>
              Name
              <input value={node.label} onChange={(e) => patchNode(node.id, { label: e.target.value })} />
            </label>
            <h5>Settings</h5>
            {Object.entries(node.params).map(([k, v]) => (
              <label key={k}>
                {k}
                <input
                  value={String(v)}
                  onChange={(e) => {
                    const raw = e.target.value
                    const val: ParamValue = typeof v === 'number' && raw !== '' && !Number.isNaN(Number(raw)) ? Number(raw) : raw
                    patchNode(node.id, { params: { ...node.params, [k]: val } })
                  }}
                />
              </label>
            ))}
            <button
              onClick={() => {
                const k = prompt('Setting name')?.trim()
                if (k) patchNode(node.id, { params: { ...node.params, [k]: '' } })
              }}
            >
              + setting
            </button>
            <button className="danger" onClick={() => removeNode(node.id)}>Remove from rig</button>
          </Card>
        )}

        {edge && selectedEdge !== null && (
          <Card title="Connection" className="right" onClose={() => setSelectedEdge(null)}>
            <p className="muted">
              {rig.nodes.find((n) => n.id === edge.source)?.label} → {rig.nodes.find((n) => n.id === edge.target)?.label}
            </p>
            <label>
              Behaviour
              <select
                value={edge.behaviour}
                onChange={(e) => onChange({ ...rig, edges: rig.edges.map((x, i) => (i === selectedEdge ? { ...x, behaviour: e.target.value as EdgeBehaviour } : x)) })}
              >
                {BEHAVIOURS.map((b) => (
                  <option key={b}>{b}</option>
                ))}
              </select>
            </label>
            <button
              className="danger"
              onClick={() => {
                onChange({ ...rig, edges: rig.edges.filter((_, i) => i !== selectedEdge) })
                setSelectedEdge(null)
              }}
            >
              Delete connection
            </button>
          </Card>
        )}
      </div>
    </EditorCtx.Provider>
  )
}
