import { useEffect, useMemo, useRef, useState } from 'react'
import { clock, notable, position, replayAt, type ReplayView } from '@pchi/sdk'
import { feedToken, signedInSession } from './liveFeed'

const SPEEDS = [1, 2, 4, 8, 16]
const COLORS = ['#38bdf8', '#a78bfa', '#34d399', '#f472b6', '#fbbf24', '#fb923c', '#22d3ee', '#a3e635']

async function call(path: string, init: RequestInit = {}) {
  const t = feedToken()
  const sess = signedInSession()
  const res = await fetch(`/api${path}`, { ...init, headers: { ...(init.headers ?? {}), ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(sess ? { 'X-Pchi-Session': sess } : {}) } })
  let body: any = null
  const text = await res.text()
  try {
    body = JSON.parse(text)
  } catch {
    /* not JSON */
  }
  return { ok: res.ok, status: res.status, body, text }
}

/**
 * Go back through a run: the one just finished (or still going) or a recording from a file. Drag the bar or press play; the readout says which scene the
 * show was in, what the context was, which cues had gone, and what people decided. It only reads: nothing here touches the running show.
 */
export default function RunReplay() {
  const [view, setView] = useState<ReplayView | null>(null)
  const [source, setSource] = useState<'live' | 'file' | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [t, setT] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [speed, setSpeed] = useState(4)
  const track = useRef<HTMLDivElement>(null)
  const log = useRef<HTMLOListElement>(null)

  const load = async (path: string, init: RequestInit | undefined, from: 'live' | 'file') => {
    setBusy(true)
    setError('')
    setPlaying(false)
    const r = await call(path, init).catch(() => null)
    setBusy(false)
    if (!r) return setError('The runtime did not answer. Is it running?')
    if (!r.ok) return setError(String(r.body?.error ?? `Could not load the run (${r.status})`))
    setView(r.body as ReplayView)
    setSource(from)
    setT(0)
  }

  const save = async () => {
    const r = await call('/recording').catch(() => null)
    if (!r?.ok) return setError(String(r?.body?.error ?? 'Could not get the recording.'))
    const name = `pchi-run-${String(r.body?.show ?? 'show').replace(/[^a-z0-9_-]+/gi, '-')}-${new Date(r.body?.started_ts ?? Date.now()).toISOString().slice(0, 16).replace(/[:T]/g, '-')}.json`
    const url = URL.createObjectURL(new Blob([r.text], { type: 'application/json' }))
    const a = document.createElement('a')
    a.href = url
    a.download = name
    a.click()
    URL.revokeObjectURL(url)
  }

  const openFile = async (f: File | undefined) => {
    if (!f) return
    if (f.size > 32 * 1024 * 1024) return setError('That file is too large to be a recording.')
    load('/replay', { method: 'POST', body: await f.text(), headers: { 'Content-Type': 'application/json' } }, 'file')
  }

  // playing: the cursor moves at `speed` times real time
  useEffect(() => {
    if (!playing || !view) return
    const id = setInterval(() => {
      setT((cur) => {
        const next = cur + 100 * speed
        if (next >= view.length_ms) {
          setPlaying(false)
          return view.length_ms
        }
        return next
      })
    }, 100)
    return () => clearInterval(id)
  }, [playing, speed, view])

  const at = useMemo(() => (view ? replayAt(view, t) : null), [view, t])
  const marks = useMemo(() => (view ? notable(view) : []), [view])
  const colorOf = useMemo(() => {
    const m = new Map<string, string>()
    view?.scenes.forEach((s) => m.has(s.scene) || m.set(s.scene, COLORS[m.size % COLORS.length]))
    return (s: string) => m.get(s) ?? '#64748b'
  }, [view])

  useEffect(() => {
    log.current?.scrollTo({ top: log.current.scrollHeight })
  }, [at?.happened.length])

  const seek = (e: React.MouseEvent) => {
    if (!view || !track.current) return
    const r = track.current.getBoundingClientRect()
    setT(Math.round(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * view.length_ms))
  }

  return (
    <div className="rp">
      <div className="rt-tools">
        <button onClick={() => load('/replay', undefined, 'live')} disabled={busy}>Load this run</button>
        <label className="file-btn" title="Open a recording saved earlier (a .json file)">
          Open a recording…
          <input type="file" accept=".json,application/json" hidden onChange={(e) => { openFile(e.target.files?.[0]); e.target.value = '' }} />
        </label>
        <button onClick={save} disabled={busy} title="Save the run on the runtime as one file to keep or send">Save recording</button>
      </div>
      {error && <p className="rt-err" role="alert">{error}</p>}
      {!view && !error && <p className="muted">Load the run on the runtime, or open a recording you saved. Nothing here changes the show.</p>}

      {view && at && (
        <>
          <p className="rp-line">
            <b>{view.show ?? 'show'}</b> · {view.summary.line} <span className="muted">({source === 'file' ? 'from a file' : 'on the runtime now'})</span>
          </p>
          {view.verify && (
            <p className={view.verify.matches ? 'rt-ok' : 'rt-err'} role="status">
              {view.verify.matches ? '✓ ' : '⚠ '}
              {view.verify.verdict}
            </p>
          )}
          {view.summary.who.length > 0 && <p className="muted">Decisions by: {view.summary.who.join(', ')}</p>}

          <div className="rp-track" ref={track} onMouseDown={seek} role="slider" aria-label="Position in the run" aria-valuemin={0} aria-valuemax={view.length_ms} aria-valuenow={at.t} aria-valuetext={`${clock(at.t)} in ${at.scene}`} tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === 'ArrowRight') setT((c) => Math.min(view.length_ms, c + (e.shiftKey ? 10000 : 1000)))
              if (e.key === 'ArrowLeft') setT((c) => Math.max(0, c - (e.shiftKey ? 10000 : 1000)))
              if (e.key === ' ') { e.preventDefault(); setPlaying((p) => !p) }
            }}>
            {view.scenes.map((s, i) => {
              const from = position(view, s.from_ms)
              const to = position(view, s.to_ms ?? view.length_ms)
              return (
                <div key={i} className="rp-scene" style={{ left: `${from * 100}%`, width: `${Math.max(0.4, (to - from) * 100)}%`, background: colorOf(s.scene) }} title={`${s.scene}: ${clock(s.from_ms)} to ${clock(s.to_ms ?? view.length_ms)}`}>
                  <span>{s.scene}</span>
                </div>
              )
            })}
            {marks.map((m, i) => (
              <i key={i} className={`rp-mark ${m.kind} ${m.bad ? 'bad' : ''}`} style={{ left: `${position(view, m.t) * 100}%` }} title={`${clock(m.t)}  ${m.text}`} />
            ))}
            <div className="rp-head" style={{ left: `${position(view, at.t) * 100}%` }} />
          </div>

          <div className="rp-ctl">
            <button onClick={() => (at.t >= view.length_ms ? (setT(0), setPlaying(true)) : setPlaying((p) => !p))} aria-label={playing ? 'Pause' : 'Play'}>{playing ? '❚❚ Pause' : '▶ Play'}</button>
            <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))} aria-label="Speed">
              {SPEEDS.map((s) => <option key={s} value={s}>{s}×</option>)}
            </select>
            <span className="rp-clock">{clock(at.t)} / {clock(view.length_ms)}</span>
            <button onClick={() => { const n = view.frames.find((f) => f.t > at.t); if (n) setT(n.t) }} disabled={!at.next} title="Jump to the next moment">Next moment ▸</button>
          </div>

          <div className="rp-now">
            <div>
              <small className="muted">scene</small>
              <b className="rp-scene-name" style={{ color: colorOf(at.scene) }}>{at.scene}</b>
              <small className="muted">for {clock(at.inScene)}</small>
            </div>
            <div className="rp-ctx">
              <small className="muted">context</small>
              {Object.keys(at.context).length === 0 && <span className="muted">none</span>}
              {Object.entries(at.context).map(([k, v]) => <span key={k} className="chip"><b>{k}</b> {JSON.stringify(v)}</span>)}
            </div>
            <div>
              <small className="muted">cues gone in this scene</small>
              <span>{at.cues.length}</span>
            </div>
          </div>

          <ol className="rp-log" ref={log} aria-label="What had happened by now">
            {at.happened.slice(-60).map((f, i) => (
              <li key={`${f.t}-${i}`} className={`${f.kind} ${f.bad ? 'bad' : ''}`}>
                <time>{clock(f.t)}</time> <span className="kind">{f.kind}</span> {f.text}
              </li>
            ))}
            {at.happened.length === 0 && <li className="muted">nothing yet</li>}
          </ol>
          {at.next && <p className="muted rp-next">next, at {clock(at.next.t)}: {at.next.text}</p>}
        </>
      )}
    </div>
  )
}
