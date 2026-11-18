import { apiBase, feedToken, signedInSession } from '../components/liveFeed'
import type { ProfileInfo } from '@pchi/sdk'

export type ProfileAction = { kind: string; basis: string; source: string }
export type Profile = ProfileInfo & { built_in: boolean; notes?: string; kinds: string[]; actions?: ProfileAction[] }

/** The runtime's own list of tool profiles (GET /profiles). Throws with a sentence a person can read. */
export async function fetchProfiles(): Promise<Profile[]> {
  const t = feedToken()
  const s = signedInSession()
  const r = await fetch(`${apiBase()}/profiles`, { headers: { ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(s ? { 'X-Pchi-Session': s } : {}) } }).catch(() => null)
  if (!r) throw new Error('the runtime did not answer')
  if (!r.ok) throw new Error(`the runtime refused it (${r.status})`)
  const body = await r.json().catch(() => null)
  if (!Array.isArray(body)) throw new Error('the runtime sent something else')
  return body as Profile[]
}

export type CueCheck = { ok: true; lines: string[] } | { ok: false; error: string; unknown?: boolean }

/**
 * Ask the runtime what a cue of this kind and value would send to a device, or why it would be refused. Nothing is sent. The runtime
 * knows the device only if the rig was sent to it, so "no device" is reported as unknown, not as a fault in the cue.
 */
export async function checkCue(deviceId: string, kind: string, value: string): Promise<CueCheck> {
  const t = feedToken()
  const s = signedInSession()
  const r = await fetch(`${apiBase()}/devices/${encodeURIComponent(deviceId)}/check`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...(t ? { Authorization: `Bearer ${t}` } : {}), ...(s ? { 'X-Pchi-Session': s } : {}) },
    body: JSON.stringify({ kind, value }),
  }).catch(() => null)
  if (!r) return { ok: false, error: 'the runtime did not answer', unknown: true }
  const body = (await r.json().catch(() => null)) as { error?: string; sent_as?: { address: string; args: unknown[] }[] } | null
  if (r.ok) return { ok: true, lines: (body?.sent_as ?? []).map((l) => [l.address, ...(l.args ?? []).map(String)].join(' ')) }
  const error = String(body?.error ?? `the runtime refused it (${r.status})`)
  return { ok: false, error, unknown: /^no device|has no rig/.test(error) }
}
