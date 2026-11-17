import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { PchiShowFile } from '@pchi/sdk'
import { apiBase, feedToken, signedInKey, signedInSession } from './liveFeed'
import { signIn } from './RuntimeTools'

/** Where the show in the editor came from on the server, so Save can add the next version to it. */
export interface ServerRef {
  id: string
  version: number
  name: string
}

interface Entry {
  id: string
  name: string
  owner: string
  visibility: 'private' | 'team'
  status: 'draft' | 'planned' | 'ready' | 'done'
  planned_for: string
  venue: string
  notes: string
  version: number
  scenes: number
  updated: number
  updated_by: string
  mine: boolean
  can_edit: boolean
}

interface Listing {
  shows: Entry[]
  me: string
  can_save: boolean
  open: boolean
}

const STATUS_LABEL: Record<string, string> = { draft: 'Draft', planned: 'Planned', ready: 'Ready', done: 'Done' }

async function call(path: string, init: RequestInit = {}) {
  const t = feedToken()
  const k = signedInKey()
  const sess = signedInSession()
  const res = await fetch(`${apiBase()}${path}`, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...(init.headers ?? {}), ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(k ? { 'X-Pchi-Edit-Key': k } : {}), ...(sess ? { 'X-Pchi-Session': sess } : {}) },
  }).catch(() => null)
  if (!res) return { ok: false, status: 0, body: { error: 'The runtime did not answer.' } as Record<string, any> }
  const body = (await res.json().catch(() => null)) as Record<string, any> | null
  return { ok: res.ok, status: res.status, body: body ?? {} }
}

const when = (s: string) => {
  if (!s) return ''
  const d = new Date(s.length === 10 ? `${s}T00:00` : s)
  if (Number.isNaN(d.getTime())) return s
  return s.length === 10 ? d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' }) : d.toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' })
}

/** Planned shows that are still to come first (soonest at the top), then everything else, newest change first. */
function order(shows: Entry[]): Entry[] {
  const now = Date.now()
  const at = (e: Entry) => (e.planned_for ? new Date(e.planned_for.length === 10 ? `${e.planned_for}T00:00` : e.planned_for).getTime() : NaN)
  const upcoming = shows.filter((e) => e.status !== 'done' && at(e) >= now - 86_400_000).sort((a, b) => at(a) - at(b))
  const rest = shows.filter((e) => !upcoming.includes(e)).sort((a, b) => b.updated - a.updated)
  return [...upcoming, ...rest]
}

export default function LibraryDialog({
  start,
  current,
  getFile,
  dirty,
  onOpen,
  onSaved,
  onClose,
}: {
  start: 'open' | 'save'
  current: { name: string; ref: ServerRef | null }
  getFile: (name: string) => PchiShowFile
  dirty: boolean
  onOpen: (show: PchiShowFile, ref: ServerRef) => void
  onSaved: (ref: ServerRef) => void
  onClose: () => void
}) {
  const [tab, setTab] = useState<'open' | 'save'>(start)
  const [list, setList] = useState<Listing | null>(null)
  const [problem, setProblem] = useState('')
  const [needKey, setNeedKey] = useState(false)
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState('')
  const ref = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    setProblem('')
    const r = await call('/library')
    if (r.ok) {
      setList(r.body as unknown as Listing)
      setNeedKey(false)
    } else if (r.body.locked || r.status === 403 || r.status === 401) {
      setList(null)
      setNeedKey(true)
      setProblem(r.status === 401 ? 'The runtime wants its access token first (Live tab).' : '')
    } else {
      setList(null)
      setProblem(String(r.body.error ?? `The library answered ${r.status}.`))
    }
  }, [])

  useEffect(() => {
    ref.current?.focus()
    load()
  }, [load])
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])

  const shows = useMemo(() => order(list?.shows ?? []), [list])
  const existing = current.ref ? list?.shows.find((s) => s.id === current.ref!.id) : undefined

  // the details form (Save tab): prefilled from the show this one came from
  const [name, setName] = useState(current.name)
  const [planned, setPlanned] = useState('')
  const [venue, setVenue] = useState('')
  const [status, setStatus] = useState<Entry['status']>('draft')
  const [visibility, setVisibility] = useState<Entry['visibility']>('private')
  const [notes, setNotes] = useState('')
  const filled = useRef(false)
  useEffect(() => {
    if (filled.current || !existing) return
    filled.current = true
    setName(existing.name)
    setPlanned(existing.planned_for.length === 10 ? `${existing.planned_for}T00:00` : existing.planned_for)
    setVenue(existing.venue)
    setStatus(existing.status)
    setVisibility(existing.visibility)
    setNotes(existing.notes)
  }, [existing])

  const details = () => ({ name: name.trim(), planned_for: planned, venue, status, visibility, notes })

  const submitKey = async () => {
    setBusy(true)
    const who = await signIn(key)
    setBusy(false)
    if (!who) {
      setProblem('That key is not known, or it has expired.')
      return
    }
    setKey('')
    load()
  }

  const open = async (e: Entry) => {
    if (dirty && !window.confirm('You have unsaved changes in the editor. Open this show and drop them?')) return
    setBusy(true)
    const r = await call(`/library/${e.id}`)
    setBusy(false)
    if (!r.ok) return setProblem(String(r.body.error ?? 'Could not open it.'))
    onOpen(r.body.show as PchiShowFile, { id: e.id, version: r.body.meta.version, name: r.body.meta.name })
    onClose()
  }

  const copy = async (e: Entry) => {
    setBusy(true)
    const r = await call(`/library/${e.id}/copy`, { method: 'POST', body: '{}' })
    setBusy(false)
    if (!r.ok) return setProblem(String(r.body.error ?? 'Could not copy it.'))
    setDone(`Copied as “${r.body.meta.name}”: it is yours, and private.`)
    load()
  }

  const remove = async (e: Entry) => {
    if (!window.confirm(`Remove “${e.name}” from the library? The server keeps it in an archive folder; ask whoever runs it to put it back.`)) return
    const r = await call(`/library/${e.id}`, { method: 'DELETE' })
    if (!r.ok) return setProblem(String(r.body.error ?? 'Could not remove it.'))
    load()
  }

  const save = async (how: 'version' | 'new' | 'details') => {
    if (!details().name) return setProblem('Give the show a name.')
    setBusy(true)
    setProblem('')
    setDone('')
    let r
    if (how === 'new' || !current.ref) {
      r = await call('/library', { method: 'POST', body: JSON.stringify({ ...details(), show: getFile(details().name) }) })
    } else if (how === 'version') {
      r = await call(`/library/${current.ref.id}`, { method: 'PUT', body: JSON.stringify({ ...details(), show: getFile(details().name), base_version: current.ref.version }) })
    } else {
      r = await call(`/library/${current.ref.id}`, { method: 'PUT', body: JSON.stringify(details()) })
    }
    setBusy(false)
    if (!r.ok) return setProblem(String(r.body.error ?? `Not saved (${r.status}).`))
    const m = r.body.meta as Entry
    if (how !== 'details') onSaved({ id: m.id, version: m.version, name: m.name })
    const removed: string[] = r.body.removed_passwords ?? []
    setDone(
      (how === 'details' ? 'Details updated.' : `Saved as version ${m.version}.`) +
        (removed.length ? ` The password on ${removed.join(', ')} was not stored on the server: it stays in this browser, and the machine that runs the show needs it set there.` : ''),
    )
    load()
  }

  const canSave = list?.can_save !== false

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog wide" role="dialog" aria-modal="true" aria-label="Shows on the server" tabIndex={-1} ref={ref}>
        <header>
          <h3>Shows on the server</h3>
          <button onClick={onClose} aria-label="Close">×</button>
        </header>
        <div className="dialog-body">
          <nav className="tabs" role="tablist" style={{ margin: '10px 0' }}>
            <button role="tab" aria-selected={tab === 'open'} className={tab === 'open' ? 'on' : ''} onClick={() => setTab('open')}>Open and plan</button>
            <button role="tab" aria-selected={tab === 'save'} className={tab === 'save' ? 'on' : ''} onClick={() => setTab('save')}>Save this show</button>
          </nav>

          {list?.open && <p className="muted">This runtime has no roster, so everyone here is the same person and every show is everyone’s. Ask whoever runs it to add a roster for your own space.</p>}
          {problem && <p role="alert" style={{ color: '#f87171' }}>{problem}</p>}
          {done && <p role="status" style={{ color: '#4ade80' }}>{done}</p>}

          {needKey && (
            <form
              onSubmit={(e) => {
                e.preventDefault()
                submitKey()
              }}
              style={{ display: 'flex', gap: 8, alignItems: 'center', margin: '12px 0' }}
            >
              <span>Sign in to use the library:</span>
              <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="your key" aria-label="Your key" autoComplete="off" />
              <button className="primary" disabled={busy || !key.trim()}>Sign in</button>
            </form>
          )}

          {tab === 'open' && list && (
            <>
              {shows.length === 0 && <p className="muted">Nothing saved yet. Build a show, then use “Save this show”.</p>}
              <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'grid', gap: 8 }}>
                {shows.map((s) => (
                  <li key={s.id} style={{ border: '1px solid #2f323e', borderRadius: 10, padding: '10px 12px', display: 'grid', gap: 4 }}>
                    <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
                      <b>{s.name}</b>
                      <span className="pill">{STATUS_LABEL[s.status] ?? s.status}</span>
                      {s.visibility === 'team' && <span className="pill">team</span>}
                      {current.ref?.id === s.id && <span className="pill online">in the editor</span>}
                      <span className="spacer" style={{ flex: 1 }} />
                      <button className="primary" disabled={busy} onClick={() => open(s)}>Open</button>
                      {list.can_save && <button disabled={busy} onClick={() => copy(s)} title="A private copy of your own to build on">Copy</button>}
                      {s.can_edit && <button disabled={busy} onClick={() => remove(s)}>Remove</button>}
                    </div>
                    <div className="muted" style={{ fontSize: 12 }}>
                      {[s.planned_for && `planned ${when(s.planned_for)}`, s.venue, `${s.scenes} scenes`, `v${s.version}`, s.mine ? 'yours' : `by ${s.owner}`, `saved ${new Date(s.updated).toLocaleString()} by ${s.updated_by}`].filter(Boolean).join(' · ')}
                    </div>
                    {s.notes && <div style={{ fontSize: 13, whiteSpace: 'pre-wrap' }}>{s.notes}</div>}
                  </li>
                ))}
              </ul>
            </>
          )}

          {tab === 'save' && list && !canSave && <p className="muted">Your role may look at the library but not save to it.</p>}
          {tab === 'save' && list && canSave && (
            <form
              onSubmit={(e) => {
                e.preventDefault()
                save(current.ref ? 'version' : 'new')
              }}
              style={{ display: 'grid', gap: 10, marginTop: 8 }}
            >
              <label style={{ display: 'grid', gap: 4 }}>Name<input value={name} onChange={(e) => setName(e.target.value)} /></label>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(200px, 1fr))', gap: 10 }}>
                <label style={{ display: 'grid', gap: 4 }}>Planned for<input type="datetime-local" value={planned} onChange={(e) => setPlanned(e.target.value)} /></label>
                <label style={{ display: 'grid', gap: 4 }}>Venue<input value={venue} onChange={(e) => setVenue(e.target.value)} /></label>
                <label style={{ display: 'grid', gap: 4 }}>
                  Status
                  <select value={status} onChange={(e) => setStatus(e.target.value as Entry['status'])}>
                    {Object.entries(STATUS_LABEL).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
                  </select>
                </label>
                <label style={{ display: 'grid', gap: 4 }}>
                  Who can see it
                  <select value={visibility} onChange={(e) => setVisibility(e.target.value as Entry['visibility'])} disabled={!!existing && !existing.can_edit}>
                    <option value="private">Only me (and operators)</option>
                    <option value="team">Everyone who signs in</option>
                  </select>
                </label>
              </div>
              <label style={{ display: 'grid', gap: 4 }}>Notes<textarea rows={3} value={notes} onChange={(e) => setNotes(e.target.value)} placeholder="load-in time, who is calling, anything the next person should know" /></label>
              <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                {current.ref && existing && !existing.can_edit ? (
                  <>
                    <span className="muted">This show is {existing.owner}’s: you can save your own copy.</span>
                    <button type="button" className="primary" disabled={busy} onClick={() => save('new')}>Save as my own show</button>
                  </>
                ) : (
                  <>
                    <button className="primary" disabled={busy}>{current.ref ? `Save new version (v${current.ref.version} → v${current.ref.version + 1})` : 'Save to the server'}</button>
                    {current.ref && <button type="button" disabled={busy} onClick={() => save('details')} title="Change the date, status or notes without saving the show itself">Update details only</button>}
                    {current.ref && <button type="button" disabled={busy} onClick={() => save('new')}>Save as a new show</button>}
                  </>
                )}
              </div>
              <p className="muted" style={{ fontSize: 12, margin: 0 }}>Each save keeps the version before it, so nothing is overwritten. Device passwords are not kept on the server.</p>
            </form>
          )}
        </div>
      </div>
    </div>
  )
}
