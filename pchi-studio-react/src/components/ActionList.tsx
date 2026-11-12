import type { ShowAction } from '@pchi/sdk'

/**
 * Edits a state's actions without raw JSON: a kind (suggestions use the PIP
 * capability names, anything else is allowed) and its payload fields.
 * Payloads that are not flat key/value pairs fall back to a JSON field.
 */

/** Known kinds and the payload field each one takes. */
export const ACTION_KINDS: Record<string, string> = {
  'scene.activate': 'scene',
  'scene.deactivate': 'scene',
  'lighting.look': 'look',
  'laser.cue': 'cue',
  'audio.scene': 'scene',
  'audio.snapshot': 'snapshot',
  'playlist.play': 'playlist',
  'output.activate': 'target',
  'cue.trigger': 'cue',
}

type Primitive = string | number | boolean

const isFlat = (p: unknown): p is Record<string, Primitive> =>
  !!p && typeof p === 'object' && !Array.isArray(p) && Object.values(p).every((v) => ['string', 'number', 'boolean'].includes(typeof v))

const coerce = (v: string): Primitive => (/^-?\d+(\.\d+)?$/.test(v) ? Number(v) : v)

export default function ActionList({
  title,
  actions,
  onChange,
}: {
  title: string
  actions: ShowAction[] | undefined
  onChange: (a: ShowAction[]) => void
}) {
  const list = actions ?? []
  const set = (i: number, a: ShowAction) => onChange(list.map((x, j) => (j === i ? a : x)))

  const changeKind = (i: number, kind: string) => {
    const a = list[i]
    const field = ACTION_KINDS[kind]
    const untouched = !a.payload || Object.keys(a.payload).length === 0 || (isFlat(a.payload) && Object.values(a.payload).every((v) => v === ''))
    set(i, { ...a, kind, payload: untouched ? (field ? { [field]: '' } : {}) : a.payload })
  }

  return (
    <div className="actions">
      <h5>{title}</h5>
      {list.length === 0 && <p className="muted">None.</p>}
      {list.map((a, i) => (
        <div className="action" key={i}>
          <div className="arow">
            <input list="action-kinds" value={a.kind} onChange={(e) => changeKind(i, e.target.value)} aria-label="Action kind" placeholder="kind, e.g. scene.activate" />
            <button className="x" aria-label="Remove action" onClick={() => onChange(list.filter((_, j) => j !== i))}>×</button>
          </div>
          {isFlat(a.payload) || !a.payload ? (
            <>
              {Object.entries(a.payload ?? {}).map(([k, v]) => (
                <div className="kv" key={k}>
                  <span>{k}</span>
                  <input value={String(v)} aria-label={`${k} value`} onChange={(e) => set(i, { ...a, payload: { ...(a.payload ?? {}), [k]: coerce(e.target.value) } })} />
                  <button
                    className="x"
                    aria-label={`Remove ${k}`}
                    onClick={() => {
                      const rest = { ...(a.payload ?? {}) }
                      delete rest[k]
                      set(i, { ...a, payload: rest })
                    }}
                  >
                    ×
                  </button>
                </div>
              ))}
              <button
                className="link"
                onClick={() => {
                  const k = prompt('Field name (e.g. scene)')?.trim()
                  if (k) set(i, { ...a, payload: { ...(a.payload ?? {}), [k]: '' } })
                }}
              >
                + field
              </button>
            </>
          ) : (
            <input
              defaultValue={JSON.stringify(a.payload)}
              aria-label="Payload (JSON)"
              onBlur={(e) => {
                try {
                  set(i, { ...a, payload: JSON.parse(e.target.value) })
                } catch {
                  e.target.value = JSON.stringify(a.payload)
                }
              }}
            />
          )}
        </div>
      ))}
      <button onClick={() => onChange([...list, { kind: 'scene.activate', payload: { scene: '' } }])}>+ action</button>
      <datalist id="action-kinds">
        {Object.keys(ACTION_KINDS).map((k) => (
          <option key={k} value={k} />
        ))}
      </datalist>
    </div>
  )
}
