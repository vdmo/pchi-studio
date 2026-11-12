import type { PchiEvent } from '@pchi/sdk'

export interface LogEntry {
  id: number
  t: number
  dir: 'in' | 'out' | 'sim'
  event: PchiEvent
}

const ARROW = { in: '←', out: '→', sim: '◆' } as const

export default function EventConsole({
  log,
  onClear,
}: {
  log: LogEntry[]
  onClear: () => void
}) {
  return (
    <section className="console">
      <div className="bar">
        <h4>Event stream</h4>
        <button onClick={onClear}>clear</button>
      </div>
      <div className="lines">
        {log.length === 0 && <p className="muted">No events yet.</p>}
        {[...log].reverse().map((l) => (
          <div key={l.id} className={`line ${l.dir}`}>
            <span>{new Date(l.t).toLocaleTimeString()}</span>
            <b>{ARROW[l.dir]}</b>
            <code>{String(l.event.type)}</code>
            <small>{JSON.stringify(l.event).slice(0, 160)}</small>
          </div>
        ))}
      </div>
    </section>
  )
}
