import { useMemo, useState } from 'react'
import type { PchiEvent } from '@pchi/sdk'

export interface LogEntry {
  id: number
  t: number
  dir: 'in' | 'out' | 'sim'
  event: PchiEvent
}

const ARROW = { in: '←', out: '→', sim: '◆' } as const
const DIR_NAME = { in: 'received', out: 'sent', sim: 'simulated' } as const

export default function EventConsole({
  log,
  onClear,
  collapsible = false,
}: {
  log: LogEntry[]
  onClear: () => void
  /** Start as a slim bar that expands on click (used over the canvas). */
  collapsible?: boolean
}) {
  const [shown, setShown] = useState(!collapsible)
  const [open, setOpen] = useState<Set<number>>(new Set())
  const [filter, setFilter] = useState('')

  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase()
    const all = [...log].reverse()
    if (!q) return all
    // matches the event type or anything inside the payload
    return all.filter((l) => String(l.event.type).toLowerCase().includes(q) || JSON.stringify(l.event).toLowerCase().includes(q))
  }, [log, filter])

  const toggle = (id: number) =>
    setOpen((s) => {
      const n = new Set(s)
      if (!n.delete(id)) n.add(id)
      return n
    })

  return (
    <section className={`console ${collapsible ? "slim" : ""} ${collapsible && !shown ? 'folded' : ''}`}>
      <div className="bar">
        {collapsible && (
          <button className="icon" onClick={() => setShown((s) => !s)} aria-expanded={shown} aria-label={shown ? "Collapse event stream" : "Expand event stream"}>
            {shown ? "▾" : "▸"}
          </button>
        )}
        <h4 onClick={collapsible ? () => setShown((x) => !x) : undefined} style={collapsible ? { cursor: 'pointer' } : undefined}>
          {collapsible && !shown ? 'Events' : 'Event stream'} <span className="muted">{filter ? `${rows.length} of ${log.length}` : log.length}</span>
        </h4>
        {shown && <input className="filter" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="filter: type or text" aria-label="Filter events" />}
        {shown && <button onClick={() => setOpen(new Set(open.size ? [] : rows.map((r) => r.id)))}>{open.size ? 'collapse' : 'expand'}</button>}
        {shown && <button onClick={onClear}>clear</button>}
      </div>
      {shown && <div className="lines">
        {rows.length === 0 && <p className="muted">{log.length ? 'No events match the filter.' : 'No events yet.'}</p>}
        {rows.map((l) => {
          const isOpen = open.has(l.id)
          return (
            <div key={l.id} className={`line ${l.dir} ${isOpen ? 'open' : ''}`}>
              <button className="row" aria-expanded={isOpen} onClick={() => toggle(l.id)} title={`${DIR_NAME[l.dir]} — click to ${isOpen ? 'collapse' : 'expand'}`}>
                <span>{new Date(l.t).toLocaleTimeString()}</span>
                <b aria-label={DIR_NAME[l.dir]}>{ARROW[l.dir]}</b>
                <code>{String(l.event.type)}</code>
                <small>{isOpen ? '▾' : JSON.stringify(l.event).slice(0, 160)}</small>
              </button>
              {isOpen && <pre className="json">{JSON.stringify(l.event, null, 2)}</pre>}
            </div>
          )
        })}
      </div>}
    </section>
  )
}
