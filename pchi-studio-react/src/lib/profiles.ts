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
