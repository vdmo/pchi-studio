// Shared by the Live tab and the department page: how to reach the runtime's feed.

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

export const feedUrl = () => {
  const params = [['token', feedToken()], ['edit_key', signedInKey()], ['session', signedInSession()]].filter(([, v]) => v).map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws${params.length ? '?' + params.join('&') : ''}`
}

/** What one department (or device) last said about itself. */
export type Ready = { ready: boolean; note?: string; ts: number; source?: string; /** when this browser heard it (ms, local clock) */ seen: number }

/** A department page checks in this often; if it goes quiet for STALE_MS it no longer counts as linked. */
export const HEARTBEAT_MS = 10_000
export const STALE_MS = 30_000
export const HEARTBEAT_SOURCE = 'dept-page'
