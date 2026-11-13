import { deptOf, laneDevice, type PchiNode, type Showchart } from '@pchi/sdk'
import { fmt } from './TimelineStrip'

/** How long before a cue it is called "standby". */
export const STANDBY_MS = 10_000

type Row = { id: string; num?: number; at: number; dept: string; device?: string; text: string; lane: string }

const rowsOf = (def: Showchart, scene: string | undefined, devices: PchiNode[]): Row[] =>
  (def.states.find((s) => s.id === scene)?.timeline?.lanes ?? [])
    .flatMap((l) =>
      l.cues.map((c) => {
        const first = c.action.payload ? Object.values(c.action.payload)[0] : undefined
        return {
          id: c.id,
          num: c.num,
          at: c.at_ms,
          dept: l.dept ?? 'other',
          device: laneDevice(devices, l)?.label,
          lane: l.name || l.id,
          text: c.label || `${c.action.kind}${first !== undefined && first !== '' ? ` · ${String(first)}` : ''}`,
        }
      }),
    )
    .sort((a, b) => a.at - b.at)

/**
 * The cue sheet, following the running show. The cue that is next is called
 * STANDBY (within 10 s) and GO (due); cues already fired are struck through.
 * The scene after this one is listed below, greyed, so the next call is known.
 */
export default function LiveCallSheet({ def, scene, elapsed, fired, devices, nextScene }: { def: Showchart; scene: string; elapsed: number; fired: Set<string>; devices: PchiNode[]; nextScene?: string }) {
  const rows = rowsOf(def, scene, devices)
  const nextRows = nextScene && nextScene !== scene ? rowsOf(def, nextScene, devices) : []
  const first = rows.find((r) => !fired.has(r.id))
  return (
    <div className="lcs">
      <h5>Scene {scene}</h5>
      {rows.length === 0 && <p className="muted">No cues in this scene.</p>}
      <ol className="lcs-list">
        {rows.map((r) => {
          const done = fired.has(r.id)
          const left = r.at - elapsed
          const call = done ? 'done' : left <= 0 ? 'go' : left <= STANDBY_MS ? 'standby' : ''
          return (
            <li key={r.id} className={`lcs-row ${call} ${r.id === first?.id ? 'next' : ''}`} style={{ ['--dept' as string]: deptOf(r.dept).color }}>
              <span className="lcs-q">{r.num !== undefined ? `Q${r.num}` : ''}</span>
              <span className="lcs-t">{fmt(r.at)}</span>
              <span className="lcs-what">
                {r.text}
                <small>{deptOf(r.dept).label}{r.device ? ` · ${r.device}` : ''}</small>
              </span>
              <b className="lcs-call">{call === 'standby' ? `STANDBY ${fmt(left)}` : call === 'go' ? 'GO' : call === 'done' ? '✓' : ''}</b>
            </li>
          )
        })}
      </ol>
      {nextRows.length > 0 && (
        <>
          <h5>Then {nextScene}</h5>
          <ol className="lcs-list later">
            {nextRows.slice(0, 5).map((r) => (
              <li key={r.id} className="lcs-row" style={{ ['--dept' as string]: deptOf(r.dept).color }}>
                <span className="lcs-q">{r.num !== undefined ? `Q${r.num}` : ''}</span>
                <span className="lcs-t">{fmt(r.at)}</span>
                <span className="lcs-what">
                  {r.text}
                  <small>{deptOf(r.dept).label}{r.device ? ` · ${r.device}` : ''}</small>
                </span>
              </li>
            ))}
          </ol>
        </>
      )}
    </div>
  )
}
