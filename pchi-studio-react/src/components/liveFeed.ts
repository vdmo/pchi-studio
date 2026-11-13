// Shared by the Live tab and the department page: how to reach the runtime's feed.

/** The runtime may require a token when it is shared over the network: Studio reads it from this browser's storage. */
export const feedToken = () => {
  try {
    return localStorage.getItem('pchi.token') ?? ''
  } catch {
    return ''
  }
}

export const feedUrl = () => {
  const t = feedToken()
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws${t ? `?token=${encodeURIComponent(t)}` : ''}`
}

/** What one department (or device) last said about itself. */
export type Ready = { ready: boolean; note?: string; ts: number; source?: string; /** when this browser heard it (ms, local clock) */ seen: number }

/** A department page checks in this often; if it goes quiet for STALE_MS it no longer counts as linked. */
export const HEARTBEAT_MS = 10_000
export const STALE_MS = 30_000
export const HEARTBEAT_SOURCE = 'dept-page'
