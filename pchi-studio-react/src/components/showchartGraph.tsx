import { createContext, useContext, useEffect, useRef, useState } from 'react'
import {
  BaseEdge,
  EdgeLabelRenderer,
  Handle,
  MarkerType,
  Position,
  getSmoothStepPath,
  useReactFlow,
  useViewport,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
} from '@xyflow/react'
import { layoutShowchart, type ShowAction, type Showchart } from '@pchi/sdk'

/**
 * Shared canvas rendering for the Showchart editor and the Live view.
 *
 * Look: quiet nodes with a bold name, right-angle edges with the event on a
 * pill, a start marker on the initial state, and the state's actions listed
 * inside the node so the show can be read straight off the canvas.
 */

const ACTIVE = '#fbbf24'
const IDLE = '#7a7f92'
const GAP = 30 // horizontal separation between parallel edges
const ROW = 30 // vertical separation between their labels
const MAX_ACTIONS = 3

// ───────────────────────── editor context ─────────────────────────
// Lets the custom node and edge components call back into the editor without
// putting functions into node/edge data (which would change on every render).

export interface EditorCtxValue {
  selectedTransition: number | null
  editTransition: (index: number) => void
  /** State id currently being renamed in place, if any. */
  renaming: string | null
  startRename: (id: string) => void
  /** Returns an error message, or null when the rename went through. */
  rename: (id: string, to: string) => string | null
  cancelRename: () => void
}
export const EditorCtx = createContext<EditorCtxValue | null>(null)

// ───────────────────────── nodes ─────────────────────────

export interface StateNodeData extends Record<string, unknown> {
  id: string
  label?: string
  actions: string[]
  more: number
  initial: boolean
  current: boolean
}

const actionLine = (arrow: '→' | '←', a: ShowAction) => {
  const first = a.payload ? Object.values(a.payload)[0] : undefined
  return `${arrow} ${a.kind}${first !== undefined ? ` · ${String(first)}` : ''}`
}

function StateNode({ data, selected }: NodeProps<Node<StateNodeData>>) {
  const ed = useContext(EditorCtx)
  const editing = ed?.renaming === data.id
  const [draft, setDraft] = useState(data.id)
  const done = useRef(false)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!editing) return
    setDraft(data.id)
    done.current = false
    // A node stays visibility:hidden until React Flow has measured it, and a hidden
    // element cannot take focus, so keep trying for a moment until it sticks.
    let tries = 0
    const id = window.setInterval(() => {
      const el = input.current
      if (el) {
        el.focus()
        if (document.activeElement === el) {
          el.select()
          window.clearInterval(id)
          return
        }
      }
      if (++tries > 40) window.clearInterval(id)
    }, 50)
    return () => window.clearInterval(id)
  }, [editing, data.id])

  const finish = (save: boolean) => {
    if (done.current) return
    done.current = true
    if (save && draft.trim() && draft.trim() !== data.id) ed?.rename(data.id, draft)
    else ed?.cancelRename()
  }

  return (
    <div
      className={`sn ${data.current ? 'current' : ''} ${selected ? 'selected' : ''}`}
      onDoubleClick={(e) => {
        e.stopPropagation()
        ed?.startRename(data.id)
      }}
    >
      {data.initial && <span className="sn-init" title="initial state" aria-label="initial state" />}
      <Handle type="target" position={Position.Left} className="sn-h" />
      {editing ? (
        <input
          className="sn-edit nodrag nopan"
          ref={input}
          value={draft}
          aria-label={`Rename ${data.id}`}
          onChange={(e) => setDraft(e.target.value)}
          onFocus={(e) => e.target.select()}
          onBlur={() => finish(true)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') finish(true)
            if (e.key === 'Escape') finish(false)
            e.stopPropagation()
          }}
        />
      ) : (
        <div className="sn-label">{data.id}</div>
      )}
      {data.label && <div className="sn-sub">{data.label}</div>}
      {data.actions.length > 0 && (
        <ul className="sn-actions" aria-label="actions">
          {data.actions.map((a, i) => (
            <li key={i}>{a}</li>
          ))}
          {data.more > 0 && <li className="more">+{data.more} more</li>}
        </ul>
      )}
      <Handle type="source" position={Position.Right} className="sn-h" />
    </div>
  )
}

export const nodeTypes = { state: StateNode }

export function buildNodes(chart: Showchart, current: string, draggable: boolean, selectedId?: string | null): Node<StateNodeData>[] {
  const auto = layoutShowchart(chart)
  return chart.states.map((s) => {
    const lines = [...(s.enter_actions ?? []).map((a) => actionLine('→', a)), ...(s.exit_actions ?? []).map((a) => actionLine('←', a))]
    return {
      id: s.id,
      type: 'state',
      position: chart.layout?.[s.id] ?? auto[s.id] ?? { x: 0, y: 0 },
      draggable,
      selected: s.id === selectedId,
      data: {
        id: s.id,
        label: s.label,
        actions: lines.slice(0, MAX_ACTIONS),
        more: Math.max(0, lines.length - MAX_ACTIONS),
        initial: s.id === chart.initial,
        current: s.id === current,
      },
    }
  })
}

// ───────────────────────── edges ─────────────────────────

interface ShowEdgeData extends Record<string, unknown> {
  /** Index into chart.transitions. */
  index: number
  event: string
  guard?: string
  /** Position among edges joining the same two states, centred on 0. */
  k: number
  active: boolean
}

function ShowEdge({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, source, target, data, markerEnd }: EdgeProps<Edge<ShowEdgeData>>) {
  const d = data!
  const ed = useContext(EditorCtx)
  const chosen = ed?.selectedTransition === d.index
  let path: string
  let lx: number
  let ly: number
  if (source === target) {
    // self-loop: arc over the top of the node
    path = `M ${sourceX},${sourceY} C ${sourceX + 40},${sourceY - 80} ${targetX - 40},${targetY - 80} ${targetX},${targetY}`
    lx = (sourceX + targetX) / 2
    ly = sourceY - 62
  } else {
    const [p, x, y] = getSmoothStepPath({
      sourceX,
      sourceY,
      sourcePosition,
      targetX,
      targetY,
      targetPosition,
      borderRadius: 12,
      centerX: (sourceX + targetX) / 2 + d.k * GAP,
    })
    path = p
    lx = x
    ly = y + d.k * ROW
  }
  const colour = chosen ? '#3b82f6' : d.active ? ACTIVE : IDLE
  return (
    <>
      <BaseEdge path={path} markerEnd={markerEnd} interactionWidth={22} style={{ stroke: colour, strokeWidth: chosen || d.active ? 2 : 1.5 }} />
      <EdgeLabelRenderer>
        <div
          className={`edge-pill nodrag nopan ${d.active ? 'active' : ''} ${chosen ? 'chosen' : ''}`}
          style={{ transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)` }}
          role="button"
          tabIndex={0}
          title="Edit this transition"
          onClick={(e) => {
            e.stopPropagation()
            ed?.editTransition(d.index)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') ed?.editTransition(d.index)
          }}
        >
          <b>{d.event}</b>
          {d.guard && <i>{d.guard}</i>}
        </div>
      </EdgeLabelRenderer>
    </>
  )
}

export const edgeTypes = { show: ShowEdge }

export function buildEdges(chart: Showchart, current?: string): Edge<ShowEdgeData>[] {
  const groups = new Map<string, number[]>()
  chart.transitions.forEach((t, i) => {
    const key = `${t.source}\u0000${t.target}`
    groups.set(key, [...(groups.get(key) ?? []), i])
  })
  return chart.transitions.map((t, i) => {
    const group = groups.get(`${t.source}\u0000${t.target}`)!
    const active = t.source === current
    return {
      id: `t${i}`,
      type: 'show',
      source: t.source,
      target: t.target,
      markerEnd: { type: MarkerType.ArrowClosed, color: active ? ACTIVE : IDLE, width: 16, height: 16 },
      data: {
        index: i,
        event: t.event ?? (t.after_ms !== undefined ? `after ${t.after_ms / 1000}s` : '(no trigger)'),
        guard: t.guard,
        k: group.indexOf(i) - (group.length - 1) / 2,
        active,
      },
    }
  })
}

// ───────────────────────── canvas helpers (use inside <ReactFlow>) ─────────────────────────

export function ZoomControls() {
  const rf = useReactFlow()
  const { zoom } = useViewport()
  return (
    <div className="zoom" role="group" aria-label="Zoom">
      <button onClick={() => rf.zoomOut({ duration: 150 })} aria-label="Zoom out">−</button>
      <span aria-live="polite">{Math.round(zoom * 100)}%</span>
      <button onClick={() => rf.zoomIn({ duration: 150 })} aria-label="Zoom in">+</button>
      <button onClick={() => rf.fitView({ padding: 0.18, duration: 250 })} aria-label="Fit to screen" title="Fit to screen">⛶</button>
    </div>
  )
}
