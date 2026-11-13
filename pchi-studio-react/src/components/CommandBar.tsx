import { useEffect, useRef, useState } from 'react'
import { COMMAND_EXAMPLES, QUICK_SYSTEMS, runStudioCommand, type LiveAction, type LiveResult, type RigState, type Showchart } from '@pchi/sdk'

/** What a confirmed command will do: shown in the voice preview, then applied. */
interface Exec {
  summaries: string[]
  notes: string[]
  apply: () => void
}

interface Props {
  chart?: Showchart
  onApply?: (chart: Showchart) => void
  /** Operate a running show instead of editing the chart: no undo, and the runtime has the last word. */
  live?: { run: (text: string) => LiveResult; apply: (a: LiveAction) => void; examples: string[] }
  /** Undo/redo act on the editor's shared history. They return false when there is nothing to do. */
  onUndo?: () => boolean
  onRedo?: () => boolean
  canUndo?: boolean
  canRedo?: boolean
  /** The rig, so systems can be added and changed by sentence too ("add qlab"). */
  rig?: RigState
  onRig?: (rig: RigState) => void
  /** The state the user is looking at: timeline commands without "in <state>" apply to it. */
  stateContext?: string
  placeholder?: string
}

// Web Speech API is not in every TypeScript DOM lib, so keep a tiny local shape.
interface Recognition {
  lang: string
  interimResults: boolean
  continuous: boolean
  maxAlternatives: number
  onresult: ((e: { results: ArrayLike<ArrayLike<{ transcript: string }> & { isFinal: boolean }> }) => void) | null
  onerror: ((e: { error: string }) => void) | null
  onend: (() => void) | null
  start(): void
  stop(): void
  abort(): void
}
const SpeechRec: (new () => Recognition) | undefined =
  typeof window === 'undefined'
    ? undefined
    : (window as unknown as { SpeechRecognition?: new () => Recognition; webkitSpeechRecognition?: new () => Recognition }).SpeechRecognition ??
      (window as unknown as { webkitSpeechRecognition?: new () => Recognition }).webkitSpeechRecognition

const VOICE_ERRORS: Record<string, string> = {
  'not-allowed': 'Microphone access was blocked. Allow it in the browser, or type the command.',
  'service-not-allowed': 'This browser will not allow speech recognition here. Type the command instead.',
  'no-speech': 'I did not hear anything. Try again.',
  'audio-capture': 'No microphone was found.',
  network: 'The browser’s speech service could not be reached. Type the command instead.',
}

type Message = { kind: 'ok' | 'error' | 'info'; text: string; hint?: string; lines?: string[]; next?: string }

/**
 * Arrange the show by typing or speaking. Both go through runCommandText():
 * typed commands apply at once (and can be undone); spoken ones are shown as a
 * preview first, because a misheard command is the main risk with voice.
 */
export default function CommandBar({ chart, onApply, live, onUndo, onRedo, canUndo, canRedo, rig, onRig, stateContext, placeholder }: Props) {
  const [text, setText] = useState('')
  const [msg, setMsg] = useState<Message | null>(null)
  const [plan, setPlan] = useState<{ exec: Exec; heard: string } | null>(null)
  const [listening, setListening] = useState(false)
  const [askPrivacy, setAskPrivacy] = useState(false)
  const [voiceOk, setVoiceOk] = useState(false) // privacy notice accepted this session
  const [showExamples, setShowExamples] = useState(false)
  const [quick, setQuick] = useState(false) // the quick-add menu
  const [sceneName, setSceneName] = useState('')
  const rec = useRef<Recognition>()
  const inputRef = useRef<HTMLInputElement>(null)
  const chartRef = useRef(chart)
  chartRef.current = chart
  const liveRef = useRef(live)
  liveRef.current = live
  const rigRef = useRef(rig)
  rigRef.current = rig
  const stateRef = useRef(stateContext)
  stateRef.current = stateContext

  useEffect(() => () => rec.current?.abort(), [])

  // Ctrl/Cmd+K jumps to the command box from anywhere
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        inputRef.current?.focus()
        inputRef.current?.select()
      }
    }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [])

  // The undo history lives in the editor (it also covers dragging, deleting and
  // field edits), so a command is just one more change handed to onApply.
  const commit = (x: Exec, label: string, next?: string) => {
    x.apply()
    setPlan(null)
    setText('')
    setMsg({ kind: 'ok', text: label, lines: [...x.summaries, ...x.notes.map((n) => (n.startsWith(':') ? n.slice(2) : `assumed ${n}`))], next })
  }

  const undo = () => setMsg(onUndo?.() ? { kind: 'ok', text: 'Undone.' } : { kind: 'info', text: 'Nothing to undo.' })
  const redo = () => setMsg(onRedo?.() ? { kind: 'ok', text: 'Redone.' } : { kind: 'info', text: 'Nothing to redo.' })

  const run = (input: string, fromVoice: boolean) => {
    setPlan(null)
    const lv = liveRef.current
    if (lv) {
      const r = lv.run(input)
      if (!r.ok) return setMsg({ kind: 'error', text: r.error, hint: r.hint })
      const exec: Exec = { summaries: [r.summary], notes: r.notes, apply: () => lv.apply(r.action) }
      if (fromVoice) {
        setMsg(null)
        return setPlan({ exec, heard: input })
      }
      return commit(exec, 'Sent.')
    }
    const r = runStudioCommand(input, chartRef.current!, rigRef.current ?? { nodes: [], edges: [] }, { state: stateRef.current })
    if (!r.ok) return setMsg({ kind: 'error', text: r.error, hint: r.hint })
    const only = r.steps.length === 1 ? r.steps[0].op : null
    if (only === 'undo' || only === 'redo' || only === 'help') setText('')
    if (only === 'undo') return undo()
    if (only === 'redo') return redo()
    if (only === 'help') {
      setShowExamples(true)
      return setMsg({ kind: 'info', text: 'Here are some things you can say or type:' })
    }
    // a sentence may change the show, the rig, or both: only hand over what actually changed
    const apply = () => {
      if (r.chart !== chartRef.current) onApply?.(r.chart)
      if (r.rigChanged) onRig?.(r.rig)
    }
    const notes = r.rigChanged ? [...r.notes, ': systems are not part of undo: say "remove <name>" to take one out'] : r.notes
    if (fromVoice) {
      setMsg(null)
      setPlan({ exec: { summaries: r.steps.map((s) => s.summary), notes, apply }, heard: input })
    } else commit({ summaries: r.steps.map((s) => s.summary), notes, apply }, 'Done.', r.next)
  }

  const startVoice = () => {
    if (!SpeechRec) return
    if (listening) return rec.current?.stop()
    if (!voiceOk) return setAskPrivacy(true)
    const r = new SpeechRec()
    r.lang = navigator.language || 'en-US'
    r.interimResults = true
    r.continuous = false
    r.maxAlternatives = 1
    r.onresult = (e) => {
      let t = ''
      for (let i = 0; i < e.results.length; i++) t += e.results[i][0].transcript
      setText(t)
      if (e.results[e.results.length - 1].isFinal) run(t, true)
    }
    r.onerror = (e) => setMsg({ kind: 'error', text: VOICE_ERRORS[e.error] ?? `Voice input failed (${e.error}).` })
    r.onend = () => setListening(false)
    rec.current = r
    setMsg(null)
    setPlan(null)
    try {
      r.start()
      setListening(true)
    } catch {
      setMsg({ kind: 'error', text: 'Could not start the microphone.' })
    }
  }

  return (
    <div className="cmdbar">
      <form
        onSubmit={(e) => {
          e.preventDefault()
          if (text.trim()) run(text, false)
        }}
      >
        <div className="inwrap">
          <input
            ref={inputRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={listening ? 'Listening…' : (placeholder ?? 'Type or say a command…')}
            aria-label="Show command"
            spellCheck={false}
          />
          {!text && !listening && <kbd className="kbd">Ctrl K</kbd>}
        </div>
        <button
          type="button"
          className={`mic ${listening ? 'on' : ''}`}
          onClick={startVoice}
          disabled={!SpeechRec}
          aria-pressed={listening}
          aria-label={listening ? 'Stop listening' : 'Speak a command'}
          title={SpeechRec ? (listening ? 'Stop listening' : 'Speak a command') : 'Voice input is not supported in this browser (try Chrome or Edge)'}
        >
          {listening ? '■' : '🎤'}
        </button>
        <button type="submit" disabled={!text.trim()}>Do it</button>
        {!live && (
          <>
            <button type="button" onClick={undo} disabled={!canUndo} title="Undo (Ctrl+Z)">↶</button>
            <button type="button" onClick={redo} disabled={!canRedo} title="Redo (Ctrl+Shift+Z)">↷</button>
          </>
        )}
        {!live && (
          <button type="button" className={quick ? 'on' : ''} onClick={() => setQuick((q) => !q)} aria-expanded={quick} title="Quick add: a scene or a system (QLab, X32, Resolume, grandMA3 ...)">＋ Add</button>
        )}
        <button type="button" onClick={() => setShowExamples((s) => !s)} aria-expanded={showExamples} title="Examples">?</button>
      </form>

      {quick && !live && (
        <div className="cmdnote quickadd" role="group" aria-label="Quick add">
          <h5>Scene</h5>
          <form
            onSubmit={(e) => {
              e.preventDefault()
              if (!sceneName.trim()) return
              run(`add scene ${sceneName.trim()}`, false)
              setSceneName('')
            }}
          >
            <input value={sceneName} onChange={(e) => setSceneName(e.target.value)} placeholder="name, e.g. drop" aria-label="New scene name" spellCheck={false} />
            <button type="submit" disabled={!sceneName.trim()}>＋ Scene</button>
          </form>
          {rig && onRig && (
            <>
              <h5>System (a tool or device the show talks to)</h5>
              <div className="chips">
                {QUICK_SYSTEMS.map((s) => (
                  <button key={s.type} type="button" className="chip" onClick={() => run(s.command, false)} title={`Same as typing "${s.command}"`}>
                    ＋ {s.label}
                  </button>
                ))}
              </div>
              <p className="muted">Or type it: "add x32 called FOH desk at 192.168.1.20". Set a host later with "set qlab host to 192.168.1.50".</p>
            </>
          )}
        </div>
      )}

      {askPrivacy && (
        <div className="cmdnote" role="alertdialog" aria-label="Voice input privacy">
          <p>
            Voice input uses your browser’s built-in speech recognition. In Chrome and Edge the audio is sent to the browser vendor’s cloud
            service to be transcribed. PCHI itself sends nothing anywhere. If that is not acceptable, use typed commands.
          </p>
          <button
            onClick={() => {
              setVoiceOk(true)
              setAskPrivacy(false)
            }}
          >
            OK, use the microphone
          </button>
          <button onClick={() => setAskPrivacy(false)}>Cancel</button>
        </div>
      )}

      {plan && (
        <div className="cmdnote preview" role="group" aria-label="Confirm spoken command">
          <p>
            I heard: <b>“{plan.heard}”</b>
          </p>
          <ul>
            {plan.exec.summaries.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
            {plan.exec.notes.map((n, i) => (
              <li key={`n${i}`} className="muted">assumed {n}</li>
            ))}
          </ul>
          <button autoFocus onClick={() => commit(plan.exec, live ? 'Sent.' : 'Done.')}>{live ? 'Send' : 'Apply'}</button>
          <button onClick={() => setPlan(null)}>Cancel</button>
        </div>
      )}

      <div role="status" aria-live="polite">
        {msg && (
          <div className={`cmdmsg ${msg.kind}`}>
            <strong>{msg.kind === 'ok' ? '✓ ' : msg.kind === 'error' ? '✗ ' : ''}{msg.text}</strong>
            {msg.lines && msg.lines.length > 0 && (
              <ul>
                {msg.lines.map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
              </ul>
            )}
            {msg.hint && (() => {
              const dym = msg.hint.match(/^Did you mean:\s*(.+?)(?:\s+·|$)/)
              return (
                <div className="muted">
                  {dym ? (
                    <>
                      Did you mean{' '}
                      <button type="button" className="suggest" onClick={() => { setText(dym[1]); inputRef.current?.focus() }} title="Put this in the box">
                        {dym[1]}
                      </button>
                      ? <span>{msg.hint.slice(msg.hint.indexOf('·'))}</span>
                    </>
                  ) : (
                    msg.hint
                  )}
                </div>
              )
            })()}
            {msg.next && (
              <div className="muted">
                Next, try{' '}
                <button type="button" className="suggest" onClick={() => { setText(msg.next!); inputRef.current?.focus() }} title="Put this in the box">
                  {msg.next}
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      {showExamples && (
        <div className="examples">
          {(live?.examples ?? COMMAND_EXAMPLES).map((ex) => (
            <button key={ex} type="button" onClick={() => setText(ex)} title="Put this in the box">
              {ex}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
