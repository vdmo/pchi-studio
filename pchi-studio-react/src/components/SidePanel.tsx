import { useEffect, useMemo, useState } from 'react'
import { validateShow, type PchiNode, type PchiShowFile, type Showchart, type Validation } from '@pchi/sdk'
import { AI_PRIVACY, ProposalCard, type Assistant } from './assistant'
import { useAiSettings } from '../lib/aiSettings'

export type SideTab = 'checks' | 'code' | 'ai'

interface Props {
  tab: SideTab
  onTab: (t: SideTab) => void
  onClose: () => void
  chart: Showchart
  nodes: PchiNode[]
  ai: Assistant
  scene: string | null
  getFile: () => PchiShowFile
  onGoto: (v: Validation) => void
  onSettings: () => void
}

const TABS: { id: SideTab; label: string }[] = [
  { id: 'checks', label: 'Checks' },
  { id: 'code', label: 'Code' },
  { id: 'ai', label: 'AI' },
]

/**
 * The drawer on the right: what is wrong with the show right now (Checks), the show as text (Code, read only),
 * and an AI helper that can only PROPOSE a change to the draft (AI). None of them touches the runtime.
 */
export default function SidePanel(p: Props) {
  return (
    <aside className="side" aria-label="Side panel">
      <header>
        <div className="side-tabs" role="tablist">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={p.tab === t.id} className={p.tab === t.id ? 'on' : ''} onClick={() => p.onTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
        <button className="icon-btn" onClick={p.onClose} aria-label="Close the side panel" title="Close">×</button>
      </header>
      <div className="side-body">
        {p.tab === 'checks' && <Checks chart={p.chart} nodes={p.nodes} onGoto={p.onGoto} />}
        {p.tab === 'code' && <Code chart={p.chart} scene={p.scene} getFile={p.getFile} />}
        {p.tab === 'ai' && <AiTab ai={p.ai} onSettings={p.onSettings} />}
      </div>
    </aside>
  )
}

// ───────── Checks ─────────

function Checks({ chart, nodes, onGoto }: { chart: Showchart; nodes: PchiNode[]; onGoto: (v: Validation) => void }) {
  const v = useMemo(() => validateShow(chart, nodes), [chart, nodes])
  if (v.length === 0) return <p className="side-ok">✓ Nothing to check: the show and the rig agree.</p>
  const errors = v.filter((x) => x.level === 'error')
  const warns = v.filter((x) => x.level === 'warn')
  const group = (title: string, list: Validation[]) =>
    list.length > 0 && (
      <section>
        <h4>{title} ({list.length})</h4>
        <ul className="side-list">
          {list.map((x, i) => (
            <li key={i}>
              <button className={`vrow ${x.level}`} onClick={() => onGoto(x)} title={x.scene || x.device ? 'Go there' : undefined}>
                <span aria-hidden>{x.level === 'error' ? '⛔' : '⚠'}</span>
                <span>{x.text}</span>
              </button>
            </li>
          ))}
        </ul>
      </section>
    )
  return (
    <>
      <p className="side-note">Checked as you edit, without the runtime. Nothing here blocks anything; the runtime's own doors check (Live tab) goes further.</p>
      {group('Will not work', errors)}
      {group('Look at these', warns)}
    </>
  )
}

// ───────── Code ─────────

/** The show as text, with device passwords hidden (this is a screen, and a screen gets shared). */
function shown(value: unknown): string {
  return JSON.stringify(value, (k, v) => (/pass(word)?$|secret|token/i.test(k) && typeof v === 'string' && v ? '••••••' : v), 2)
}

function Code({ chart, scene, getFile }: { chart: Showchart; scene: string | null; getFile: () => PchiShowFile }) {
  const [what, setWhat] = useState<'scene' | 'show'>(scene ? 'scene' : 'show')
  const [said, setSaid] = useState('')
  useEffect(() => setWhat(scene ? 'scene' : 'show'), [scene])
  const text = useMemo(() => {
    if (what === 'scene' && scene) return shown(chart.states.find((s) => s.id === scene) ?? {})
    return shown(getFile())
  }, [what, scene, chart, getFile])
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setSaid('Copied (passwords are hidden in this view).')
    } catch {
      setSaid('The browser would not let me copy: select the text instead.')
    }
  }
  return (
    <>
      <div className="side-bar">
        <select value={what} onChange={(e) => setWhat(e.target.value as 'scene' | 'show')} aria-label="What to show">
          {scene && <option value="scene">Scene {scene}</option>}
          <option value="show">Whole show</option>
        </select>
        <button onClick={copy}>Copy</button>
      </div>
      <p className="side-note">Read only: edit in the other tabs. This is what Save writes, with passwords hidden here.</p>
      {said && <p className="side-note" role="status">{said}</p>}
      <pre className="side-code" tabIndex={0}>{text}</pre>
    </>
  )
}

// ───────── AI ─────────

function AiTab({ ai, onSettings }: { ai: Assistant; onSettings: () => void }) {
  const { key, model } = useAiSettings()
  const ready = key.trim() !== ''
  return (
    <>
      <p className="side-note">Describe a change and get a <b>proposal</b>. {AI_PRIVACY}</p>
      <p className="side-note">
        {ready ? <>Using <b>{model}</b> with the key set in Settings. </> : <b>No key yet. </b>}
        <button onClick={onSettings}>{ready ? 'Settings' : 'Add your key in Settings'}</button>
      </p>
      <label className="side-field">
        What should change?
        <textarea
          rows={4}
          value={ai.text}
          onChange={(e) => ai.setText(e.target.value)}
          onKeyDown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && ai.send()}
          placeholder="Add a PARK cue to the arm 2 seconds before the end of the finale…"
        />
      </label>
      <div className="side-bar">
        <button className="primary" onClick={ai.send} disabled={ai.busy || !ready || !ai.text.trim()}>
          {ai.busy ? 'Thinking…' : 'Send'}
        </button>
        {ai.busy && <button onClick={ai.cancel}>Cancel</button>}
      </div>
      {ai.error && <p className="side-err" role="alert">{ai.error}</p>}
      <ProposalCard ai={ai} />
    </>
  )
}
