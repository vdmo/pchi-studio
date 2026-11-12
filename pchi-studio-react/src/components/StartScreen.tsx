import { useState } from 'react'
import { runCommandText, type Showchart } from '@pchi/sdk'

/**
 * Starting points for an empty show. Each one is just a command sentence, so
 * what the template does is visible, editable, and goes through the same parser
 * as everything typed or spoken.
 */
export const TEMPLATES: { name: string; blurb: string; command: string }[] = [
  {
    name: 'Festival set',
    blurb: 'Preshow to finale, with a timeline in the drop',
    command: 'chain preshow, intro, build, drop, breakdown, finale; add the standard lanes in drop; set drop length to 30 seconds',
  },
  {
    name: 'Theatre show',
    blurb: 'Acts, an interval and curtain call',
    command: 'chain houseopen, act1, interval, act2, bows',
  },
  {
    name: 'Installation',
    blurb: 'Loops by itself: idle, attract, interact',
    command: 'chain idle, attract, interact, cooldown; from cooldown to idle after 10 seconds',
  },
]

const IDEAS = ['chain preshow, intro, drop, finale', 'chain idle, attract, interact', 'add state intro']

export default function StartScreen({ chart, onApply }: { chart: Showchart; onApply: (c: Showchart) => void }) {
  const [text, setText] = useState('')
  const [error, setError] = useState<{ text: string; hint?: string } | null>(null)

  const run = (input: string) => {
    const r = runCommandText(input, chart)
    if (!r.ok) return setError({ text: r.error, hint: r.hint })
    setError(null)
    onApply(r.chart)
  }

  return (
    <div className="start" role="region" aria-label="Start a show" onDoubleClick={(e) => e.stopPropagation()}>
      <h2>What is the show?</h2>
      <p className="muted">Describe it in a sentence, or start from a template. You can change everything afterwards.</p>
      <form
        onSubmit={(e) => {
          e.preventDefault()
          if (text.trim()) run(text)
        }}
      >
        <input autoFocus value={text} onChange={(e) => setText(e.target.value)} placeholder="e.g. chain preshow, intro, drop, finale" aria-label="Describe the show" spellCheck={false} />
        <button type="submit" className="primary" disabled={!text.trim()}>Build it</button>
      </form>
      {error && (
        <p className="start-err" role="alert">
          ✗ {error.text} {error.hint && <span className="muted">{error.hint}</span>}
        </p>
      )}
      <div className="start-ideas">
        {IDEAS.map((i) => (
          <button key={i} type="button" onClick={() => setText(i)}>{i}</button>
        ))}
      </div>
      <div className="start-templates">
        {TEMPLATES.map((t) => (
          <button key={t.name} type="button" onClick={() => run(t.command)} title={t.command}>
            <b>{t.name}</b>
            <span className="muted">{t.blurb}</span>
          </button>
        ))}
      </div>
      <p className="muted start-foot">
        Or double-click the canvas to add a state, press <kbd>Ctrl K</kbd> to type or speak commands, or <b>Open</b> a saved .pchi file.
      </p>
    </div>
  )
}
