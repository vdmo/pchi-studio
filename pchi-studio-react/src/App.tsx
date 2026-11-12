import { useCallback, useMemo, useRef, useState } from 'react'
import { newShow, newShowchart, type PchiEvent, type PchiShowFile } from '@pchi/sdk'
import ShowchartPanel from './components/ShowchartPanel'
import TimelinePanel from './components/TimelinePanel'
import RigPanel, { type Rig } from './components/RigPanel'
import LivePanel from './components/LivePanel'
import EventConsole, { type LogEntry } from './components/EventConsole'
import { useHistory } from './components/useHistory'

type Mode = 'show' | 'timeline' | 'rig' | 'live'
type Status = 'connecting' | 'online' | 'offline'

const TABS: { id: Mode; label: string; hint: string }[] = [
  { id: 'show', label: 'Show', hint: 'States and the transitions between them' },
  { id: 'timeline', label: 'Timeline', hint: 'Multi-lane cues inside each state' },
  { id: 'rig', label: 'Rig', hint: 'The systems that take part in the show' },
  { id: 'live', label: 'Live', hint: 'Operate a running show' },
]

/**
 * One show file holds everything: the showchart (what happens, and its
 * per-state timelines) and the rig (who can do it). The tabs are views of that
 * one model, and Show and Timeline share a single undo history.
 */
export default function App() {
  const initial = useMemo(() => newShow(), [])
  const [mode, setMode] = useState<Mode>('show')
  const [meta, setMeta] = useState({ project: initial.project, name: initial.name })
  const [showchart, setShowchart] = useState(initial.showchart ?? newShowchart())
  const [rig, setRig] = useState<Rig>({ nodes: initial.nodes, edges: initial.edges })
  const [log, setLog] = useState<LogEntry[]>([])
  const [liveStatus, setLiveStatus] = useState<Status>('connecting')
  const history = useHistory(showchart, setShowchart)
  const seq = useRef(1)

  const pushLog = useCallback((dir: LogEntry['dir'], event: PchiEvent) => {
    setLog((l) => [...l.slice(-199), { id: seq.current++, t: Date.now(), dir, event }])
  }, [])
  const note = useCallback((e: PchiEvent) => pushLog('sim', e), [pushLog])
  const onFeed = useCallback((e: PchiEvent) => pushLog('in', e), [pushLog])

  const save = () => {
    const file: PchiShowFile = {
      version: '1.0',
      project: meta.project,
      name: meta.name,
      nodes: rig.nodes,
      edges: rig.edges,
      timeline: [],
      assets: [],
      showchart,
    }
    const blob = new Blob([JSON.stringify(file, null, 2)], { type: 'application/json' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${meta.name}.pchi`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  const load = async (f: File) => {
    try {
      const s = JSON.parse(await f.text()) as PchiShowFile
      setMeta({ project: s.project ?? 'Untitled Project', name: s.name ?? 'untitled-show' })
      setRig({ nodes: s.nodes ?? [], edges: s.edges ?? [] })
      setShowchart(s.showchart ?? newShowchart())
      history.reset()
    } catch {
      alert('That file could not be read as a .pchi show.')
    }
  }

  return (
    <div className="app">
      <header className="topbar">
        <strong className="brand">PCHI Studio</strong>
        <nav className="tabs" role="tablist" aria-label="Views">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={mode === t.id} className={mode === t.id ? 'on' : ''} onClick={() => setMode(t.id)} title={t.hint}>
              {t.label}
            </button>
          ))}
        </nav>
        <input value={meta.name} onChange={(e) => setMeta({ ...meta, name: e.target.value })} aria-label="Show name" title="Show name" />
        <span className="spacer" />
        {mode === 'live' && (
          <span className={`pill ${liveStatus}`} role="status" title="Connection to the running show (pchi-studio-api)">
            runtime: {liveStatus}
          </span>
        )}
        <button onClick={save}>Save</button>
        <label className="btn">
          Open
          <input type="file" accept=".pchi,.json" hidden onChange={(e) => e.target.files?.[0] && load(e.target.files[0])} />
        </label>
      </header>

      {mode === 'show' && <ShowchartPanel chart={showchart} history={history} note={note} />}
      {mode === 'timeline' && <TimelinePanel chart={showchart} history={history} note={note} />}
      {mode === 'rig' && <RigPanel rig={rig} onChange={setRig} />}
      {mode === 'live' && <LivePanel onFeed={onFeed} onStatus={setLiveStatus} />}

      <footer className="slimfoot">
        <EventConsole log={log} onClear={() => setLog([])} collapsible />
      </footer>
    </div>
  )
}
