import { deptOf, timelineEnd, type ShowTimeline } from '@pchi/sdk'

export const fmt = (ms: number) => {
  const s = Math.max(0, ms) / 1000
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`
}

/** Height the strip needs: the container reserves this much under the canvas. */
export const stripHeight = (lanes: number) => (lanes > 0 ? Math.min(240, 46 + lanes * 32) : 58)

interface Props {
  state: string
  tl?: ShowTimeline
  /** Live: time since the state was entered, which draws the playhead. Omit when editing. */
  elapsed?: number
  /** Live: ids of cues the server has fired. */
  fired?: Set<string>
  /** When the show leaves this scene by itself: drawn as a marker, and it sets the strip's length. */
  exit?: { ms: number; target: string }
  /** Editing view: opens this scene in the Timeline tab. */
  onOpen?: () => void
}

/**
 * The timeline of ONE scene, drawn the same way wherever it appears: under the
 * Show canvas for the selected scene, and under the Live canvas for the running
 * one. Read-only; editing happens in the Timeline tab.
 */
export default function TimelineStrip({ state, tl, elapsed, fired, exit, onOpen }: Props) {
  const lanes = tl?.lanes ?? []
  const total = Math.max(timelineEnd(tl) + 1000, 5000, tl?.duration_ms ?? 0, exit ? exit.ms + 500 : 0)
  const pct = (ms: number) => `${Math.min(100, (ms / total) * 100)}%`
  const live = elapsed !== undefined
  const p = live ? Math.min(1, Math.max(0, elapsed / total)) : 0
  return (
    <section className="live-tl" aria-label={`Timeline for ${state}`}>
      <header>
        <b>{state}</b>
        <span className="muted">timeline</span>
        {exit && <span className="muted">· leaves after {exit.ms / 1000} s → {exit.target}</span>}
        <span className="spacer" />
        {live && <span className="time">{fmt(elapsed)}</span>}
        <span className="muted">{live ? '/ ' : 'length '}{fmt(total)}</span>
        {onOpen && <button onClick={onOpen} title="Edit this scene's timeline">{lanes.length ? 'Edit timeline' : 'Add a timeline'}</button>}
      </header>
      {lanes.length === 0 ? (
        <p className="muted tl-none">{state} has no timeline yet: nothing fires while the show is in it.</p>
      ) : (
        <div className="live-tl-body">
          {lanes.map((l) => {
            const d = deptOf(l.dept)
            return (
              <div className="live-lane" key={l.id}>
                <span className="lname" style={{ borderLeftColor: d.color }} title={l.name || l.id}>
                  {l.name || l.id}
                </span>
                <div className="ltrack">
                  {l.cues.map((c) => {
                    const done = !!fired?.has(c.id)
                    const first = c.action.payload ? Object.values(c.action.payload)[0] : undefined
                    const text = c.label ?? `${c.action.kind}${first !== undefined && first !== '' ? ` · ${String(first)}` : ''}`
                    const cls = !live ? 'upcoming' : done ? 'fired' : elapsed < c.at_ms ? 'upcoming' : 'due'
                    return (
                      <div
                        key={c.id}
                        className={`lcue ${cls}`}
                        style={{ left: pct(c.at_ms), width: pct(c.length_ms ?? 0), background: `${d.color}${done ? '55' : '22'}`, borderColor: d.color }}
                        title={`${text} at ${fmt(c.at_ms)}${done ? ' (fired)' : ''}`}
                      >
                        <span>{done ? '✓ ' : ''}{text}</span>
                      </div>
                    )
                  })}
                </div>
              </div>
            )
          })}
          {exit && <div className="live-exit" style={{ left: `calc(var(--lname-w) + (100% - var(--lname-w)) * ${exit.ms / total})` }} title={`The show moves on to ${exit.target} here`} aria-hidden />}
          {live && <div className="live-playhead" style={{ left: `calc(var(--lname-w) + (100% - var(--lname-w)) * ${p})` }} aria-hidden />}
        </div>
      )}
    </section>
  )
}
