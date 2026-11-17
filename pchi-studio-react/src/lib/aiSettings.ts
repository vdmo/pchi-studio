// The AI helper's key and model. Kept for this browser tab only (session storage), never in the show, the runtime or the server.
// One store, so Settings, the side panel and the floating prompt all see the same key.
import { useSyncExternalStore } from 'react'

const KEY = 'pchi.ai.key'
const MODEL = 'pchi.ai.model'
export const DEFAULT_MODEL = 'claude-sonnet-5-5'

const read = (k: string) => {
  try {
    return sessionStorage.getItem(k) ?? ''
  } catch {
    return ''
  }
}
const write = (k: string, v: string) => {
  try {
    if (v) sessionStorage.setItem(k, v)
    else sessionStorage.removeItem(k)
  } catch {
    /* blocked: the value then lives only while this page is open */
  }
}

// a new object on every change, so useSyncExternalStore sees it
let state = { key: read(KEY), model: read(MODEL) || DEFAULT_MODEL }
const listeners = new Set<() => void>()

export function setAiSettings(next: Partial<typeof state>) {
  state = { ...state, ...next }
  if (next.key !== undefined) write(KEY, state.key.trim())
  if (next.model !== undefined) write(MODEL, state.model.trim() === DEFAULT_MODEL ? '' : state.model.trim())
  listeners.forEach((l) => l())
}

export const getAiSettings = () => state

export function useAiSettings() {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l)
      return () => listeners.delete(l)
    },
    () => state,
  )
}
