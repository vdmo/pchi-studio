import { useEffect, useMemo, useState } from 'react'
import { apiBase, feedToken, signedInSession } from './liveFeed'

type Action = { kind: string; basis: string; source: string }
type Profile = { id: string; name: string; dept: string; transport: string; built_in: boolean; hint?: string; notes?: string; kinds: string[]; actions?: Action[] }

const TRANSPORT: Record<string, string> = {
  osc: 'OSC (UDP)',
  'tcp-text': 'Text over TCP',
  http: 'Web request (http)',
  'jsonrpc-tcp': 'JSON-RPC over TCP',
  mqtt: 'MQTT publish',
  serial: 'Serial line',
  udp: 'UDP text',
}

/**
 * What the runtime can drive, from its own list (GET /profiles): every tool profile, by how it is reached, with the cue kinds it takes
 * and what each command rests on. Nothing here is proven on real gear: "memory" and "community" mean the vendor's own document was not read.
 */
export default function ProfilesPanel() {
  const [list, setList] = useState<Profile[] | null>(null)
  const [error, setError] = useState('')
  const [q, setQ] = useState('')

  useEffect(() => {
    const t = feedToken()
    const s = signedInSession()
    fetch(`${apiBase()}/profiles`, { headers: { ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(s ? { 'X-Pchi-Session': s } : {}) } })
      .then(async (r) => {
        if (!r.ok) throw new Error(`the runtime refused it (${r.status})`)
        const body = await r.json()
        if (!Array.isArray(body)) throw new Error('the runtime sent something else')
        setList(body as Profile[])
      })
      .catch((e) => setError(e instanceof Error ? e.message : 'the runtime did not answer'))
  }, [])

  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase()
    const shown = (list ?? []).filter((p) => !needle || [p.id, p.name, p.dept, p.transport, ...p.kinds].some((x) => x.toLowerCase().includes(needle)))
    const m = new Map<string, Profile[]>()
    for (const p of shown) m.set(p.transport, [...(m.get(p.transport) ?? []), p])
    return [...m.entries()]
  }, [list, q])

  if (error) return <p role="alert">Could not read the profiles: {error}. Is the runtime running?</p>
  if (!list) return <p className="muted">Loading…</p>
  return (
    <div className="profiles-panel">
      <p className="muted">
        {list.length} profile{list.length === 1 ? '' : 's'} loaded. Add your own with <code>--profiles DIR</code>. A command that rests on memory or community notes has not been checked against the maker's document; none of these has run on real gear unless a profile says so.
      </p>
      <input type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Find a tool, a kind or a department" aria-label="Find a profile" />
      {groups.length === 0 && <p className="muted">Nothing matches.</p>}
      {groups.map(([transport, ps]) => (
        <section key={transport} aria-label={TRANSPORT[transport] ?? transport}>
          <h4>{TRANSPORT[transport] ?? transport}</h4>
          {ps.map((p) => (
            <details key={p.id}>
              <summary>
                <b>{p.name}</b> <span className="muted">· {p.id} · {p.dept}{p.built_in ? ' · built in' : ''}</span>
              </summary>
              {p.hint && <p>{p.hint}</p>}
              {p.notes && <p className="muted">{p.notes}</p>}
              {p.actions && p.actions.length > 0 ? (
                <ul>
                  {p.actions.map((a) => (
                    <li key={a.kind}>
                      <code>{a.kind}</code> · rests on <span style={{ fontWeight: 600 }}>{a.basis}</span>
                      {a.source ? <span className="muted"> · {a.source}</span> : null}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="muted">Takes: {p.kinds.length ? p.kinds.join(', ') : 'any cue (the generic message)'}</p>
              )}
            </details>
          ))}
        </section>
      ))}
    </div>
  )
}
