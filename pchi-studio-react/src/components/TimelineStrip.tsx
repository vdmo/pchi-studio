import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as RKeyboardEvent, type MouseEvent as RMouseEvent, type PointerEvent as RPointerEvent } from 'react'
import { allowedPrograms, cueValueField, cueValueOf, deptOf, fmtTick, fmtTime, laneDevice, nudgeMs, parseTime, rulerTicks, snapMs, timelineEnd, withCueValue, type PchiNode, type ShowCue, type ShowTimeline } from '@pchi/sdk'

/** A time as m:ss.s with exactly the decimals it needs (see fmtTime): what is shown is what is stored. */
export const fmt = fmtTime

/** Height the strip needs: the container reserves this much under the canvas (a ruler row and the lanes). */
export const stripHeight = (lanes: number) => (lanes > 0 ? Math.max(MIN_H, Math.min(380, 124 + lanes * 36, Math.round(window.innerHeight * 0.42))) : 64) // never more than ~40% of the window: the canvas needs room too

const COLLAPSED = 34
const MIN_H = 90
const KEY = 'pchi.strip'
const SNAP_KEY = 'pchi.strip.snap'
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

// ───────── the grid the strip snaps to (remembered in this browser) ─────────

const SNAPS: { ms: number; label: string }[] = [
  { ms: 0, label: 'No snap' },
  { ms: 100, label: '0.1 s' },
  { ms: 250, label: '0.25 s' },
  { ms: 500, label: '0.5 s' },
  { ms: 1000, label: '1 s' },
]
const loadSnap = (): number => {
  try {
    const v = Number(localStorage.getItem(SNAP_KEY))
    return SNAPS.some((s) => s.ms === v) && localStorage.getItem(SNAP_KEY) !== null ? v : 250
  } catch {
    return 250
  }
}

const LNAME = 112 // px: the lane-name column (matches --lname-w in the stylesheet)
const MAX_ZOOM = 24
const newId = (p: string) => `${p}${Date.now().toString(36)}${Math.floor(Math.random() * 36).toString(36)}`

interface Drag {
  cue: string
  lane: string
  at: number
  length: number | undefined
  mode: 'move' | 'resize'
  moved: boolean
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
  /** Editing view: hand back the changed timeline. With it the strip edits in place; without it (Live) it only shows. */
  onChange?: (next: ShowTimeline, key?: string) => void
  /** The rig, so a robot lane can suggest the programs its device allows. */
  nodes?: PchiNode[]
  layout: StripLayout
}

/**
 * The timeline of ONE scene, drawn the same way wherever it appears: under the Show canvas for the selected scene, and under the
 * Live canvas for the running one. It has a ruler with real time marks, a readout of the time under the pointer, and zoom.
 * Where `onChange` is given (the Show tab) cues can be edited right here: drag to move, drag the right edge to resize, click and
 * type an exact time, arrow keys to nudge, double-click a lane to add one. Live is read-only.
 */
export default function TimelineStrip({ state, tl, elapsed, fired, exit, onOpen, onChange, nodes, layout }: Props) {
  const lanes = tl?.lanes ?? []
  const total = Math.max(timelineEnd(tl) + 1000, 5000, tl?.duration_ms ?? 0, exit ? exit.ms + 500 : 0)
  const live = elapsed !== undefined
  const editable = !!onChange && !live

  // ── scale: zoom 1 fits the whole scene; more zooms in and the strip scrolls sideways ──
  const body = useRef<HTMLDivElement>(null)
  const rtrack = useRef<HTMLDivElement>(null)
  const [bodyW, setBodyW] = useState(0)
  const [zoom, setZoom] = useState(1)
  const anchor = useRef<{ ms: number; x: number } | null>(null) // keeps the time under the pointer still while zooming
  useEffect(() => {
    const el = body.current
    if (!el) return
    const measure = () => setBodyW(el.clientWidth)
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [layout.collapsed, lanes.length === 0])
  const trackW = Math.max(120, bodyW - LNAME - 1) * zoom // one pixel under the body, so "fits" never adds a scrollbar
  const pps = trackW / (total / 1000)
  const px = (ms: number) => (ms / total) * trackW
  const pct = (ms: number) => `${(ms / total) * 100}%`
  const { ticks, majorMs } = rulerTicks(total, pps)

  const setZoomAt = (z: number, clientX?: number) => {
    const next = Math.min(MAX_ZOOM, Math.max(1, z))
    const r = rtrack.current?.getBoundingClientRect()
    if (r && clientX !== undefined) anchor.current = { ms: ((clientX - r.left) / r.width) * total, x: clientX - (body.current?.getBoundingClientRect().left ?? 0) }
    setZoom(next)
  }
  useLayoutEffect(() => {
    const a = anchor.current
    anchor.current = null
    if (a && body.current) body.current.scrollLeft = Math.max(0, LNAME + px(a.ms) - a.x)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoom])
  // Ctrl + wheel zooms (a plain wheel still scrolls the lanes). Native listener: React's wheel handler cannot cancel the page zoom.
  const zoomRef = useRef(zoom)
  zoomRef.current = zoom
  useEffect(() => {
    const el = body.current
    if (!el) return
    const h = (e: WheelEvent) => {
      if (!e.ctrlKey) return
      e.preventDefault()
      setZoomAt(zoomRef.current * (e.deltaY < 0 ? 1.25 : 0.8), e.clientX)
    }
    el.addEventListener('wheel', h, { passive: false })
    return () => el.removeEventListener('wheel', h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout.collapsed, lanes.length === 0, total])
  // Live, zoomed in: keep the playhead in view
  useEffect(() => {
    const el = body.current
    if (!live || zoom <= 1 || !el) return
    const x = LNAME + px(elapsed!)
    if (x < el.scrollLeft + LNAME + 8 || x > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = Math.max(0, x - el.clientWidth / 2)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [elapsed])

  // ── the time under the pointer ──
  const [hover, setHover] = useState<number | null>(null)
  const msAt = (clientX: number) => {
    const r = rtrack.current?.getBoundingClientRect()
    return r && r.width > 0 ? ((clientX - r.left) / r.width) * total : null
  }
  const onMove = (e: RPointerEvent<HTMLDivElement>) => {
    const ms = msAt(e.clientX)
    // the time a click would land on: the grid line when editing, else the nearest tenth of a second (never a noisy 3.162)
    setHover(ms === null || ms < 0 || ms > total ? null : editable ? Math.min(total, snapMs(ms, e.altKey ? 0 : snapStep || 10)) : Math.round(ms / 100) * 100)
  }

  // ── editing ──
  const [snapStep, setSnapStep] = useState(loadSnap)
  const chooseSnap = (v: number) => {
    setSnapStep(v)
    try {
      localStorage.setItem(SNAP_KEY, String(v))
    } catch {
      /* not remembered */
    }
  }
  const [sel, setSel] = useState<string | null>(null)
  const [justAdded, setJustAdded] = useState<string | null>(null)
  const [drag, setDrag] = useState<Drag | null>(null)
  const origin = useRef<{ x: number; at: number; length: number | undefined; w: number } | null>(null)
  // forget the selection when the scene changes or the cue is gone
  useEffect(() => setSel(null), [state])
  const found = sel ? lanes.flatMap((l) => l.cues.map((c) => ({ l, c }))).find((x) => x.c.id === sel) : undefined
  useEffect(() => {
    if (sel && !found) setSel(null)
  }, [sel, found])

  const patchCue = (cueId: string, patch: Partial<ShowCue>, key?: string) => {
    if (!tl || !onChange) return
    onChange({ ...tl, lanes: tl.lanes.map((l) => ({ ...l, cues: l.cues.map((c) => (c.id === cueId ? { ...c, ...patch } : c)) })) }, key)
  }
  const setValue = (cueId: string, value: string) => {
    if (!tl || !onChange) return
    onChange({ ...tl, lanes: tl.lanes.map((l) => ({ ...l, cues: l.cues.map((c) => (c.id === cueId ? withCueValue(c, l.dept, value) : c)) })) })
  }
  const removeCue = (cueId: string) => {
    if (!tl || !onChange) return
    onChange({ ...tl, lanes: tl.lanes.map((l) => ({ ...l, cues: l.cues.filter((c) => c.id !== cueId) })) })
    setSel(null)
  }
  const snapped = (ms: number, free: boolean) => Math.min(total, snapMs(ms, free ? 0 : snapStep))

  const down = (e: RPointerEvent<HTMLElement>, laneId: string, c: ShowCue, mode: Drag['mode']) => {
    if (!editable || e.button !== 0) return
    e.stopPropagation()
    setSel(c.id)
    const r = rtrack.current?.getBoundingClientRect()
    if (!r) return
    try {
      e.currentTarget.setPointerCapture(e.pointerId) // keeps the drag alive when the pointer leaves the cue
    } catch {
      /* a synthetic pointer: the drag still follows while over the cue */
    }
    origin.current = { x: e.clientX, at: c.at_ms, length: c.length_ms, w: r.width }
    setDrag({ cue: c.id, lane: laneId, at: c.at_ms, length: c.length_ms, mode, moved: false })
  }
  const move = (e: RPointerEvent<HTMLElement>) => {
    const o = origin.current
    if (!o || !drag) return
    const dx = e.clientX - o.x
    const dms = (dx / o.w) * total
    const moved = drag.moved || Math.abs(dx) > 3
    if (drag.mode === 'resize') setDrag({ ...drag, moved, length: Math.max(snapStep || 100, snapped((o.length ?? 0) + dms, e.altKey)) })
    else setDrag({ ...drag, moved, at: snapped(o.at + dms, e.altKey) })
  }
  const up = () => {
    const d = drag
    origin.current = null
    setDrag(null)
    if (!d || !d.moved) return
    if (d.mode === 'resize') patchCue(d.cue, { length_ms: d.length })
    else patchCue(d.cue, { at_ms: d.at })
  }
  const cueKey = (e: RKeyboardEvent<HTMLDivElement>, c: ShowCue) => {
    if (!editable) return
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault()
      e.stopPropagation()
      // one key per cue, so holding an arrow is a single undo step
      patchCue(c.id, { at_ms: Math.min(total, nudgeMs(c.at_ms, e.key === 'ArrowRight' ? 1 : -1, snapStep, e.shiftKey)) }, `nudge:${c.id}`)
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault()
      e.stopPropagation() // the canvas also listens for Delete (it removes the selected scene)
      removeCue(c.id)
    } else if (e.key === 'Escape') {
      e.stopPropagation()
      setSel(null)
      e.currentTarget.blur()
    }
  }
  const addAt = (e: RMouseEvent<HTMLDivElement>, laneId: string) => {
    if (!editable || !tl || (e.target as HTMLElement).closest('.lcue')) return
    const r = e.currentTarget.getBoundingClientRect()
    const lane = tl.lanes.find((l) => l.id === laneId)
    if (!lane || r.width <= 0) return
    const d = deptOf(lane.dept)
    const cue: ShowCue = { id: newId('c'), at_ms: snapped(((e.clientX - r.left) / r.width) * total, e.altKey), length_ms: 2000, action: { kind: d.kind, payload: { [d.field]: '' } } }
    onChange!({ ...tl, lanes: tl.lanes.map((l) => (l.id === laneId ? { ...l, cues: [...l.cues, cue] } : l)) })
    setSel(cue.id)
    setJustAdded(cue.id) // its Value field takes focus, so the next thing typed is what the cue does
  }

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
    const mv = (ev: PointerEvent) => {
      last = startH + (startY - ev.clientY)
      layout.drag(last)
    }
    const upG = () => {
      window.removeEventListener('pointermove', mv)
      window.removeEventListener('pointerup', upG)
      layout.commit(last)
    }
    window.addEventListener('pointermove', mv)
    window.addEventListener('pointerup', upG)
  }

  const dragging = drag?.moved ? drag : null
  const readout = dragging ? dragging.at : hover
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
        {!layout.collapsed && lanes.length > 0 && (
          <span className="strip-tools">
            {editable && (
              <label className="strip-snap" title="Cues snap to this grid when you drag or nudge them. Hold Alt while dragging to place freely.">
                Snap
                <select value={snapStep} onChange={(e) => chooseSnap(Number(e.target.value))} aria-label="Snap to">
                  {SNAPS.map((s) => <option key={s.ms} value={s.ms}>{s.label}</option>)}
                </select>
              </label>
            )}
            <span className="strip-zoom" role="group" aria-label="Zoom the timeline">
              <button onClick={() => setZoomAt(zoom / 1.5)} disabled={zoom <= 1} aria-label="Zoom out" title="Zoom out (Ctrl + scroll)">−</button>
              <button onClick={() => setZoom(1)} disabled={zoom === 1} aria-label="Fit the whole scene" title="Fit the whole scene">{zoom === 1 ? 'Fit' : `${Math.round(zoom * 100)}%`}</button>
              <button onClick={() => setZoomAt(zoom * 1.5)} disabled={zoom >= MAX_ZOOM} aria-label="Zoom in" title="Zoom in (Ctrl + scroll)">+</button>
            </span>
          </span>
        )}
        {onOpen && <button onClick={onOpen} title="Edit this scene's timeline">{lanes.length ? 'Edit timeline' : 'Add a timeline'}</button>}
      </header>
      {editable && found && !layout.collapsed && (
        <CueEditor
          key={found.c.id}
          cue={found.c}
          dept={found.l.dept}
          laneName={found.l.name || found.l.id}
          total={total}
          suggestions={found.l.dept === 'motion' && nodes ? (() => { const d = laneDevice(nodes, found.l); return d ? allowedPrograms(d) : [] })() : []}
          autoFocusValue={justAdded === found.c.id}
          onValueBlur={() => setJustAdded(null)}
          onTime={(ms) => patchCue(found.c.id, { at_ms: ms })}
          onLength={(ms) => patchCue(found.c.id, { length_ms: ms })}
          onValue={(v) => setValue(found.c.id, v)}
          onDelete={() => removeCue(found.c.id)}
        />
      )}
      {layout.collapsed ? null : lanes.length === 0 ? (
        <p className="muted tl-none">{state} has no timeline yet: nothing fires while the show is in it.</p>
      ) : (
        <div className="live-tl-body" ref={body} onPointerMove={onMove} onPointerLeave={() => setHover(null)}>
          <div className="live-tl-inner" style={{ width: LNAME + trackW }}>
            <div className="live-ruler">
              <span className="lname rspace" aria-hidden />
              <div className="ltrack rtrack" ref={rtrack} role="presentation">
                {ticks.map((t) => (
                  <span key={t.ms} className={`rtick ${t.major ? 'major' : ''}`} style={{ left: pct(t.ms) }}>
                    {t.major && <i>{fmtTick(t.ms, majorMs)}</i>}
                  </span>
                ))}
                {readout !== null && <span className={`rread ${dragging ? 'drag' : ''}`} style={{ left: `clamp(40px, ${pct(readout)}, calc(100% - 40px))` }}>{fmt(readout)}{dragging?.mode === 'resize' && dragging.length !== undefined ? ` · ${fmt(dragging.length)} long` : ''}</span>}
              </div>
            </div>
            {lanes.map((l) => {
              const d = deptOf(l.dept)
              return (
                <div className="live-lane" key={l.id}>
                  <span className="lname" style={{ borderLeftColor: d.color }} title={l.name || l.id}>
                    {l.name || l.id}
                  </span>
                  <div className={`ltrack ${editable ? 'editable' : ''}`} onDoubleClick={(e) => addAt(e, l.id)}>
                    {l.cues.map((c) => {
                      const live1 = drag && drag.cue === c.id ? drag : null
                      const at = live1 ? live1.at : c.at_ms
                      const len = live1 ? live1.length : c.length_ms
                      const done = !!fired?.has(c.id)
                      const first = c.action.payload ? Object.values(c.action.payload)[0] : undefined
                      const text = `${c.num !== undefined ? `Q${c.num} ` : ''}${c.label ?? `${c.action.kind}${first !== undefined && first !== '' ? ` · ${String(first)}` : ''}`}`
                      const cls = !live ? 'upcoming' : done ? 'fired' : elapsed < c.at_ms ? 'upcoming' : 'due'
                      const isSel = editable && sel === c.id
                      // a box too narrow for its label and its time shows only the label (the time is in the tooltip and the editor)
                      const compact = px(len ?? 0) < 118
                      return (
                        <div
                          key={c.id}
                          className={`lcue ${cls} ${editable ? 'edit' : ''} ${isSel ? 'sel' : ''} ${live1?.moved ? 'moving' : ''} ${compact ? 'compact' : ''}`}
                          style={{ left: pct(at), width: pct(len ?? 0), background: `${d.color}${done ? '55' : '22'}`, borderColor: d.color }}
                          title={`${text} at ${fmt(at)}${len ? `, ${fmt(len)} long` : ''}${done ? ' (fired)' : ''}`}
                          {...(editable
                            ? {
                                role: 'button',
                                tabIndex: 0,
                                'aria-label': `${text} at ${fmt(at)}. Arrow keys move it, Delete removes it.`,
                                'aria-pressed': isSel,
                                onPointerDown: (e: RPointerEvent<HTMLDivElement>) => down(e, l.id, c, 'move'),
                                onPointerMove: move,
                                onPointerUp: up,
                                onPointerCancel: up,
                                onKeyDown: (e: RKeyboardEvent<HTMLDivElement>) => cueKey(e, c),
                                onFocus: () => setSel(c.id),
                              }
                            : {})}
                        >
                          <span className="ltxt">{done ? '✓ ' : ''}{text}</span>
                          <span className="lt">{fmt(at)}</span>
                          {/* the grip handles its own pointer events and keeps them from the cue, or a resize would commit twice */}
                          {editable && (
                            <span
                              className="lgrip"
                              onPointerDown={(e) => down(e, l.id, c, 'resize')}
                              onPointerMove={(e) => (e.stopPropagation(), move(e))}
                              onPointerUp={(e) => (e.stopPropagation(), up())}
                              onPointerCancel={(e) => (e.stopPropagation(), up())}
                              title="Drag to change how long it lasts"
                              aria-hidden
                            />
                          )}
                        </div>
                      )
                    })}
                  </div>
                </div>
              )
            })}
            {exit && <div className="live-exit" style={{ left: LNAME + px(exit.ms) }} title={`The show moves on to ${exit.target} here`} aria-hidden />}
            {live && <div className="live-playhead" style={{ left: LNAME + px(elapsed!) }} aria-hidden />}
            {readout !== null && <div className={`live-hair ${dragging ? 'drag' : ''}`} style={{ left: LNAME + px(readout) }} aria-hidden />}
          </div>
        </div>
      )}
    </section>
  )
}

interface EditorProps {
  cue: ShowCue
  dept?: string
  laneName: string
  total: number
  /** Values worth offering for this cue (a robot's allowed programs). */
  suggestions: string[]
  autoFocusValue: boolean
  onValueBlur: () => void
  onTime: (ms: number) => void
  onLength: (ms: number | undefined) => void
  onValue: (value: string) => void
  onDelete: () => void
}

/**
 * The selected cue, typed: what it does (Value), its exact time and its length. `12.5` or `0:12.5` for times.
 * Enter or leaving a box applies it; Esc puts the old value back.
 */
function CueEditor({ cue, dept, laneName, total, suggestions, autoFocusValue, onValueBlur, onTime, onLength, onValue, onDelete }: EditorProps) {
  return (
    <div className="strip-edit" role="group" aria-label="Selected cue">
      <b>{cue.num !== undefined ? `Q${cue.num}` : 'Cue'}</b>
      <span className="muted">{laneName}</span>
      <ValueField cue={cue} dept={dept} suggestions={suggestions} autoFocus={autoFocusValue} onBlurDone={onValueBlur} onCommit={onValue} />
      <TimeField label="Time" ms={cue.at_ms} max={total} onCommit={(ms) => ms !== null && onTime(ms)} />
      <TimeField label="Length" ms={cue.length_ms} onCommit={(ms) => onLength(ms === null || ms === 0 ? undefined : ms)} blankOk />
      <span className="spacer" />
      <button onClick={onDelete} title="Remove this cue (Delete)">Remove</button>
    </div>
  )
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)

/** What the cue does: the scene, look, cue or program, by the department's own word. Whether it is a value the tool accepts is the Checks panel's job. */
function ValueField({ cue, dept, suggestions, autoFocus, onBlurDone, onCommit }: { cue: ShowCue; dept?: string; suggestions: string[]; autoFocus: boolean; onBlurDone: () => void; onCommit: (v: string) => void }) {
  const field = cueValueField(cue, dept)
  const shown = cueValueOf(cue, dept)
  const [text, setText] = useState(shown)
  const cancelled = useRef(false)
  useEffect(() => setText(shown), [shown])
  const list = suggestions.length ? `strip-values-${cue.id}` : undefined
  const commit = () => {
    onBlurDone()
    if (cancelled.current) {
      cancelled.current = false
      return setText(shown)
    }
    const v = text.trim()
    if (v === shown) return setText(shown)
    onCommit(v)
  }
  return (
    <label className="strip-time strip-value">
      {cap(field)}
      <input
        value={text}
        spellCheck={false}
        autoFocus={autoFocus}
        list={list}
        placeholder={`what to ${field === 'program' ? 'run' : 'fire'}`}
        aria-label={`${cap(field)} of the selected cue`}
        title={`${cap(field)}: what this cue does. Type it, then Enter (Esc cancels).`}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          e.stopPropagation()
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
          else if (e.key === 'Escape') {
            cancelled.current = true
            ;(e.target as HTMLInputElement).blur()
          }
        }}
      />
      {list && (
        <datalist id={list}>
          {suggestions.map((s) => <option key={s} value={s} />)}
        </datalist>
      )}
    </label>
  )
}

function TimeField({ label, ms, max, onCommit, blankOk }: { label: string; ms: number | undefined; max?: number; onCommit: (ms: number | null) => void; blankOk?: boolean }) {
  const shown = ms === undefined ? '' : fmt(ms)
  const [text, setText] = useState(shown)
  const cancelled = useRef(false) // Esc: the blur that follows must not apply what was typed
  useEffect(() => setText(shown), [shown])
  const parsed = text.trim() === '' ? (blankOk ? null : undefined) : parseTime(text)
  const bad = parsed === undefined || (text.trim() !== '' && parsed === null)
  const commit = () => {
    if (cancelled.current) {
      cancelled.current = false
      return setText(shown)
    }
    if (bad) return setText(shown)
    if (parsed === (ms ?? null)) return setText(shown)
    onCommit(parsed === null ? null : max !== undefined ? Math.min(max, parsed) : parsed)
  }
  return (
    <label className={`strip-time ${bad ? 'bad' : ''}`}>
      {label}
      <input
        value={text}
        inputMode="decimal"
        spellCheck={false}
        placeholder={blankOk ? 'none' : '0:00.0'}
        aria-label={`${label} of the selected cue`}
        aria-invalid={bad}
        title={bad ? 'Type seconds (12.5) or minutes:seconds (0:12.5)' : `${label}: type seconds (12.5) or minutes:seconds (0:12.5), then Enter`}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          e.stopPropagation() // typing here is not a scene or cue shortcut
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
          else if (e.key === 'Escape') {
            cancelled.current = true
            ;(e.target as HTMLInputElement).blur()
          }
        }}
      />
    </label>
  )
}
