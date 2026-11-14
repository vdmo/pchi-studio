// Which runtime Studio should talk to when there is a primary and a backup (a standby that takes over). Pure functions; the Studio only
// decides how to poll and show it. The runtime answers GET /role with its role: active, standby or fenced (see docs/failover.md).

export type Which = 'primary' | 'backup'

/** What a runtime said about itself, or null if it did not answer. */
export type RoleInfo = { role: 'active' | 'standby' | 'fenced'; epoch?: number } | null

const active = (r: RoleInfo) => r?.role === 'active'

/**
 * Where Studio should talk next. It stays where it is while that runtime is the active one, and moves only when it is not and the other one is.
 * When both claim to be active (a takeover that has not yet stopped the old primary), the one on the later takeover wins; on a tie Studio stays put.
 * With no backup address, or when neither answers as active, it stays.
 */
export function chooseRuntime(current: Which, primary: RoleInfo, backup: RoleInfo, hasBackup: boolean): Which {
  if (!hasBackup) return 'primary'
  const [p, b] = [active(primary), active(backup)]
  if (p && b) {
    const [pe, be] = [primary?.epoch ?? 0, backup?.epoch ?? 0]
    return be > pe ? 'backup' : pe > be ? 'primary' : current
  }
  if (p) return 'primary'
  if (b) return 'backup'
  return current
}

/** A backup address as typed ("10.0.0.6:3100", "http://show-b:3100/") made into a base URL, or an error in words. Empty clears it. */
export function normalizeBase(text: string): { base: string } | { error: string } {
  const t = text.trim()
  if (!t) return { base: '' }
  const withScheme = /^https?:\/\//i.test(t) ? t : `http://${t}`
  try {
    const u = new URL(withScheme)
    if (!u.hostname) return { error: 'That is not an address.' }
    if (u.pathname !== '/' && u.pathname !== '') return { error: 'Give just the address (host and port), not a path.' }
    return { base: `${u.protocol}//${u.host}` }
  } catch {
    return { error: 'That is not an address. Write it like 10.0.0.6:3100.' }
  }
}
