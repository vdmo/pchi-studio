import { useEffect, useMemo, useRef, useState } from 'react'
import { deptOf, type ShowCue, type Showchart } from '@pchi/sdk'
import Logo from './Logo'
import { fmt } from './TimelineStrip'
import { feedUrl, HEARTBEAT_MS, HEARTBEAT_SOURCE, useRuntimeWhich } from './liveFeed'

type RunDevice = { id: string; label: string; dept: string }
type Status = 'connecting' | 'online' | 'offline'

const STANDBY_MS = 10_000

/**
 * A page for one department's desk or tablet: open  /?dept=lighting  (or a device
 * id, /?dept=laser-1). It shows what that department has coming and one big
 * READY button. While READY, it checks in every few seconds, so the operator
 * sees "no link" if the tablet drops off the network instead of a stale READY.
 */
export default function DeptPage({ who }: { who: string }) {
  const which = useRuntimeWhich() // reconnect when Studio follows a takeover
  const [status, setStatus] = useState<Status>('connecting')
  const [def, setDef] = useState<Showchart | null>(null)
  const [devices, setDevices] = useState<RunDevice[]>([])
  const [scene, setScene] = useState('')
  const [enteredAt, setEnteredAt] = useState(Date.now())
  const [fired, setFired] = useState<Set<string>>(new Set())
  const [ready, setReady] = useState<boolean | null>(null) // null: not said anything yet
  const [now, setNow] = useState(Date.now())
  const ws = useRef<WebSocket>()
  const readyRef = useRef(ready)
  readyRef.current = ready

  const device = devices.find((d) => d.id === who)
  const dept = device?.dept ?? who
  const d = deptOf(dept)
  const name = device?.label ?? d.label

  const say = (r: boolean) => {
    if (ws.current?.readyState === WebSocket.OPEN) ws.current.send(JSON.stringify({ type: 'ready', dept: who, ready: r, note: r ? 'standing by' : 'on hold', source: HEARTBEAT_SOURCE }))
  }
  const sayRef = useRef(say)
  sayRef.current = say

  useEffect(() => {
    let closed = false
    let timer: number | undefined
    const connect = () => {
      setStatus('connecting')
      const s = new WebSocket(feedUrl())
      ws.current = s
      s.onopen = () => {
        setStatus('online')
        if (readyRef.current !== null) sayRef.current(readyRef.current) // say it again after a reconnect
      }
      s.onmessage = (m) => {
        let msg: Record<string, unknown>
        try {
          msg = JSON.parse(String(m.data))
        } catch {
          return
        }
        if (msg.type === 'hello') {
          const dd = msg.definition as Showchart
          setDef(dd)
          setDevices(Array.isArray(msg.devices) ? (msg.devices as RunDevice[]) : [])
          setScene(String(msg.state))
          setEnteredAt(Date.now() - Number(msg.elapsed_ms ?? 0))
          const el = Number(msg.elapsed_ms ?? 0)
          setFired(new Set((dd.states.find((x) => x.id === msg.state)?.timeline?.lanes ?? []).flatMap((l) => l.cues).filter((c) => c.at_ms <= el).map((c) => c.id)))
        } else if (msg.type === 'transition' && msg.applied) {
          setScene(String(msg.to))
          setEnteredAt(Date.now())
          setFired(new Set())
        } else if (msg.type === 'action' && typeof msg.cue === 'string') {
          setFired((f) => new Set(f).add(String(msg.cue)))
        }
      }
      s.onclose = () => {
        setStatus('offline')
        if (!closed) timer = window.setTimeout(connect, 2000)
      }
    }
    connect()
    return () => {
      closed = true
      window.clearTimeout(timer)
      ws.current?.close()
    }
  }, [which])

  // check in while READY or on HOLD (anything the operator should keep seeing)
  useEffect(() => {
    const id = window.setInterval(() => readyRef.current !== null && sayRef.current(readyRef.current), HEARTBEAT_MS)
    return () => window.clearInterval(id)
  }, [])
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 200)
    return () => window.clearInterval(id)
  }, [])

  const mine = useMemo(() => {
    const lanes = def?.states.find((s) => s.id === scene)?.timeline?.lanes ?? []
    return lanes
      .filter((l) => l.dept === dept && (!device || !l.device || l.device === device.id))
      .flatMap((l) => l.cues.map((c) => ({ ...c, lane: l.name || l.id })))
      .sort((a, b) => a.at_ms - b.at_ms) as (ShowCue & { lane: string })[]
  }, [def, scene, dept, device])

  const elapsed = Math.max(0, now - enteredAt)
  const upcoming = mine.filter((c) => !fired.has(c.id))
  const next = upcoming[0]
  const left = next ? next.at_ms - elapsed : undefined
  const call = !next ? 'NO CUES' : left! <= 0 ? 'GO' : left! <= STANDBY_MS ? 'STANDBY' : 'LATER'

  return (
    <div className="deptpage" style={{ ['--dept' as string]: d.color }}>
      <header>
        <Logo />
        <span className={`pill ${status}`} role="status">{status === 'online' ? 'linked' : status}</span>
      </header>
      <h1>{name}</h1>
      <p className="muted">{def ? `Scene ${scene || '—'}` : 'Waiting for the show…'}</p>

      <div className={`dp-call ${call.toLowerCase().replace(' ', '-')}`} aria-live="polite">
        <b>{call}</b>
        {next && <span>{`${next.num !== undefined ? `Q${next.num} · ` : ''}${next.label ?? next.action.kind}${left! > 0 ? ` in ${fmt(left!)}` : ''}`}</span>}
      </div>

      <button className={`dp-ready ${ready === null ? 'none' : ready ? 'on' : 'hold'}`} onClick={() => (setReady(!ready), say(!ready))} aria-pressed={ready === true}>
        {ready === null ? 'TAP WHEN READY' : ready ? 'READY' : 'ON HOLD'}
        <small>{ready === null ? 'the operator sees "no link" until you do' : ready ? 'tap to put on hold' : 'tap to say READY'}</small>
      </button>

      <h5>Coming up in this scene</h5>
      <ul className="dp-list">
        {upcoming.slice(0, 6).map((c) => (
          <li key={c.id}>
            <span>{c.num !== undefined ? `Q${c.num}` : ''}</span>
            <b>{c.label ?? c.action.kind}</b>
            <span className="muted">{fmt(c.at_ms)}</span>
          </li>
        ))}
        {upcoming.length === 0 && <li className="muted">Nothing more for {name} in this scene.</li>}
      </ul>
    </div>
  )
}
