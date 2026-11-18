import { useEffect, useState } from 'react'
import type { ProfileKinds, ShowAction } from '@pchi/sdk'
import { checkCue, type CueCheck } from '../lib/profiles'

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
  'motion.run': 'program',
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

/**
 * Asks the runtime, a moment after typing stops, what this cue would send to its device, and shows that or the reason it would be
 * refused. Nothing is sent. When the runtime does not know the device yet (the rig has not been sent), it says so and checks nothing.
 */
function CueChecker({ deviceId, kind, value }: { deviceId: string; kind: string; value: string }) {
  const [res, setRes] = useState<CueCheck | null>(null)
  useEffect(() => {
    setRes(null)
    if (!kind.trim()) return
    let live = true
    const t = setTimeout(() => checkCue(deviceId, kind, value).then((r) => live && setRes(r)), 400)
    return () => {
      live = false
      clearTimeout(t)
    }
  }, [deviceId, kind, value])
  if (!res) return null
  if (res.ok) return <p className="muted" role="status" title="What the runtime would send for this cue. Nothing was sent.">Would send: {res.lines.join(' · ')}</p>
  if (res.unknown) return <p className="muted" role="status">Not checked: {res.error}. Send the show to the runtime to check this cue against the device.</p>
  return <p className="muted tc-warn" role="status">The device would refuse this: {res.error}</p>
}

export default function ActionList({
  title,
  actions,
  onChange,
  profile,
  deviceId,
}: {
  title: string
  actions: ShowAction[] | undefined
  onChange: (a: ShowAction[]) => void
  /** The profile of the device these actions go to: its kinds are suggested, and its hint says what a value looks like. */
  profile?: ProfileKinds
  /** The rig device these actions go to: when given, each action is checked against it as it is typed. */
  deviceId?: string
}) {
  const list = actions ?? []
  const set = (i: number, a: ShowAction) => onChange(list.map((x, j) => (j === i ? a : x)))

  const changeKind = (i: number, kind: string) => {
    const a = list[i]
    const field = ACTION_KINDS[kind] ?? (profile?.kinds.includes(kind) ? 'value' : undefined)
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
            <input list={profile && profile.kinds.length > 0 ? 'action-kinds-profile' : 'action-kinds'} value={a.kind} onChange={(e) => changeKind(i, e.target.value)} aria-label="Action kind" placeholder="kind, e.g. scene.activate" />
            <button className="x" aria-label="Remove action" onClick={() => onChange(list.filter((_, j) => j !== i))}>×</button>
          </div>
          {profile && profile.kinds.length > 0 && (
            <p className="muted" title="From the profile of the device this cue goes to">
              {profile.name} takes {profile.kinds.join(', ')}.{profile.kinds.includes(a.kind) ? '' : ' This kind is not one of them.'}
              {profile.hint ? ` ${profile.hint}` : ''}
            </p>
          )}
          {deviceId && <CueChecker deviceId={deviceId} kind={a.kind} value={isFlat(a.payload) ? String(Object.values(a.payload)[0] ?? '') : ''} />}
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
      <datalist id="action-kinds-profile">
        {(profile?.kinds ?? []).map((k) => (
          <option key={k} value={k} />
        ))}
      </datalist>
      <datalist id="action-kinds">
        {Object.keys(ACTION_KINDS).map((k) => (
          <option key={k} value={k} />
        ))}
      </datalist>
    </div>
  )
}
