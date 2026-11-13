import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { clearDevice, ensureCueNumbers, rigCheck, newShow, newShowchart, type PchiEvent, type PchiShowFile, type Showchart } from '@pchi/sdk'
import ShowchartPanel from './components/ShowchartPanel'
import TimelinePanel from './components/TimelinePanel'
import CueSheet from './components/CueSheet'
import RigPanel, { type Rig } from './components/RigPanel'
import LivePanel from './components/LivePanel'
import EventConsole, { type LogEntry } from './components/EventConsole'
import { useHistory } from './components/useHistory'
import Logo from './components/Logo'
import ShortcutsDialog from './components/ShortcutsDialog'

const AUTOSAVE = 'pchi.autosave'
interface Autosave {
  meta: { project: string; name: string }
  rig: Rig
  showchart: Showchart
  at: number
}
const readAutosave = (): Autosave | null => {
  try {
    const v = JSON.parse(localStorage.getItem(AUTOSAVE) || 'null')
    return v && v.showchart && v.meta ? (v as Autosave) : null
  } catch {
    return null
  }
}
const writeAutosave = (v: Autosave | null) => {
  try {
    if (v) localStorage.setItem(AUTOSAVE, JSON.stringify(v))
    else localStorage.removeItem(AUTOSAVE)
  } catch {
    /* storage full or blocked: autosave is a convenience, the Save button still works */
  }
}

type Mode = 'show' | 'timeline' | 'cues' | 'rig' | 'live'
type Status = 'connecting' | 'online' | 'offline'

const TABS: { id: Mode; label: string; hint: string }[] = [
  { id: 'show', label: 'Show', hint: 'States and the transitions between them' },
  { id: 'timeline', label: 'Timeline', hint: 'Multi-lane cues inside each state' },
  { id: 'cues', label: 'Cue sheet', hint: 'Every cue in calling order, for stage managers' },
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
  // the scene (state) being looked at: shared by Show and Timeline so they always show the same one
  const [scene, setScene] = useState<string | null>(null)
  const [meta, setMeta] = useState({ project: initial.project, name: initial.name })
  const [showchart, setShowchart] = useState(() => ensureCueNumbers(initial.showchart ?? newShowchart()))
  const [rig, setRig] = useState<Rig>({ nodes: initial.nodes, edges: initial.edges })
  const [log, setLog] = useState<LogEntry[]>([])
  const [liveStatus, setLiveStatus] = useState<Status>('connecting')
  // every change passes through here, so a cue always has its number the moment it exists
  const setChart = useCallback((c: Showchart) => setShowchart(ensureCueNumbers(c)), [])
  const history = useHistory(showchart, setChart)
  const rigIssues = useMemo(() => rigCheck(showchart, rig.nodes), [showchart, rig.nodes])
  const [help, setHelp] = useState(false)
  const [pending, setPending] = useState<Autosave | null>(readAutosave) // unsaved work from an earlier visit

  // what is on disk (or the starting point): anything different is unsaved
  const snapshot = useMemo(() => JSON.stringify([meta, rig, showchart]), [meta, rig, showchart])
  const [baseline, setBaseline] = useState(snapshot)
  const dirty = snapshot !== baseline

  useEffect(() => {
    if (pending) return // do not overwrite the earlier work before the user has chosen
    if (!dirty) return
    const id = window.setTimeout(() => writeAutosave({ meta, rig, showchart, at: Date.now() }), 800)
    return () => window.clearTimeout(id)
  }, [dirty, meta, rig, showchart, pending])

  useEffect(() => {
    if (!dirty) return
    const h = (e: BeforeUnloadEvent) => e.preventDefault()
    window.addEventListener('beforeunload', h)
    return () => window.removeEventListener('beforeunload', h)
  }, [dirty])

  // "?" opens the shortcut list (unless the user is typing)
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null
      if (e.key !== '?' || e.ctrlKey || e.metaKey || (el && (/^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName) || el.isContentEditable))) return
      e.preventDefault()
      setHelp((v) => !v)
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [])
  const seq = useRef(1)

  const pushLog = useCallback((dir: LogEntry['dir'], event: PchiEvent) => {
    setLog((l) => [...l.slice(-199), { id: seq.current++, t: Date.now(), dir, event }])
  }, [])
  const note = useCallback((e: PchiEvent) => pushLog('sim', e), [pushLog])
  const onFeed = useCallback((e: PchiEvent) => pushLog('in', e), [pushLog])

  const buildFile = (): PchiShowFile => ({
    version: '1.0',
    project: meta.project,
    name: meta.name,
    nodes: rig.nodes,
    edges: rig.edges,
    timeline: [],
    assets: [],
    showchart,
  })
  const save = () => {
    const file = buildFile()
    const blob = new Blob([JSON.stringify(file, null, 2)], { type: 'application/json' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${meta.name}.pchi`
    a.click()
    URL.revokeObjectURL(a.href)
    setBaseline(snapshot)
    writeAutosave(null)
  }

  const load = async (f: File) => {
    try {
      const s = JSON.parse(await f.text()) as PchiShowFile
      setMeta({ project: s.project ?? 'Untitled Project', name: s.name ?? 'untitled-show' })
      setRig({ nodes: s.nodes ?? [], edges: s.edges ?? [] })
      const chart = ensureCueNumbers(s.showchart ?? newShowchart())
      setShowchart(chart)
      history.reset()
      setScene(null)
      setBaseline(JSON.stringify([{ project: s.project ?? 'Untitled Project', name: s.name ?? 'untitled-show' }, { nodes: s.nodes ?? [], edges: s.edges ?? [] }, chart]))
      writeAutosave(null)
    } catch {
      alert('That file could not be read as a .pchi show.')
    }
  }

  return (
    <div className="app">
      <header className="topbar">
        <Logo />
        <nav className="tabs" role="tablist" aria-label="Views">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={mode === t.id} className={mode === t.id ? 'on' : ''} onClick={() => setMode(t.id)} title={t.hint}>
              {t.label}
            </button>
          ))}
        </nav>
        <input value={meta.name} onChange={(e) => setMeta({ ...meta, name: e.target.value })} aria-label="Show name" title="Show name" />
        {dirty && <span className="dirty" title="Changes since the last Save or Open. They are also kept in this browser, so a refresh will not lose them.">● unsaved</span>}
        <span className="spacer" />
        {mode === 'live' && (
          <span className={`pill ${liveStatus}`} role="status" title="Connection to the running show (pchi-studio-api)">
            runtime: {liveStatus}
          </span>
        )}
        <button className="icon-btn" onClick={() => setHelp(true)} aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)">?</button>
        <button onClick={save}>Save</button>
        <label className="btn">
          Open
          <input type="file" accept=".pchi,.json" hidden onChange={(e) => e.target.files?.[0] && load(e.target.files[0])} />
        </label>
      </header>

      {pending && (
        <div className="restore" role="alertdialog" aria-label="Restore unsaved work">
          <span>
            Unsaved work from {new Date(pending.at).toLocaleString()}: <b>{pending.meta.name}</b>, {pending.showchart.states.length} scenes.
          </span>
          <button
            className="primary"
            onClick={() => {
              setMeta(pending.meta)
              setRig(pending.rig)
              setShowchart(ensureCueNumbers(pending.showchart))
              history.reset()
              setScene(null)
              setPending(null)
            }}
          >
            Restore
          </button>
          <button
            onClick={() => {
              writeAutosave(null)
              setPending(null)
            }}
          >
            Discard
          </button>
        </div>
      )}
      {help && <ShortcutsDialog onClose={() => setHelp(false)} />}

      {mode === 'show' && <ShowchartPanel chart={showchart} history={history} note={note} scene={scene} onScene={setScene} onOpenTimeline={() => setMode('timeline')} rig={rig} onRig={setRig} />}
      {mode === 'timeline' && <TimelinePanel chart={showchart} history={history} note={note} scene={scene} onScene={setScene} nodes={rig.nodes} rig={rig} onRig={setRig} />}
      {mode === 'cues' && <CueSheet chart={showchart} history={history} scene={scene} onScene={setScene} onOpenTimeline={() => setMode('timeline')} nodes={rig.nodes} />}
      {mode === 'rig' && <RigPanel rig={rig} onChange={setRig} issues={rigIssues} onRemoved={(id) => {
        const c = clearDevice(showchart, id)
        if (c !== showchart) history.commit(c)
      }} />}
      {mode === 'live' && <LivePanel onFeed={onFeed} onStatus={setLiveStatus} rig={rig.nodes} getFile={buildFile} />}

      <footer className="slimfoot">
        <EventConsole log={log} onClear={() => setLog([])} collapsible />
      </footer>
    </div>
  )
}
