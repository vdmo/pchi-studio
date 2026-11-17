import { useEffect, useMemo, useRef, useState } from 'react'
import { ASSIST_SYSTEM, assistPrompt, canApply, readProposal, validateShow, type PchiNode, type PchiShowFile, type Proposal, type Showchart, type Validation } from '@pchi/sdk'
import type { History } from './useHistory'

export type SideTab = 'checks' | 'code' | 'ai'

interface Props {
  tab: SideTab
  onTab: (t: SideTab) => void
  onClose: () => void
  chart: Showchart
  nodes: PchiNode[]
  history: History
  scene: string | null
  getFile: () => PchiShowFile
  onGoto: (v: Validation) => void
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
        {p.tab === 'ai' && <Assistant chart={p.chart} nodes={p.nodes} history={p.history} />}
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

const KEY = 'pchi.ai.key'
const MODEL = 'pchi.ai.model'
const read = (k: string) => {
  try {
    return sessionStorage.getItem(k) ?? ''
  } catch {
    return ''
  }
}
const write = (k: string, v: string) => {
  try {
    if (v) sessionStorage.setItem(k, v)
    else sessionStorage.removeItem(k)
  } catch {
    /* blocked: the key is then kept only while this panel is open */
  }
}

function Assistant({ chart, nodes, history }: { chart: Showchart; nodes: PchiNode[]; history: History }) {
  const [key, setKey] = useState(() => read(KEY))
  const [model, setModel] = useState(() => read(MODEL) || 'claude-sonnet-5-5')
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [proposal, setProposal] = useState<Proposal | null>(null)
  const [applied, setApplied] = useState(false)
  const abort = useRef<AbortController | null>(null)
  useEffect(() => () => abort.current?.abort(), [])

  const send = async () => {
    if (!key.trim() || !text.trim() || busy) return
    write(KEY, key.trim())
    write(MODEL, model.trim())
    setBusy(true)
    setError('')
    setProposal(null)
    setApplied(false)
    abort.current?.abort()
    const ac = new AbortController()
    abort.current = ac
    try {
      const r = await fetch('https://api.anthropic.com/v1/messages', {
        method: 'POST',
        signal: ac.signal,
        headers: {
          'content-type': 'application/json',
          'x-api-key': key.trim(),
          'anthropic-version': '2023-06-01',
          'anthropic-dangerous-direct-browser-access': 'true',
        },
        body: JSON.stringify({ model: model.trim(), max_tokens: 16000, system: ASSIST_SYSTEM, messages: [{ role: 'user', content: assistPrompt(chart, nodes, text) }] }),
      })
      const body = await r.json().catch(() => null)
      if (!r.ok) {
        setError(r.status === 401 ? 'The key was not accepted.' : `The AI service said no (${r.status}): ${body?.error?.message ?? 'no reason given'}`)
        return
      }
      if (body?.stop_reason === 'max_tokens') {
        setError('The reply was cut off. Ask for a smaller change, or one scene at a time.')
        return
      }
      const reply = (body?.content ?? []).filter((b: { type: string }) => b.type === 'text').map((b: { text: string }) => b.text).join('')
      const res = readProposal(reply, chart, nodes)
      if (!res.ok) setError(res.error)
      else setProposal(res.proposal)
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setError('Could not reach the AI service. Check the connection.')
    } finally {
      setBusy(false)
    }
  }

  const apply = () => {
    if (!proposal || !canApply(proposal)) return
    history.commit(proposal.chart)
    setApplied(true)
  }
  // Undo is offered only while the draft is still exactly what Apply made it: after any other edit it would undo that edit instead
  const untouched = applied && proposal !== null && chart === proposal.chart
  const undo = () => {
    if (untouched && history.undo()) setApplied(false)
  }

  return (
    <>
      <p className="side-note">
        Describe a change and get a <b>proposal</b>. Nothing is applied until you press Apply, and Apply edits only this draft, which you can undo. The AI cannot change the rig, arm a device or send anything to the runtime.
        When you press Send, the scenes and cues, and each device's name, type and allowed program names, go to Anthropic. <b>Addresses, ports and passwords are never sent.</b>
      </p>
      <label className="side-field">
        API key (kept in this tab only)
        <input type="password" value={key} onChange={(e) => setKey(e.target.value)} autoComplete="off" spellCheck={false} placeholder="sk-ant-…" />
      </label>
      <label className="side-field">
        Model
        <input value={model} onChange={(e) => setModel(e.target.value)} spellCheck={false} />
      </label>
      <label className="side-field">
        What should change?
        <textarea
          rows={4}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => (e.ctrlKey || e.metaKey) && e.key === 'Enter' && send()}
          placeholder="Add a PARK cue to the arm 2 seconds before the end of the finale…"
        />
      </label>
      <div className="side-bar">
        <button className="primary" onClick={send} disabled={busy || !key.trim() || !text.trim()}>
          {busy ? 'Thinking…' : 'Send'}
        </button>
        {busy && <button onClick={() => abort.current?.abort()}>Cancel</button>}
      </div>
      {error && <p className="side-err" role="alert">{error}</p>}
      {proposal && (
        <section className="proposal" aria-label="Proposal">
          <h4>Proposal</h4>
          <p>{proposal.summary}</p>
          {proposal.changes.length === 0 ? <p className="side-note">It changes nothing in the show.</p> : (
            <ul className="side-list">
              {proposal.changes.map((c, i) => <li key={i}>{c}</li>)}
            </ul>
          )}
          {proposal.added.length > 0 && (
            <>
              <p className="side-err">This would add problems, so it cannot be applied:</p>
              <ul className="side-list">{proposal.added.map((v, i) => <li key={i}>{v.level === 'error' ? '⛔' : '⚠'} {v.text}</li>)}</ul>
            </>
          )}
          {proposal.fixed.length > 0 && <p className="side-note">It also fixes {proposal.fixed.length} existing problem{proposal.fixed.length > 1 ? 's' : ''}.</p>}
          <div className="side-bar">
            <button className="primary" onClick={apply} disabled={!canApply(proposal) || applied || proposal.changes.length === 0}>
              {applied ? 'Applied' : 'Apply to the draft'}
            </button>
            <button onClick={() => setProposal(null)}>Discard</button>
          </div>
          {applied && (
            <p className="side-note" role="status">
              Applied to the draft. Nothing was sent to the runtime. {untouched ? <button onClick={undo}>Undo this change</button> : 'Use the ↶ button on the Show or Timeline tab to undo.'}
            </p>
          )}
        </section>
      )}
    </>
  )
}
