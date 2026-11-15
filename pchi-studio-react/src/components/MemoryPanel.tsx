import { useCallback, useEffect, useState } from 'react'
import { apiBase, feedToken, signedInSession } from './liveFeed'

type Note = {
  id: string
  kind: string
  text: string
  key?: string | null
  status: 'draft' | 'verified' | 'archived'
  source_type: string
  conflicts_with: string[]
  confidence?: number
}
type Hit = { id: string; kind: string; text: string; trust: 'verified' | 'draft'; score: number; conflicts_with: string[] }
type Answer = { answer: boolean; abstained: string | null; memories: Hit[]; closest: Hit[] }

const KEY = 'pchi.editkey'

async function call(path: string, init: RequestInit = {}) {
  const t = feedToken()
  const sess = signedInSession()
  let k = ''
  try {
    k = sessionStorage.getItem(KEY) ?? ''
  } catch {
    /* storage blocked */
  }
  const res = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...(init.headers ?? {}), ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(k ? { 'X-Pchi-Edit-Key': k } : {}), ...(sess ? { 'X-Pchi-Session': sess } : {}) },
  })
  let body: any = null
  try {
    body = await res.json()
  } catch {
    /* not JSON */
  }
  return { ok: res.ok, status: res.status, body }
}

/**
 * What the show remembers: ask it a question (it says when it does not know), and review what the run left behind. Drafts are what the log says happened
 * or what an agent noted; they count for little until a person verifies them. Two memories that disagree are shown together, and a person says which is right.
 * Everything here goes through the runtime, which decides who may do what: this panel only asks.
 */
export default function MemoryPanel() {
  const [notes, setNotes] = useState<Note[] | null>(null)
  const [off, setOff] = useState(false)
  const [error, setError] = useState('')
  const [info, setInfo] = useState('')
  const [q, setQ] = useState('')
  const [answer, setAnswer] = useState<Answer | null>(null)
  const [busy, setBusy] = useState(false)

  const load = useCallback(async () => {
    const r = await call('/memory/list').catch(() => null)
    if (!r) return setError('Could not reach the runtime.')
    if (r.status === 404) return setOff(true)
    if (!r.ok) return setError(String(r.body?.error ?? 'Could not read memory.'))
    setOff(false)
    setNotes((r.body?.memories ?? []) as Note[])
  }, [])
  useEffect(() => {
    void load()
  }, [load])

  const act = async (path: string, body: unknown, done: string) => {
    setBusy(true)
    setError('')
    setInfo('')
    const r = await call(path, { method: 'POST', body: body === undefined ? undefined : JSON.stringify(body) }).catch(() => null)
    setBusy(false)
    if (!r?.ok) return setError(String(r?.body?.error ?? 'The runtime did not take that.'))
    setInfo(done)
    await load()
  }

  const ask = async () => {
    if (!q.trim()) return
    setBusy(true)
    setError('')
    const r = await call(`/memory?q=${encodeURIComponent(q.trim())}`).catch(() => null)
    setBusy(false)
    if (!r?.ok) return setError(String(r?.body?.error ?? 'Could not ask.'))
    setAnswer(r.body as Answer)
    void load()
  }

  if (off) return <p className="rt-notes-plain">Memory is off. Start the runtime with <code>--memory memory.db</code> to turn it on (see docs/memory.md).</p>

  const drafts = (notes ?? []).filter((n) => n.status === 'draft')
  const verified = (notes ?? []).filter((n) => n.status === 'verified')
  const byId = new Map((notes ?? []).map((n) => [n.id, n]))
  const conflicted = (notes ?? []).filter((n) => n.status !== 'archived' && n.conflicts_with.length > 0)

  const badge = (n: { status?: string; trust?: string }) => {
    const s = n.trust ?? n.status
    return <span className={`mem-badge ${s}`}>{s === 'verified' ? 'verified' : 'draft: not yet verified'}</span>
  }

  return (
    <div className="mem">
      <div className="mem-ask">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void ask()} placeholder="Ask the show's memory: why was the haze cue late?" aria-label="Ask memory" />
        <button className="primary" disabled={busy || !q.trim()} onClick={() => void ask()}>Ask</button>
      </div>

      {answer && (
        <div className="mem-answer" role="status">
          {answer.answer ? (
            answer.memories.map((h) => (
              <div key={h.id} className={`mem-card ${h.trust}`}>
                <div>{badge(h)} <span className="muted">{h.kind}</span></div>
                <p>{h.text}</p>
              </div>
            ))
          ) : (
            <div className="mem-card idk">
              <p><b>I don't know.</b> {answer.abstained}.</p>
              {answer.closest.length > 0 && (
                <>
                  <p className="muted">What came closest (not an answer):</p>
                  <ul className="rt-notes">{answer.closest.map((h) => <li key={h.id}>{h.text}</li>)}</ul>
                </>
              )}
            </div>
          )}
        </div>
      )}

      {error && <p className="rt-err" role="alert">{error}</p>}
      {info && <p className="rt-ok" role="status">{info}</p>}

      <h4>Waiting for a person ({drafts.length})</h4>
      <p className="muted">Drafts are what the run's log says happened, or what an agent noted. They count for little until someone verifies them. Verify the ones worth keeping; archive the rest.</p>
      <div className="rt-actions">
        <button disabled={busy} onClick={() => void act('/memory/derive', undefined, 'Drafted from this run. Review them below.')} title="Make drafts from what this run's log says: failed sends, late cues, interruptions, decisions">Derive from this run</button>
      </div>
      {drafts.length === 0 && notes && <p className="muted">Nothing is waiting. Derive from the run to see what it left behind.</p>}
      <ul className="mem-list">
        {drafts.map((n) => (
          <li key={n.id}>
            <div><span className="muted">{n.kind} · {n.source_type}</span>{n.conflicts_with.length > 0 && <b className="mem-conflict"> in conflict</b>}</div>
            <p>{n.text}</p>
            <div className="rt-actions">
              <button disabled={busy} onClick={() => void act(`/memory/${n.id}/verify`, undefined, 'Verified.')}>Verify</button>
              <button disabled={busy} onClick={() => void act(`/memory/${n.id}/archive`, undefined, 'Archived: it no longer shows in answers.')}>Archive</button>
            </div>
          </li>
        ))}
      </ul>

      {conflicted.length > 0 && (
        <>
          <h4>Memories that disagree</h4>
          <p className="muted">Both are kept. Memory will not choose between two that are close: it says they disagree. Say which is right; the other is archived and points to it.</p>
          <ul className="mem-list">
            {conflicted.map((n) => (
              <li key={n.id}>
                <div className="muted">{n.kind} · {n.status}{n.key ? ` · ${n.key}` : ''}</div>
                <p>{n.text}</p>
                <div className="rt-actions">
                  {n.conflicts_with.filter((id) => byId.get(id)?.status !== 'archived').map((id) => (
                    <button key={id} disabled={busy} onClick={() => void act(`/memory/${n.id}/settle`, { over: id }, 'Settled: the other is archived.')} title={`Keep this one and archive: ${byId.get(id)?.text ?? id}`}>This one is right</button>
                  ))}
                </div>
              </li>
            ))}
          </ul>
        </>
      )}

      <h4>Verified ({verified.length})</h4>
      {verified.length === 0 ? <p className="muted">Nothing verified yet.</p> : <ul className="mem-list compact">{verified.map((n) => <li key={n.id}><span className="muted">{n.kind}</span> {n.text}</li>)}</ul>}
    </div>
  )
}
