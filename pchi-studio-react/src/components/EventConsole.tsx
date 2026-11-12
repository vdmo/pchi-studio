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
}: {
  log: LogEntry[]
  onClear: () => void
}) {
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
    <section className="console">
      <div className="bar">
        <h4>
          Event stream <span className="muted">{filter ? `${rows.length} of ${log.length}` : log.length}</span>
        </h4>
        <input className="filter" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="filter: type or text" aria-label="Filter events" />
        <button onClick={() => setOpen(new Set(open.size ? [] : rows.map((r) => r.id)))}>{open.size ? 'collapse' : 'expand'}</button>
        <button onClick={onClear}>clear</button>
      </div>
      <div className="lines">
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
      </div>
    </section>
  )
}
