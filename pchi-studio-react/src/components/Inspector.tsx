import {
  BEHAVIOUR_COLOR,
  specOf,
  type EdgeBehaviour,
  type PchiNode,
  type SceneItem,
} from '@pchi/sdk'

const BEHAVIOURS = Object.keys(BEHAVIOUR_COLOR) as EdgeBehaviour[]
const ICON: Record<SceneItem['kind'], string> = {
  camera: '🎥',
  fog: '🌫',
  particles: '✦',
  asset: '▣',
  output: '⇥',
  trigger: '⚡',
  object: '◇',
}

export interface EdgeSel {
  id: string
  source: string
  target: string
  behaviour: EdgeBehaviour
}

interface Props {
  node: PchiNode | null
  edge: EdgeSel | null
  onNode: (patch: Partial<PchiNode>) => void
  onEdge: (behaviour: EdgeBehaviour) => void
  onSimulate: () => void
  onActivate: () => void
}

export default function Inspector({ node, edge, onNode, onEdge, onSimulate, onActivate }: Props) {
  if (edge) {
    return (
      <div>
        <h4>Connection</h4>
        <p className="muted">
          {edge.source} → {edge.target}
        </p>
        <label>
          Behaviour
          <select value={edge.behaviour} onChange={(e) => onEdge(e.target.value as EdgeBehaviour)}>
            {BEHAVIOURS.map((b) => (
              <option key={b}>{b}</option>
            ))}
          </select>
        </label>
      </div>
    )
  }
  if (!node) return <p className="muted">Select a node or connection.</p>

  const scene = node.sceneGraph ?? []
  return (
    <div>
      <h4>{specOf(node.type).label}</h4>
      <p className="muted">
        {specOf(node.type).category} · {node.id}
      </p>
      <label>
        Label
        <input value={node.label} onChange={(e) => onNode({ label: e.target.value })} />
      </label>

      <h5>Parameters</h5>
      {Object.entries(node.params).map(([k, v]) => (
        <label key={k}>
          {k}
          <input
            value={String(v)}
            onChange={(e) =>
              onNode({
                params: {
                  ...node.params,
                  [k]: typeof v === 'number' ? Number(e.target.value) : e.target.value,
                },
              })
            }
          />
        </label>
      ))}

      {node.type === 'scene' && (
        <>
          <h5>Scene graph</h5>
          <ul className="tree">
            <li>
              <b>{node.label}</b>
              <ul>
                {scene.map((s) => (
                  <li key={s.id}>
                    {ICON[s.kind]} {s.name}
                    <button
                      className="x"
                      onClick={() => onNode({ sceneGraph: scene.filter((x) => x.id !== s.id) })}
                    >
                      ×
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          </ul>
          <select
            value=""
            onChange={(e) => {
              const kind = e.target.value as SceneItem['kind']
              if (!kind) return
              onNode({
                sceneGraph: [
                  ...scene,
                  { id: `${node.id}-${kind}-${Date.now().toString(36)}`, kind, name: kind },
                ],
              })
            }}
          >
            <option value="">+ add item…</option>
            {(Object.keys(ICON) as SceneItem['kind'][]).map((k) => (
              <option key={k}>{k}</option>
            ))}
          </select>
          <button onClick={onActivate}>Send scene.activate</button>
        </>
      )}

      <button onClick={onSimulate}>▶ Simulate from here</button>
    </div>
  )
}
