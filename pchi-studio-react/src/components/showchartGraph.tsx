import { BaseEdge, EdgeLabelRenderer, MarkerType, type Edge, type EdgeProps, type Node } from '@xyflow/react'
import { layoutShowchart, transitionLabel, type Showchart } from '@pchi/sdk'

/**
 * Shared rendering of a showchart as a React Flow graph (Showchart editor and
 * Live view). Edges between the same two states are fanned out vertically and
 * every label sits on a solid chip, so labels never overlap the lines or each
 * other.
 */

const ACTIVE = '#fbbf24'
const IDLE = '#60a5fa'
const SPREAD = 44 // vertical bend per parallel edge, in px

interface ShowEdgeData extends Record<string, unknown> {
  label: string
  /** Position among edges joining the same pair, centred on 0 (-1, 0, 1 ...). */
  k: number
  active: boolean
}

function ShowEdge({ sourceX, sourceY, targetX, targetY, source, target, data, markerEnd }: EdgeProps<Edge<ShowEdgeData>>) {
  const d = data!
  let path: string
  let lx: number
  let ly: number
  if (source === target) {
    // self-loop: arc over the top of the node
    path = `M ${sourceX},${sourceY} C ${sourceX + 50},${sourceY - 90} ${targetX - 50},${targetY - 90} ${targetX},${targetY}`
    lx = (sourceX + targetX) / 2
    ly = sourceY - 68
  } else {
    const dx = (targetX - sourceX) * 0.4
    const off = d.k * SPREAD
    path = `M ${sourceX},${sourceY} C ${sourceX + dx},${sourceY + off} ${targetX - dx},${targetY + off} ${targetX},${targetY}`
    // point at t = 0.5 of that curve
    lx = (sourceX + targetX) / 2
    ly = (sourceY + targetY) / 2 + 0.75 * off
  }
  const colour = d.active ? ACTIVE : IDLE
  return (
    <>
      <BaseEdge path={path} markerEnd={markerEnd} style={{ stroke: colour, strokeWidth: d.active ? 2.5 : 1.5 }} />
      <EdgeLabelRenderer>
        <div
          className="edge-chip nodrag nopan"
          style={{ transform: `translate(-50%, -50%) translate(${lx}px, ${ly}px)`, borderColor: colour, color: d.active ? ACTIVE : '#e6e6e6' }}
        >
          {d.label}
        </div>
      </EdgeLabelRenderer>
    </>
  )
}

// defined once, outside any component, so React Flow does not remount edges
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
      markerEnd: { type: MarkerType.ArrowClosed, color: active ? ACTIVE : IDLE },
      data: { label: transitionLabel(t), k: group.indexOf(i) - (group.length - 1) / 2, active },
    }
  })
}

export function buildNodes(chart: Showchart, current: string, draggable: boolean): Node[] {
  const pos = layoutShowchart(chart)
  return chart.states.map((s) => ({
    id: s.id,
    position: pos[s.id] ?? { x: 0, y: 0 },
    data: { label: s.label ? `${s.id}\n${s.label}` : s.id },
    draggable,
    style: {
      whiteSpace: 'pre-line',
      border: `2px solid ${s.id === current ? ACTIVE : s.id === chart.initial ? '#34d399' : '#3b82f6'}`,
      background: s.id === current ? ACTIVE : '#16181d',
      color: s.id === current ? '#000' : '#e6e6e6',
      borderRadius: 8,
      minWidth: 110,
    },
  }))
}
