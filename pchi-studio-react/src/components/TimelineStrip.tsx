import { useState, type PointerEvent as RPointerEvent } from 'react'
import { deptOf, timelineEnd, type ShowTimeline } from '@pchi/sdk'

export const fmt = (ms: number) => {
  const s = Math.max(0, ms) / 1000
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`
}

/** Height the strip needs: the container reserves this much under the canvas. */
export const stripHeight = (lanes: number) => (lanes > 0 ? Math.min(330, 56 + lanes * 40) : 64)

const COLLAPSED = 34
const MIN_H = 90
const KEY = 'pchi.strip'
const maxH = () => Math.max(MIN_H, Math.round(window.innerHeight * 0.7))
const clamp = (v: number) => Math.round(Math.min(maxH(), Math.max(MIN_H, v)))
const load = (): { collapsed: boolean; user: number | null } => {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) || '{}')
    return { collapsed: !!v.collapsed, user: typeof v.user === 'number' ? v.user : null }
  } catch {
    return { collapsed: false, user: null }
  }
}
const save = (v: { collapsed: boolean; user: number | null }) => {
  try {
    localStorage.setItem(KEY, JSON.stringify(v))
  } catch {
    /* private window: the layout just is not remembered */
  }
}

export interface StripLayout {
  /** Height right now, including while the handle is being dragged. */
  h: number
  /** Height once the drag is over: use this to decide when to re-fit a canvas. */
  settled: number
  collapsed: boolean
  toggle: () => void
  drag: (h: number) => void
  commit: (h: number) => void
  reset: () => void
}

/**
 * How tall the timeline strip is and whether it is folded away. One setting for
 * every place the strip appears (Show and Live), remembered in this browser.
 */
export function useStripLayout(lanes: number): StripLayout {
  const [st, setSt] = useState(load)
  const [dragH, setDragH] = useState<number | null>(null)
  const settled = st.collapsed ? COLLAPSED : clamp(st.user ?? stripHeight(lanes))
  return {
    h: st.collapsed ? COLLAPSED : dragH ?? settled,
    settled,
    collapsed: st.collapsed,
    toggle: () =>
      setSt((s) => {
        const n = { ...s, collapsed: !s.collapsed }
        save(n)
        return n
      }),
    drag: (h) => setDragH(clamp(h)),
    commit: (h) => {
      setDragH(null)
      setSt(() => {
        const n = { collapsed: false, user: clamp(h) }
        save(n)
        return n
      })
    },
    reset: () =>
      setSt((s) => {
        const n = { ...s, user: null }
        save(n)
        return n
      }),
  }
}

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
  layout: StripLayout
}

/**
 * The timeline of ONE scene, drawn the same way wherever it appears: under the
 * Show canvas for the selected scene, and under the Live canvas for the running
 * one. Read-only; editing happens in the Timeline tab.
 */
export default function TimelineStrip({ state, tl, elapsed, fired, exit, onOpen, layout }: Props) {
  const lanes = tl?.lanes ?? []
  const total = Math.max(timelineEnd(tl) + 1000, 5000, tl?.duration_ms ?? 0, exit ? exit.ms + 500 : 0)
  const pct = (ms: number) => `${Math.min(100, (ms / total) * 100)}%`
  const live = elapsed !== undefined
  const p = live ? Math.min(1, Math.max(0, elapsed / total)) : 0

  // drag the top edge to make the strip taller or shorter; double-click it to go back to automatic height
  const onGrip = (e: RPointerEvent<HTMLDivElement>) => {
    if (layout.collapsed) return
    const startY = e.clientY
    const startH = layout.h
    let last = startH
    const el = e.currentTarget
    try {
      el.setPointerCapture(e.pointerId)
    } catch {
      /* synthetic pointer: the window listeners below still work */
    }
    const move = (ev: PointerEvent) => {
      last = startH + (startY - ev.clientY)
      layout.drag(last)
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      layout.commit(last)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  return (
    <section className={`live-tl ${layout.collapsed ? 'folded' : ''}`} aria-label={`Timeline for ${state}`}>
      {!layout.collapsed && <div className="tl-grip" role="separator" aria-orientation="horizontal" aria-label="Drag to resize the timeline" title="Drag to resize · double-click for automatic height" onPointerDown={onGrip} onDoubleClick={layout.reset} />}
      <header>
        <button className="fold" onClick={layout.toggle} aria-expanded={!layout.collapsed} title={layout.collapsed ? 'Show the timeline' : 'Collapse the timeline'}>{layout.collapsed ? '▴' : '▾'}</button>
        <b>{state}</b>
        <span className="muted">timeline</span>
        {exit && <span className="muted">· leaves after {exit.ms / 1000} s → {exit.target}</span>}
        <span className="spacer" />
        {live && <span className="time">{fmt(elapsed)}</span>}
        <span className="muted">{live ? '/ ' : 'length '}{fmt(total)}</span>
        {onOpen && <button onClick={onOpen} title="Edit this scene's timeline">{lanes.length ? 'Edit timeline' : 'Add a timeline'}</button>}
      </header>
      {layout.collapsed ? null : lanes.length === 0 ? (
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
