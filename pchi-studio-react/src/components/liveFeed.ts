// Shared by the Live tab and the department page: how to reach the runtime's feed.
import { useSyncExternalStore } from 'react'
import { normalizeBase, type Which } from '@pchi/sdk'

/** The runtime may require a token when it is shared over the network: Studio reads it from this browser's storage. */
export const feedToken = () => {
  try {
    return localStorage.getItem('pchi.token') ?? ''
  } catch {
    return ''
  }
}

/** The key this tab signed in with (kept in this tab's session only). The runtime uses it to know who you are. */
export const signedInKey = () => {
  try {
    return sessionStorage.getItem('pchi.editkey') ?? ''
  } catch {
    return ''
  }
}

/** The session token this tab got by signing in (the key itself is not kept). It lives in this tab only. */
export const signedInSession = () => {
  try {
    return sessionStorage.getItem('pchi.session') ?? ''
  } catch {
    return ''
  }
}

// ---- which runtime: the primary (Studio's own /api) or a backup address that took over ----

const BACKUP_KEY = 'pchi.backup'
/** The backup runtime's base URL (for example http://10.0.0.6:3100), kept in this browser. Empty: none. */
export const backupBase = () => {
  try {
    return localStorage.getItem(BACKUP_KEY) ?? ''
  } catch {
    return ''
  }
}

let which: Which = 'primary'
const listeners = new Set<() => void>()
const notify = () => listeners.forEach((l) => l())
export const runtimeWhich = (): Which => (which === 'backup' && backupBase() ? 'backup' : 'primary')

/** Save the backup address ('' clears it). Returns an error in words, or ''. */
export function setBackupBase(text: string): string {
  const r = normalizeBase(text)
  if ('error' in r) return r.error
  try {
    if (r.base) localStorage.setItem(BACKUP_KEY, r.base)
    else localStorage.removeItem(BACKUP_KEY)
  } catch {
    return 'This browser would not keep the address.'
  }
  if (!r.base) which = 'primary'
  notify()
  return ''
}

/** Talk to the other runtime from now on. A session belongs to one runtime, so this tab signs out (the operator key, if typed, still works on both). */
export function followRuntime(next: Which) {
  if (next === which) return
  which = next
  try {
    sessionStorage.removeItem('pchi.session')
  } catch {
    /* nothing to forget */
  }
  notify()
}

/** Re-render when the runtime Studio talks to changes. */
export const useRuntimeWhich = (): Which => useSyncExternalStore((cb) => (listeners.add(cb), () => void listeners.delete(cb)), runtimeWhich)

/** Where the runtime's HTTP side is: Studio's own /api for the primary, the backup's address after a takeover. */
export const apiBase = () => (runtimeWhich() === 'backup' ? backupBase() : '/api')

export const feedUrl = () => {
  const params = [['token', feedToken()], ['edit_key', signedInKey()], ['session', signedInSession()]].filter(([, v]) => v).map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
  const base = runtimeWhich() === 'backup' ? `${backupBase().replace(/^http/, 'ws')}/ws` : `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`
  return `${base}${params.length ? '?' + params.join('&') : ''}`
}

/** What one department (or device) last said about itself. */
export type Ready = { ready: boolean; note?: string; ts: number; source?: string; /** when this browser heard it (ms, local clock) */ seen: number }

/** A department page checks in this often; if it goes quiet for STALE_MS it no longer counts as linked. */
export const HEARTBEAT_MS = 10_000
export const STALE_MS = 30_000
export const HEARTBEAT_SOURCE = 'dept-page'
