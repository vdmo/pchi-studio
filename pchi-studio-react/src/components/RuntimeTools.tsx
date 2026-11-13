import { useEffect, useRef, useState } from 'react'
import { feedToken } from './liveFeed'

/** Calls the runtime's HTTP side (the dev server forwards /api). */
async function api(path: string, init: RequestInit = {}) {
  const t = feedToken()
  const res = await fetch(`/api${path}`, { ...init, headers: { ...(init.headers ?? {}), ...(t ? { Authorization: `Bearer ${t}` } : {}) } })
  let body: unknown = null
  try {
    body = await res.json()
  } catch {
    /* not JSON */
  }
  return { ok: res.ok, status: res.status, body: body as Record<string, any> | null }
}

type Diff = { changes: string[]; notes: string[]; blockers: string[] }

function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.focus()
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog wide" role="dialog" aria-modal="true" aria-label={title} tabIndex={-1} ref={ref}>
        <header>
          <h3>{title}</h3>
          <button onClick={onClose} aria-label="Close">×</button>
        </header>
        <div className="dialog-body">{children}</div>
      </div>
    </div>
  )
}

const sec = (ms: number) => `${(ms / 1000).toFixed(2)}s`

/**
 * Two things an operator needs around a running show: send the edited show to it
 * (after seeing exactly what would change), and read back how the run went.
 */
export default function RuntimeTools({ getFile }: { getFile: () => unknown }) {
  const [send, setSend] = useState<{ diff?: Diff; error?: string; done?: boolean; busy?: boolean } | null>(null)
  const [report, setReport] = useState<Record<string, any> | { error: string } | null>(null)

  const preview = async () => {
    setSend({ busy: true })
    const r = await api('/show?dry=1', { method: 'POST', body: JSON.stringify(getFile()), headers: { 'Content-Type': 'application/json' } }).catch(() => null)
    if (!r) return setSend({ error: 'The runtime did not answer. Is it running?' })
    if (r.status === 409) return setSend({ diff: r.body?.diff, error: r.body?.error })
    if (!r.ok) return setSend({ error: String(r.body?.error ?? `The runtime refused it (${r.status}).`) })
    setSend({ diff: r.body?.diff })
  }
  const apply = async () => {
    setSend((s) => ({ ...s, busy: true }))
    const r = await api('/show', { method: 'POST', body: JSON.stringify(getFile()), headers: { 'Content-Type': 'application/json' } }).catch(() => null)
    if (!r?.ok) return setSend((s) => ({ ...s, busy: false, error: String(r?.body?.error ?? 'The runtime did not take it.') }))
    setSend({ diff: r.body?.diff, done: true })
  }
  const openReport = async () => {
    setReport({ error: 'Loading…' })
    const r = await api('/report').catch(() => null)
    setReport(r?.ok ? (r.body ?? {}) : { error: 'Could not read the report. Is the runtime running?' })
  }

  const d = send?.diff
  const nothing = d && d.changes.length === 0 && d.blockers.length === 0
  return (
    <>
      <div className="rt-tools">
        <button onClick={preview} title="Show what would change in the running show, then send it">Send show to runtime…</button>
        <button onClick={openReport} title="How the run went: cue timing, failed sends, edits made mid-show">Run report</button>
      </div>

      {send && (
        <Dialog title="Send show to the running runtime" onClose={() => setSend(null)}>
          {send.busy && <p className="muted">Working…</p>}
          {send.error && <p className="rt-err" role="alert">{send.error}</p>}
          {send.done && <p className="rt-ok" role="status">Sent. The running show now uses your edited version. The scene, its clock and the context were kept.</p>}
          {d && (
            <>
              {nothing && <p className="muted">The runtime already has this show. Nothing to send.</p>}
              {d.changes.length > 0 && (
                <>
                  <h5>{send.done ? 'What changed' : 'What will change'}</h5>
                  <ul className="rt-diff">
                    {d.changes.map((c, i) => (
                      <li key={i} className={c[0] === '+' ? 'add' : c[0] === '-' ? 'del' : 'mod'}>{c}</li>
                    ))}
                  </ul>
                </>
              )}
              {d.notes.length > 0 && (
                <>
                  <h5>In the scene that is running now</h5>
                  <ul className="rt-notes">
                    {d.notes.map((c, i) => (
                      <li key={i}>⚠ {c}</li>
                    ))}
                  </ul>
                </>
              )}
            </>
          )}
          {!send.done && (
            <div className="rt-actions">
              <button className="primary" disabled={send.busy || !d || !!d.blockers.length || !!nothing} onClick={apply}>Send now</button>
              <button onClick={() => setSend(null)}>Cancel</button>
            </div>
          )}
          {send.done && (
            <div className="rt-actions">
              <button onClick={() => setSend(null)}>Close</button>
            </div>
          )}
        </Dialog>
      )}

      {report && (
        <Dialog title="Run report" onClose={() => setReport(null)}>
          {'error' in report ? (
            <p className="muted">{String(report.error)}</p>
          ) : (
            <>
              <p className="rt-sum">
                <b>{report.summary?.cues_fired ?? 0}</b> cues fired · worst timing error <b>{sec(report.summary?.worst_drift_ms ?? 0)}</b> · <b>{report.summary?.failed ?? 0}</b> failed sends ·{' '}
                <b>{report.summary?.no_device ?? 0}</b> with no device · <b>{(report.edits ?? []).length}</b> edits mid-show
              </p>
              {(report.problems ?? []).length > 0 && (
                <>
                  <h5>Problems</h5>
                  <ul className="rt-notes">
                    {(report.problems as any[]).map((p, i) => (
                      <li key={i}>⚠ {p.kind}{p.label ? ` → ${p.label}` : ''}: {p.error ?? p.status}</li>
                    ))}
                  </ul>
                </>
              )}
              <h5>Cue timing</h5>
              <table className="cs-table rt-table">
                <thead>
                  <tr><th>Q</th><th>Scene</th><th>Lane</th><th>Action</th><th>Planned</th><th>Actual</th><th>Off by</th></tr>
                </thead>
                <tbody>
                  {((report.cues ?? []) as any[]).map((c, i) => (
                    <tr key={i} className={Math.abs(c.drift_ms) > 250 ? 'late' : ''}>
                      <td>{c.num != null ? `Q${c.num}` : ''}</td>
                      <td>{c.scene}</td>
                      <td>{c.lane}</td>
                      <td>{c.kind}</td>
                      <td>{sec(c.planned_ms)}</td>
                      <td>{sec(c.actual_ms)}</td>
                      <td>{c.drift_ms >= 0 ? '+' : ''}{c.drift_ms} ms</td>
                    </tr>
                  ))}
                  {(report.cues ?? []).length === 0 && (
                    <tr><td colSpan={7} className="muted">No cues have fired yet.</td></tr>
                  )}
                </tbody>
              </table>
              {(report.edits ?? []).length > 0 && (
                <>
                  <h5>Edits made while running</h5>
                  <ul className="rt-diff">
                    {(report.edits as any[]).flatMap((e, i) => (e.changes as string[]).map((c, j) => <li key={`${i}-${j}`} className="mod">{c}</li>))}
                  </ul>
                </>
              )}
              <p className="muted">Timing is measured on the runtime from the moment a scene is entered; a late cue means the runtime was busy, not that the device was late.</p>
            </>
          )}
        </Dialog>
      )}
    </>
  )
}

/** One harmless message to a rig device, to check the link before doors. */
export async function testFire(id: string): Promise<{ status: string; error?: string }> {
  const r = await api(`/devices/${encodeURIComponent(id)}/test`, { method: 'POST' }).catch(() => null)
  if (!r) return { status: 'failed', error: 'The runtime did not answer.' }
  if (!r.ok) return { status: 'failed', error: String(r.body?.error ?? `refused (${r.status})`) }
  return { status: String(r.body?.status), error: r.body?.error }
}
