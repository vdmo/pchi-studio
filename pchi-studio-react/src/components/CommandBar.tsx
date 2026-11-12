import { useEffect, useRef, useState } from 'react'
import { COMMAND_EXAMPLES, runCommandText, type CommandResult, type Showchart } from '@pchi/sdk'

type Ok = Extract<CommandResult, { ok: true }>

interface Props {
  chart: Showchart
  onApply: (chart: Showchart) => void
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

type Message = { kind: 'ok' | 'error' | 'info'; text: string; hint?: string; lines?: string[] }

/**
 * Arrange the show by typing or speaking. Both go through runCommandText():
 * typed commands apply at once (and can be undone); spoken ones are shown as a
 * preview first, because a misheard command is the main risk with voice.
 */
export default function CommandBar({ chart, onApply }: Props) {
  const [text, setText] = useState('')
  const [msg, setMsg] = useState<Message | null>(null)
  const [plan, setPlan] = useState<{ result: Ok; heard: string } | null>(null)
  const [past, setPast] = useState<Showchart[]>([])
  const [future, setFuture] = useState<Showchart[]>([])
  const [listening, setListening] = useState(false)
  const [askPrivacy, setAskPrivacy] = useState(false)
  const [voiceOk, setVoiceOk] = useState(false) // privacy notice accepted this session
  const [showExamples, setShowExamples] = useState(false)
  const rec = useRef<Recognition>()
  const chartRef = useRef(chart)
  chartRef.current = chart

  useEffect(() => () => rec.current?.abort(), [])

  // Read the chart BEFORE queuing state updates: updater functions can run more than
  // once (and after the new chart has rendered), so they must not read a mutable ref.
  const commit = (r: Ok, label: string) => {
    const before = chartRef.current
    setPast((p) => [...p.slice(-49), before])
    setFuture([])
    onApply(r.chart)
    setPlan(null)
    setText('')
    setMsg({ kind: 'ok', text: label, lines: [...r.steps.map((s) => s.summary), ...r.notes.map((n) => `assumed ${n}`)] })
  }

  const undo = () => {
    const prev = past.at(-1)
    if (!prev) return setMsg({ kind: 'info', text: 'Nothing to undo.' })
    const now = chartRef.current
    setFuture((f) => [now, ...f])
    setPast((p) => p.slice(0, -1))
    onApply(prev)
    setMsg({ kind: 'ok', text: 'Undone.' })
  }
  const redo = () => {
    const next = future[0]
    if (!next) return setMsg({ kind: 'info', text: 'Nothing to redo.' })
    const now = chartRef.current
    setPast((p) => [...p, now])
    setFuture((f) => f.slice(1))
    onApply(next)
    setMsg({ kind: 'ok', text: 'Redone.' })
  }

  const run = (input: string, fromVoice: boolean) => {
    const r = runCommandText(input, chartRef.current)
    setPlan(null)
    if (!r.ok) return setMsg({ kind: 'error', text: r.error, hint: r.hint })
    const only = r.steps.length === 1 ? r.steps[0].command.op : null
    if (only === 'undo' || only === 'redo' || only === 'help') setText('')
    if (only === 'undo') return undo()
    if (only === 'redo') return redo()
    if (only === 'help') {
      setShowExamples(true)
      return setMsg({ kind: 'info', text: 'Here are some things you can say or type:' })
    }
    if (fromVoice) {
      setMsg(null)
      setPlan({ result: r, heard: input })
    } else commit(r, 'Done.')
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
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={listening ? 'Listening…' : 'Arrange the show: “chain preshow, intro, drop, finale” or “when entering drop set lighting look blue”'}
          aria-label="Show command"
          spellCheck={false}
        />
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
        <button type="button" onClick={undo} disabled={!past.length} title="Undo the last command">↶</button>
        <button type="button" onClick={redo} disabled={!future.length} title="Redo">↷</button>
        <button type="button" onClick={() => setShowExamples((s) => !s)} aria-expanded={showExamples} title="Examples">?</button>
      </form>

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
            {plan.result.steps.map((s, i) => (
              <li key={i}>{s.summary}</li>
            ))}
            {plan.result.notes.map((n, i) => (
              <li key={`n${i}`} className="muted">assumed {n}</li>
            ))}
          </ul>
          <button autoFocus onClick={() => commit(plan.result, 'Done.')}>Apply</button>
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
            {msg.hint && <div className="muted">{msg.hint}</div>}
          </div>
        )}
      </div>

      {showExamples && (
        <div className="examples">
          {COMMAND_EXAMPLES.map((ex) => (
            <button key={ex} type="button" onClick={() => setText(ex)} title="Put this in the box">
              {ex}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
