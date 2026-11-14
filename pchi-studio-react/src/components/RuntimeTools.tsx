import { useEffect, useRef, useState } from 'react'
import { feedToken } from './liveFeed'

const KEY = 'pchi.editkey'
/** The operator key lives in this tab's session only: closing the tab forgets it. */
const editKey = () => {
  try {
    return sessionStorage.getItem(KEY) ?? ''
  } catch {
    return ''
  }
}
const setEditKey = (k: string) => {
  try {
    if (k) sessionStorage.setItem(KEY, k)
    else sessionStorage.removeItem(KEY)
  } catch {
    /* storage blocked: the key just has to be typed again */
  }
}

/** Calls the runtime's HTTP side (the dev server forwards /api). */
async function api(path: string, init: RequestInit = {}) {
  const t = feedToken()
  const k = editKey()
  const res = await fetch(`/api${path}`, { ...init, headers: { ...(init.headers ?? {}), ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(k ? { 'X-Pchi-Edit-Key': k } : {}) } })
  let body: unknown = null
  try {
    body = await res.json()
  } catch {
    /* not JSON */
  }
  return { ok: res.ok, status: res.status, body: body as Record<string, any> | null }
}

type Diff = { changes: string[]; notes: string[]; blockers: string[] }
type DoorCheck = { level: 'ok' | 'warn' | 'fail'; area: string; text: string; device?: string; scene?: string }

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
export default function RuntimeTools({ getFile, locked = false, undoDepth = 0 }: { getFile: () => unknown; locked?: boolean; undoDepth?: number }) {
  // mode: sending the edited show, or undoing the last send
  const [send, setSend] = useState<{ mode: 'send' | 'undo'; diff?: Diff; error?: string; done?: boolean; busy?: boolean; needKey?: boolean; keyWrong?: boolean } | null>(null)
  const [report, setReport] = useState<Record<string, any> | { error: string } | null>(null)
  const [keyText, setKeyText] = useState('')
  const [doors, setDoors] = useState<{ summary?: { go: boolean; fail: number; warn: number; ok: number }; checks?: DoorCheck[]; error?: string; busy?: boolean } | null>(null)
  type Rehearsal = { run: { length_ms: number; inputs: number; edited_live: number }; recorded_path: string[]; rehearsed_path: string[]; summary: { different: boolean; verdict: string; lines: string[] } }
  const [rehearse, setRehearse] = useState<{ r?: Rehearsal; error?: string; busy?: boolean } | null>(null)
  const openRehearse = async () => {
    setRehearse({ busy: true })
    const r = await api('/rehearse', { method: 'POST', body: JSON.stringify(getFile()), headers: { 'Content-Type': 'application/json' } }).catch(() => null)
    if (!r) return setRehearse({ error: 'The runtime did not answer. Is it running?' })
    if (!r.ok) return setRehearse({ error: String(r.body?.error ?? `Could not rehearse (${r.status})`) })
    setRehearse({ r: r.body as unknown as Rehearsal })
  }
  const openDoors = async () => {
    setDoors({ busy: true })
    const r = await api('/preflight').catch(() => null)
    if (!r?.ok) return setDoors({ error: 'Could not run the check. Is the runtime running?' })
    setDoors({ summary: r.body?.summary, checks: r.body?.checks })
  }

  const post = (mode: 'send' | 'undo', dry: boolean) =>
    api(mode === 'send' ? `/show${dry ? '?dry=1' : ''}` : `/show/undo${dry ? '?dry=1' : ''}`, {
      method: 'POST',
      ...(mode === 'send' ? { body: JSON.stringify(getFile()), headers: { 'Content-Type': 'application/json' } } : {}),
    }).catch(() => null)

  const open = async (mode: 'send' | 'undo') => {
    if (locked && !editKey()) return setSend({ mode, needKey: true })
    setSend({ mode, busy: true })
    const r = await post(mode, true)
    if (!r) return setSend({ mode, error: 'The runtime did not answer. Is it running?' })
    if (r.status === 403 && !r.body?.locked) return setSend({ mode, error: String(r.body?.error ?? 'You may not do that.') })
    if (r.status === 403) {
      setEditKey('')
      return setSend({ mode, needKey: true, keyWrong: true })
    }
    if (r.status === 409) return setSend({ mode, diff: r.body?.diff, error: r.body?.error })
    if (!r.ok) return setSend({ mode, error: String(r.body?.error ?? `The runtime refused it (${r.status}).`) })
    setSend({ mode, diff: r.body?.diff })
  }
  const apply = async () => {
    if (!send) return
    const mode = send.mode
    setSend((s) => s && { ...s, busy: true })
    const r = await post(mode, false)
    if (r?.status === 403 && !r.body?.locked) return setSend((x) => x && { ...x, busy: false, error: String(r.body?.error ?? 'You may not do that.') })
    if (r?.status === 403) {
      setEditKey('')
      return setSend({ mode, needKey: true, keyWrong: true })
    }
    if (!r?.ok) return setSend((s) => s && { ...s, busy: false, error: String(r?.body?.error ?? 'The runtime did not take it.') })
    setSend({ mode, diff: r.body?.diff, done: true })
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
        <button onClick={() => open('send')} title="Show what would change in the running show, then send it">{locked ? '🔒 ' : ''}Send show to runtime…</button>
        {undoDepth > 0 && <button onClick={() => open('undo')} title="Put the running show back as it was before the last send">Undo last send ({undoDepth})</button>}
        <button onClick={openDoors} title="Check the whole show against the rig and each tool before the doors open">Doors check</button>
        <button onClick={openRehearse} title="Play this edited show against what already happened in the run, without sending anything">Rehearse</button>
        <button onClick={openReport} title="How the run went: cue timing, failed sends, edits made mid-show">Run report</button>
      </div>

      {send && (
        <Dialog title={send.mode === 'send' ? 'Send show to the running runtime' : 'Undo the last send'} onClose={() => setSend(null)}>
          {send.needKey && (
            <form
              className="rt-key"
              onSubmit={(e) => {
                e.preventDefault()
                if (!keyText.trim()) return
                setEditKey(keyText.trim())
                setKeyText('')
                open(send.mode)
              }}
            >
              <p>{send.keyWrong ? <span className="rt-err" role="alert">That key was not accepted. </span> : null}This runtime only lets the operator change the show. Enter the operator key.</p>
              <input type="password" autoFocus value={keyText} onChange={(e) => setKeyText(e.target.value)} aria-label="Operator key" autoComplete="off" />
              <button type="submit" className="primary">Continue</button>
            </form>
          )}
          {send.mode === 'undo' && !send.done && !send.needKey && <p className="muted">This puts the <b>running</b> show back as it was before your last send. Your Studio file is not changed.</p>}
          {send.busy && <p className="muted">Working…</p>}
          {send.error && <p className="rt-err" role="alert">{send.error}</p>}
          {send.done && <p className="rt-ok" role="status">{send.mode === 'send' ? 'Sent. The running show now uses your edited version.' : 'Undone. The running show is back as it was.'} The scene, its clock and the context were kept.</p>}
          {d && (
            <>
              {nothing && <p className="muted">{send.mode === 'send' ? 'The runtime already has this show. Nothing to send.' : 'Nothing would change.'}</p>}
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
                  <h5>Check before {send.mode === 'send' ? 'sending' : 'undoing'}: the scene running now</h5>
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
              <button className="primary" disabled={send.busy || send.needKey || !d || !!d.blockers.length || !!nothing} onClick={apply}>{send.mode === 'send' ? 'Send now' : 'Undo now'}</button>
              <button onClick={() => setSend(null)}>Cancel</button>
            </div>
          )}
          {send.done && (
            <div className="rt-actions">
              {send.mode === 'send' && <button onClick={() => open('undo')}>Undo this send</button>}
              <button onClick={() => setSend(null)}>Close</button>
            </div>
          )}
        </Dialog>
      )}

      {rehearse && (
        <Dialog title="Rehearse against the run so far" onClose={() => setRehearse(null)}>
          {rehearse.busy && <p className="muted">Playing it through…</p>}
          {rehearse.error && <p className="rt-err" role="alert">{rehearse.error}</p>}
          {rehearse.r && (
            <>
              <p className={`doors-sum ${rehearse.r.summary.different ? 'nogo' : 'go'}`} role="status">
                {rehearse.r.summary.verdict}
                <span className="muted"> · {sec(rehearse.r.run.length_ms)} of run, {rehearse.r.run.inputs} recorded press{rehearse.r.run.inputs === 1 ? '' : 'es'} replayed</span>
              </p>
              {rehearse.r.summary.lines.length > 0 && (
                <ul className="doors-list warn">
                  {rehearse.r.summary.lines.map((l, i) => (
                    <li key={i}>{l}</li>
                  ))}
                </ul>
              )}
              <p className="muted">Real run: {rehearse.r.recorded_path.join(' → ')}</p>
              <p className="muted">This show: {rehearse.r.rehearsed_path.join(' → ')}</p>
              {rehearse.r.run.edited_live > 0 && <p className="muted">The real run was edited {rehearse.r.run.edited_live} time(s) while it ran, so its cue lists are what was running at the time.</p>}
              <p className="muted">Nothing was sent. Presses are replayed as they were: people might have pressed differently with a different show.</p>
            </>
          )}
          <div className="rt-actions">
            <button onClick={() => setRehearse(null)}>Close</button>
          </div>
        </Dialog>
      )}

      {doors && (
        <Dialog title="Doors check" onClose={() => setDoors(null)}>
          {doors.busy && <p className="muted">Checking…</p>}
          {doors.error && <p className="rt-err" role="alert">{doors.error}</p>}
          {doors.summary && (
            <p className={`doors-sum ${doors.summary.go ? 'go' : 'nogo'}`} role="status">
              {doors.summary.go ? (doors.summary.warn ? 'No failures: look at the warnings below.' : 'All clear.') : `${doors.summary.fail} thing${doors.summary.fail > 1 ? 's' : ''} will not work.`}
              <span className="muted"> {doors.summary.fail} failed · {doors.summary.warn} to look at · {doors.summary.ok} fine</span>
            </p>
          )}
          {(['fail', 'warn', 'ok'] as const).map((lvl) => {
            const rows = (doors.checks ?? []).filter((c) => c.level === lvl)
            if (!rows.length) return null
            return (
              <ul className={`doors-list ${lvl}`} key={lvl}>
                {rows.map((c, i) => (
                  <li key={i}>
                    <b>{lvl === 'fail' ? '✗' : lvl === 'warn' ? '⚠' : '✓'}</b> {c.text}
                  </li>
                ))}
              </ul>
            )
          })}
          <div className="rt-actions">
            <button onClick={openDoors}>Check again</button>
            <button onClick={() => setDoors(null)}>Close</button>
          </div>
          <p className="muted">This checks the show as the runtime has it. It never blocks the show. Failed lines are cues that cannot reach a device or that a tool cannot understand: fix those first.</p>
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
              {(report.decisions ?? []).length > 0 && (
                <>
                  <h5>Who did what</h5>
                  <ul className="rt-diff">
                    {(report.decisions as any[]).map((d, i) => (
                      <li key={i} className={d.gate === 'ALLOW' ? '' : 'mod'}>
                        {new Date(d.ts ?? 0).toLocaleTimeString()} · <b>{d.by}</b>: {d.gate === 'DENY' ? '(refused) ' : d.gate === 'ESCALATE' ? '(proposed) ' : ''}{d.text}
                      </li>
                    ))}
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

/** Send one cue to one device by hand (through its profile). Returns what was, or would be, sent. */
export async function tryCue(id: string, value: string): Promise<{ ok: boolean; status?: string; sentAs?: string; error?: string }> {
  const go = () => api(`/devices/${encodeURIComponent(id)}/try`, { method: 'POST', body: JSON.stringify({ value }), headers: { 'Content-Type': 'application/json' } }).catch(() => null)
  let r = await go()
  if (r?.status === 403 && r.body?.locked) {
    const k = window.prompt('This runtime needs your key to send a cue by hand:')
    if (!k) return { ok: false, error: 'Not sent: the operator key is needed.' }
    setEditKey(k.trim())
    r = await go()
  }
  if (!r) return { ok: false, error: 'The runtime did not answer.' }
  if (r.status === 403 && r.body?.locked) {
    setEditKey('')
    return { ok: false, error: 'That key was not accepted.' }
  }
  if (!r.ok) return { ok: false, error: String(r.body?.error ?? `refused (${r.status})`) }
  const sent = (r.body?.sent_as as { address: string; args: unknown[] }[] | undefined) ?? []
  return { ok: true, status: String(r.body?.status), sentAs: sent.map((m) => `${m.address}${m.args.length ? ' ' + m.args.map((a) => JSON.stringify(a)).join(' ') : ''}`).join(', '), error: r.body?.error as string | undefined }
}

/** Arm or disarm a laser. Arming needs the operator key when the runtime has one; it is asked for once per tab. */
export async function armLaser(id: string, armed: boolean): Promise<{ ok: boolean; error?: string }> {
  const go = () => api(`/devices/${encodeURIComponent(id)}/arm?armed=${armed ? 1 : 0}`, { method: 'POST' }).catch(() => null)
  let r = await go()
  if (r?.status === 403 && r.body?.locked) {
    const k = window.prompt('This runtime needs your key to arm a laser:')
    if (!k) return { ok: false, error: 'Not armed: the operator key is needed.' }
    setEditKey(k.trim())
    r = await go()
  }
  if (!r) return { ok: false, error: 'The runtime did not answer.' }
  if (r.status === 403 && r.body?.locked) {
    setEditKey('')
    return { ok: false, error: 'That key was not accepted.' }
  }
  return r.ok ? { ok: true } : { ok: false, error: String(r.body?.error ?? `refused (${r.status})`) }
}

/** The hard stop: disarm every laser and tell tools that have a safe message to go dark. No key needed. */
export async function stopLasers(): Promise<{ ok: boolean; notes: string[]; error?: string }> {
  const r = await api('/laser/stop', { method: 'POST' }).catch(() => null)
  if (!r?.ok) return { ok: false, notes: [], error: 'The runtime did not answer: use the laser software\'s own stop NOW.' }
  return { ok: true, notes: (r.body?.notes as string[]) ?? [] }
}

/** A person decides an agent's proposed show edit. Needs the operator key when the runtime has one. */
export async function decideProposal(id: number, verb: 'approve' | 'reject'): Promise<{ ok: boolean; error?: string }> {
  const go = () => api(`/proposals/${id}/${verb}`, { method: 'POST' }).catch(() => null)
  let r = await go()
  if (r?.status === 403 && r.body?.locked) {
    const k = window.prompt(`This runtime needs your key to ${verb} a proposal:`)
    if (!k) return { ok: false, error: 'The operator key is needed.' }
    setEditKey(k.trim())
    r = await go()
  }
  if (!r) return { ok: false, error: 'The runtime did not answer.' }
  if (r.status === 403 && r.body?.locked) {
    setEditKey('')
    return { ok: false, error: 'That key was not accepted.' }
  }
  return r.ok ? { ok: true } : { ok: false, error: String(r.body?.error ?? `refused (${r.status})`) }
}

/** One harmless message to a rig device, to check the link before doors. */
export async function testFire(id: string): Promise<{ status: string; error?: string }> {
  const r = await api(`/devices/${encodeURIComponent(id)}/test`, { method: 'POST' }).catch(() => null)
  if (!r) return { status: 'failed', error: 'The runtime did not answer.' }
  if (!r.ok) return { status: 'failed', error: String(r.body?.error ?? `refused (${r.status})`) }
  return { status: String(r.body?.status), error: r.body?.error }
}

export type WhoAmI = { name: string; role: string; signed_in: boolean; locked: boolean; can: string[] }

/** Who the runtime takes this tab to be, and what that person may do. */
export async function whoami(): Promise<WhoAmI | null> {
  const r = await api('/whoami').catch(() => null)
  return r?.ok ? (r.body as unknown as WhoAmI) : null
}

/** Sign in with your own key (given by whoever runs the show). Returns who you are, or null if the key is not known. */
export async function signIn(key: string): Promise<WhoAmI | null> {
  setEditKey(key.trim())
  const me = await whoami()
  if (!me?.signed_in) {
    setEditKey('')
    return null
  }
  return me
}

export const signOut = () => setEditKey('')
