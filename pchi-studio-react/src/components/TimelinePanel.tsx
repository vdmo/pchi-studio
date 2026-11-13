import { useEffect, useMemo, useRef, useState, type MouseEvent, type PointerEvent } from 'react'
import Card from './Card'
import ActionList from './ActionList'
import CommandBar from './CommandBar'
import { sceneInfo } from './sceneInfo'
import type { History } from './useHistory'
import {
  DEPTS,
  actionToEvent,
  deptOf,
  scheduledCues,
  standardLanes,
  timelineEnd,
  type PchiEvent,
  type ShowCue,
  type ShowLane,
  type ShowTimeline,
  type Showchart,
} from '@pchi/sdk'

/**
 * Multi-lane timelines. Every state can have its own timeline: lanes (one per
 * department or system) holding cues placed at offsets from the moment the
 * state is entered. The runtime fires them and drops any that have not fired
 * when the show leaves the state.
 */

const SNAP = 250 // ms
const LANE_H = 56
const HEAD_W = 188
const RULER_H = 30
const MIN_CUE_PX = 70

const snap = (ms: number) => Math.max(0, Math.round(ms / SNAP) * SNAP)
const fmt = (ms: number) => {
  const s = ms / 1000
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`
}
const newId = (p: string) => `${p}${Date.now().toString(36)}${Math.floor(Math.random() * 36).toString(36)}`

interface Props {
  chart: Showchart
  history: History
  /** Logs a simulated event (playing the timeline here sends nothing to the runtime). */
  note: (e: PchiEvent) => void
  /** The scene being looked at, shared with the Show tab. */
  scene: string | null
  onScene: (id: string | null) => void
}

interface Drag {
  cue: string
  lane: string
  at: number
  length: number | undefined
  mode: 'move' | 'resize'
  moved: boolean
}

const cueText = (c: ShowCue) => {
  if (c.label) return c.label
  const first = c.action.payload ? Object.values(c.action.payload)[0] : undefined
  return `${c.action.kind}${first !== undefined && first !== '' ? ` · ${String(first)}` : ''}`
}

export default function TimelinePanel({ chart, history, note, scene, onScene }: Props) {
  const { commit } = history
  const stateId = chart.states.some((s) => s.id === scene) ? scene! : chart.initial || chart.states[0]?.id || ''
  const setStateId = (id: string) => onScene(id)
  const [pps, setPps] = useState(90) // pixels per second
  const [sel, setSel] = useState<{ lane: string; cue: string } | null>(null)
  const [playing, setPlaying] = useState(false)
  const [t, setT] = useState(0)
  const [drag, setDrag] = useState<Drag | null>(null)

  const state = chart.states.find((s) => s.id === stateId)
  const tl: ShowTimeline = state?.timeline ?? { lanes: [] }
  const info = sceneInfo(chart, stateId)
  const total = Math.max(timelineEnd(tl) + 2000, 10000, info.exit ? info.exit.ms + 1500 : 0)
  const widthPx = (total / 1000) * pps

  const setTimeline = (next: ShowTimeline, key?: string) =>
    commit({ ...chart, states: chart.states.map((s) => (s.id === stateId ? { ...s, timeline: next } : s)) }, key)
  const patchLane = (id: string, patch: Partial<ShowLane>, key?: string) =>
    setTimeline({ ...tl, lanes: tl.lanes.map((l) => (l.id === id ? { ...l, ...patch } : l)) }, key)
  const patchCue = (laneId: string, cueId: string, patch: Partial<ShowCue>, key?: string) =>
    patchLane(laneId, { cues: tl.lanes.find((l) => l.id === laneId)!.cues.map((c) => (c.id === cueId ? { ...c, ...patch } : c)) }, key)

  const selLane = sel ? tl.lanes.find((l) => l.id === sel.lane) : undefined
  const selCue = selLane?.cues.find((c) => c.id === sel?.cue)

  const addCue = (laneId: string, at: number) => {
    const lane = tl.lanes.find((l) => l.id === laneId)
    if (!lane) return
    const d = deptOf(lane.dept)
    const cue: ShowCue = { id: newId('c'), at_ms: snap(at), length_ms: 2000, action: { kind: d.kind, payload: { [d.field]: '' } } }
    patchLane(laneId, { cues: [...lane.cues, cue] })
    setSel({ lane: laneId, cue: cue.id })
  }

  const moveCue = (from: string, to: string, cueId: string, at: number, length: number | undefined) => {
    const cue = tl.lanes.find((l) => l.id === from)?.cues.find((c) => c.id === cueId)
    if (!cue) return
    const moved: ShowCue = { ...cue, at_ms: at, ...(length !== undefined ? { length_ms: length } : {}) }
    setTimeline({
      ...tl,
      lanes: tl.lanes.map((l) => {
        if (l.id === from && l.id === to) return { ...l, cues: l.cues.map((c) => (c.id === cueId ? moved : c)) }
        if (l.id === from) return { ...l, cues: l.cues.filter((c) => c.id !== cueId) }
        if (l.id === to) return { ...l, cues: [...l.cues, moved] }
        return l
      }),
    })
    setSel({ lane: to, cue: cueId })
  }

  const deleteCue = (laneId: string, cueId: string) => {
    patchLane(laneId, { cues: tl.lanes.find((l) => l.id === laneId)!.cues.filter((c) => c.id !== cueId) })
    setSel(null)
  }

  // ── dragging cues ──
  const origin = useRef<{ x: number; y: number; at: number; laneIdx: number; length: number | undefined } | null>(null)
  const startDrag = (e: PointerEvent, lane: ShowLane, cue: ShowCue, mode: Drag['mode']) => {
    if (e.button !== 0) return
    e.stopPropagation()
    try {
      e.currentTarget.setPointerCapture(e.pointerId) // keeps the drag alive outside the cue
    } catch {
      /* not an active pointer (e.g. synthetic events): dragging still works while over the lanes */
    }
    origin.current = { x: e.clientX, y: e.clientY, at: cue.at_ms, laneIdx: tl.lanes.findIndex((l) => l.id === lane.id), length: cue.length_ms }
    setSel({ lane: lane.id, cue: cue.id })
    setDrag({ cue: cue.id, lane: lane.id, at: cue.at_ms, length: cue.length_ms, mode, moved: false })
  }
  const onDragMove = (e: PointerEvent) => {
    const o = origin.current
    if (!o || !drag) return
    const dx = e.clientX - o.x
    const dy = e.clientY - o.y
    const moved = drag.moved || Math.abs(dx) > 3 || Math.abs(dy) > 3
    if (drag.mode === 'resize') {
      setDrag({ ...drag, moved, length: Math.max(SNAP, snap((o.length ?? 2000) + (dx / pps) * 1000)) })
    } else {
      const idx = Math.min(tl.lanes.length - 1, Math.max(0, o.laneIdx + Math.round(dy / LANE_H)))
      setDrag({ ...drag, moved, at: snap(o.at + (dx / pps) * 1000), lane: tl.lanes[idx].id })
    }
  }
  const endDrag = () => {
    const d = drag
    origin.current = null
    setDrag(null)
    if (!d || !d.moved) return
    const from = sel?.lane ?? d.lane
    if (d.mode === 'resize') patchCue(from, d.cue, { length_ms: d.length })
    else moveCue(from, d.lane, d.cue, d.at, d.length)
  }

  // ── playback (a local preview: nothing is sent to the runtime) ──
  const startedAt = useRef(0)
  const fired = useRef<Set<string>>(new Set())
  const cues = useMemo(() => scheduledCues(tl), [tl])
  useEffect(() => {
    if (!playing) return
    let raf = 0
    const tick = () => {
      const now = performance.now() - startedAt.current
      setT(now)
      for (const c of cues) {
        if (c.at_ms <= now && !fired.current.has(c.cue)) {
          fired.current.add(c.cue)
          note({ ...actionToEvent(c.action), simulated: true, state: stateId, lane: c.lane, cue: c.cue })
        }
      }
      if (now >= total) return setPlaying(false)
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [playing, cues, total, note, stateId])

  const togglePlay = () => {
    if (playing) return setPlaying(false)
    const from = t >= total ? 0 : t
    startedAt.current = performance.now() - from
    fired.current = new Set(cues.filter((c) => c.at_ms < from).map((c) => c.cue))
    setT(from)
    setPlaying(true)
  }
  const stop = () => {
    setPlaying(false)
    setT(0)
  }

  // ── keyboard ──
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable)) return
      const mod = e.ctrlKey || e.metaKey
      if (mod && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        e.shiftKey ? history.redo() : history.undo()
      } else if (e.key === ' ') {
        e.preventDefault()
        togglePlay()
      } else if ((e.key === 'Delete' || e.key === 'Backspace') && sel) {
        e.preventDefault()
        deleteCue(sel.lane, sel.cue)
      } else if (e.key === 'Escape') setSel(null)
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, playing, t, total, cues, chart, history.undo, history.redo])

  const addLane = () => {
    const lane: ShowLane = { id: newId('lane'), name: 'New lane', dept: 'other', cues: [] }
    setTimeline({ ...tl, lanes: [...tl.lanes, lane] })
  }

  const trackDouble = (e: MouseEvent<HTMLDivElement>, lane: ShowLane) => {
    if ((e.target as HTMLElement).closest('.tl-cue')) return
    const rect = e.currentTarget.getBoundingClientRect()
    addCue(lane.id, ((e.clientX - rect.left) / pps) * 1000)
  }
  const scrub = (e: MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    setT(Math.max(0, ((e.clientX - rect.left) / pps) * 1000))
    fired.current = new Set(cues.filter((c) => c.at_ms < ((e.clientX - rect.left) / pps) * 1000).map((c) => c.cue))
    startedAt.current = performance.now() - ((e.clientX - rect.left) / pps) * 1000
  }

  const step = pps >= 70 ? 1 : pps >= 35 ? 2 : 5
  const ticks = Array.from({ length: Math.floor(total / 1000 / step) + 1 }, (_, i) => i * step)

  if (chart.states.length === 0) {
    return (
      <div className="cv">
        <div className="cv-canvas tl-canvas">
          <div className="tl-empty">
            <h3>No states yet</h3>
            <p>Timelines belong to states. Create your states on the <b>Show</b> tab first, for example: <code>chain preshow, intro, drop, finale</code>.</p>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className={`cv ${selCue ? 'has-right' : ''}`}>
      <div className="cv-canvas tl-canvas">
        <div className="tl-cmd">
          <CommandBar
            chart={chart}
            onApply={commit}
            onUndo={history.undo}
            onRedo={history.redo}
            canUndo={history.canUndo}
            canRedo={history.canRedo}
            stateContext={stateId}
            placeholder={`Timeline commands for ${stateId}, e.g. “at 4 seconds lighting look blue”`}
          />
        </div>
        <div className="tl-bar">
          <div className="tl-states" role="tablist" aria-label="State timelines">
            {chart.states.map((s) => (
              <button
                key={s.id}
                role="tab"
                aria-selected={s.id === stateId}
                className={`chip ${s.id === stateId ? 'on' : ''}`}
                onClick={() => {
                  setStateId(s.id)
                  setSel(null)
                  stop()
                }}
                title={s.timeline?.lanes.some((l) => l.cues.length) ? 'has cues' : 'no cues yet'}
              >
                {s.id}
                {s.timeline?.lanes.some((l) => l.cues.length) ? <i aria-hidden /> : null}
              </button>
            ))}
          </div>
          <span className="spacer" />
          <div className="tl-transport">
            <button onClick={stop} aria-label="Back to start" title="Back to start">⏮</button>
            <button className={playing ? 'on' : ''} onClick={togglePlay} aria-label={playing ? 'Pause' : 'Play'} title="Play / pause (Space). A preview only: nothing is sent to the runtime.">
              {playing ? '⏸' : '▶'}
            </button>
            <span className="time" aria-live="off">{fmt(t)}</span>
          </div>
          <label className="tl-zoom" title="Zoom">
            <span aria-hidden>⌕</span>
            <input type="range" min={30} max={240} value={pps} onChange={(e) => setPps(Number(e.target.value))} aria-label="Timeline zoom" />
          </label>
          <label className="tl-dur" title="Length of this state's timeline, for the ruler">
            Length
            <input
              type="number"
              min={0}
              step={1}
              value={tl.duration_ms !== undefined ? tl.duration_ms / 1000 : ''}
              placeholder="auto"
              onChange={(e) => setTimeline({ ...tl, duration_ms: e.target.value === '' ? undefined : Math.max(0, Number(e.target.value)) * 1000 }, 'duration')}
            />
            s
          </label>
          <button onClick={addLane}>+ Lane</button>
        </div>

        <div className="tl-scene" aria-label={`How the show enters and leaves ${stateId}`}>
          <span className="muted">Scene</span>
          <b>{stateId}</b>
          <span className="muted">comes from</span>
          {info.into.length === 0 ? <i className="muted">{stateId === chart.initial ? 'the start' : 'nowhere yet'}</i> : [...new Set(info.into.map((t) => t.source))].map((id) => (
            <button key={id} className="chip" onClick={() => (setStateId(id), setSel(null), stop())}>{id}</button>
          ))}
          <span className="muted">leaves to</span>
          {info.out.length === 0 ? <i className="muted">nothing: the show ends here</i> : info.out.map((t, i) => (
            <button key={i} className="chip" onClick={() => (setStateId(t.target), setSel(null), stop())} title={info.describe(t)}>
              {t.after_ms !== undefined ? `after ${t.after_ms / 1000} s` : info.describe(t)} → {t.target}
            </button>
          ))}
        </div>

        {tl.lanes.length === 0 ? (
          <div className="tl-empty">
            <h3>No timeline for {stateId} yet</h3>
            <p>A timeline is a set of lanes, one per department or system, with cues placed at times after this state starts. They fire automatically while the show is in <b>{stateId}</b>.</p>
            <div className="btnrow">
              <button className="primary" onClick={() => setTimeline({ ...tl, lanes: standardLanes() })}>Add Visuals, Lighting, Lasers and Audio lanes</button>
              <button onClick={addLane}>Add one empty lane</button>
            </div>
          </div>
        ) : (
          <div className="tl-scroll" onPointerMove={onDragMove} onPointerUp={endDrag} onPointerCancel={endDrag} onClick={() => setSel(null)}>
            <div className="tl-grid" style={{ width: HEAD_W + widthPx }}>
              <div className="tl-ruler">
                <div className="tl-corner" style={{ width: HEAD_W }}>{stateId}</div>
                <div className="tl-ticks" style={{ width: widthPx, height: RULER_H }} onClick={(e) => (e.stopPropagation(), scrub(e))}>
                  {ticks.map((s) => (
                    <span key={s} className="tick" style={{ left: s * pps }}>
                      {s % 60 === 0 ? `${s / 60}:00` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`}
                    </span>
                  ))}
                  {info.exit && (
                    <span className="tl-exit" style={{ left: (info.exit.ms / 1000) * pps }} title={`The show leaves ${stateId} here`}>
                      → {info.exit.target}
                    </span>
                  )}
                </div>
              </div>
              {info.exit && <div className="tl-exitline" style={{ left: HEAD_W + (info.exit.ms / 1000) * pps }} aria-hidden />}

              {tl.lanes.map((lane) => {
                const d = deptOf(lane.dept)
                return (
                  <div className="tl-row" key={lane.id} style={{ height: LANE_H }}>
                    <div className="tl-head" style={{ width: HEAD_W, borderLeftColor: d.color }} onClick={(e) => e.stopPropagation()}>
                      <input value={lane.name ?? ''} aria-label="Lane name" placeholder="Lane" onChange={(e) => patchLane(lane.id, { name: e.target.value }, `lane:${lane.id}`)} />
                      <select value={lane.dept ?? 'other'} aria-label="Department" onChange={(e) => patchLane(lane.id, { dept: e.target.value })}>
                        {DEPTS.map((x) => (
                          <option key={x.id} value={x.id}>{x.label}</option>
                        ))}
                      </select>
                      <button
                        className="x"
                        aria-label={`Delete lane ${lane.name ?? ''}`}
                        title="Delete lane"
                        onClick={() => {
                          if (lane.cues.length === 0 || confirm(`Delete lane "${lane.name}" and its ${lane.cues.length} cue(s)? You can undo this.`)) {
                            setTimeline({ ...tl, lanes: tl.lanes.filter((l) => l.id !== lane.id) })
                            setSel(null)
                          }
                        }}
                      >
                        ×
                      </button>
                    </div>
                    <div
                      className="tl-track"
                      style={{ width: widthPx, backgroundSize: `${pps}px 100%` }}
                      onDoubleClick={(e) => trackDouble(e, lane)}
                      title="Double-click to add a cue"
                    >
                      {lane.cues.map((c) => {
                        const dragging = drag?.cue === c.id && drag.moved ? drag : null
                        if (dragging && dragging.lane !== lane.id) return null
                        const at = dragging && dragging.mode === 'move' ? dragging.at : c.at_ms
                        const len = dragging && dragging.mode === 'resize' ? dragging.length : c.length_ms
                        const isSel = sel?.cue === c.id
                        return (
                          <div
                            key={c.id}
                            className={`tl-cue ${isSel ? 'sel' : ''} ${dragging ? 'dragging' : ''}`}
                            style={{ left: (at / 1000) * pps, width: Math.max(((len ?? 0) / 1000) * pps, MIN_CUE_PX), background: `${d.color}33`, borderColor: d.color }}
                            onPointerDown={(e) => startDrag(e, lane, c, 'move')}
                            onClick={(e) => e.stopPropagation()}
                            title={`${cueText(c)} at ${fmt(at)}`}
                          >
                            <span>{cueText(c)}</span>
                            <i className="grip" onPointerDown={(e) => startDrag(e, lane, c, 'resize')} aria-label="Resize" />
                          </div>
                        )
                      })}
                      {/* a cue dragged into this lane from another */}
                      {drag?.moved && drag.mode === 'move' && drag.lane === lane.id && !lane.cues.some((c) => c.id === drag.cue) && (() => {
                        const src = tl.lanes.flatMap((l) => l.cues).find((c) => c.id === drag.cue)
                        return src ? (
                          <div className="tl-cue dragging" style={{ left: (drag.at / 1000) * pps, width: Math.max(((src.length_ms ?? 0) / 1000) * pps, MIN_CUE_PX), background: `${d.color}33`, borderColor: d.color }}>
                            <span>{cueText(src)}</span>
                          </div>
                        ) : null
                      })()}
                    </div>
                  </div>
                )
              })}

              <div className="tl-playhead" style={{ left: HEAD_W + (t / 1000) * pps, top: 0 }} aria-hidden />
            </div>
          </div>
        )}
        <p className="tl-hint">Double-click a lane to add a cue · drag to move, drag the right edge to resize · Space plays a preview · Ctrl+Z undoes</p>
      </div>

      {selCue && selLane && sel && (
        <Card title="Cue" className="right" onClose={() => setSel(null)}>
          <label>
            Label
            <input value={selCue.label ?? ''} placeholder={cueText({ ...selCue, label: undefined })} onChange={(e) => patchCue(sel.lane, sel.cue, { label: e.target.value || undefined }, `cl:${sel.cue}`)} />
          </label>
          <label>
            Lane
            <select
              value={sel.lane}
              onChange={(e) => moveCue(sel.lane, e.target.value, sel.cue, selCue.at_ms, selCue.length_ms)}
            >
              {tl.lanes.map((l) => (
                <option key={l.id} value={l.id}>{l.name || l.id}</option>
              ))}
            </select>
          </label>
          <div className="btnrow">
            <label>
              Starts at (s)
              <input type="number" min={0} step={0.25} value={selCue.at_ms / 1000} onChange={(e) => patchCue(sel.lane, sel.cue, { at_ms: Math.max(0, Math.round(Number(e.target.value) * 1000)) }, `ca:${sel.cue}`)} />
            </label>
            <label>
              Lasts (s)
              <input
                type="number"
                min={0}
                step={0.25}
                value={selCue.length_ms !== undefined ? selCue.length_ms / 1000 : ''}
                placeholder="–"
                onChange={(e) => patchCue(sel.lane, sel.cue, { length_ms: e.target.value === '' ? undefined : Math.max(0, Math.round(Number(e.target.value) * 1000)) }, `cn:${sel.cue}`)}
              />
            </label>
          </div>
          <ActionList title="Action" actions={[selCue.action]} onChange={(a) => a[0] && patchCue(sel.lane, sel.cue, { action: a[0] }, `cx:${sel.cue}`)} />
          <p className="muted">Fires once, {fmt(selCue.at_ms)} after {stateId} starts. “Lasts” only changes how it looks here.</p>
          <button className="danger" onClick={() => deleteCue(sel.lane, sel.cue)}>Delete cue</button>
        </Card>
      )}
    </div>
  )
}
