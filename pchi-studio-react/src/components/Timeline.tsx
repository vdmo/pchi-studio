import type { PchiCue } from '@pchi/sdk'

const PX_PER_SEC = 40

export default function Timeline({
  clips,
  nodeIds,
  onChange,
}: {
  clips: PchiCue[]
  nodeIds: string[]
  onChange: (c: PchiCue[]) => void
}) {
  const update = (id: string, patch: Partial<PchiCue>) =>
    onChange(clips.map((c) => (c.id === id ? { ...c, ...patch } : c)))
  return (
    <section className="timeline">
      <div className="bar">
        <h4>Timeline</h4>
        <button
          onClick={() =>
            nodeIds[0] &&
            onChange([
              ...clips,
              { id: `c${Date.now().toString(36)}`, node: nodeIds[0], start: 0, duration: 4 },
            ])
          }
        >
          + cue
        </button>
      </div>
      {clips.map((c) => (
        <div key={c.id} className="track">
          <select value={c.node} onChange={(e) => update(c.id, { node: e.target.value })}>
            {nodeIds.map((n) => (
              <option key={n}>{n}</option>
            ))}
          </select>
          <div className="lane">
            <div
              className="clip"
              style={{ left: c.start * PX_PER_SEC, width: c.duration * PX_PER_SEC }}
            />
          </div>
          <input
            type="number"
            min={0}
            step={0.5}
            value={c.start}
            title="start (s)"
            onChange={(e) => update(c.id, { start: Number(e.target.value) })}
          />
          <input
            type="number"
            min={0.5}
            step={0.5}
            value={c.duration}
            title="duration (s)"
            onChange={(e) => update(c.id, { duration: Number(e.target.value) })}
          />
          <button onClick={() => onChange(clips.filter((x) => x.id !== c.id))}>×</button>
        </div>
      ))}
    </section>
  )
}
