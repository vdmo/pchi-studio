// What jsdom lacks and the app (or React Flow) expects, and a runtime that answers nothing until a test says otherwise.
import { afterEach, vi } from 'vitest'
import { cleanup } from '@testing-library/react'

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = globalThis.ResizeObserver ?? (ResizeObserverStub as unknown as typeof ResizeObserver)
if (!globalThis.DOMMatrixReadOnly) {
  // React Flow reads the viewport transform through this
  ;(globalThis as any).DOMMatrixReadOnly = class {
    m22 = 1
    constructor(_t?: string) {}
  }
}
Object.defineProperty(window, 'matchMedia', { writable: true, value: (q: string) => ({ matches: false, media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, onchange: null, dispatchEvent: () => false }) })
window.HTMLElement.prototype.scrollIntoView = window.HTMLElement.prototype.scrollIntoView ?? (() => {})

/** A WebSocket that never connects: the Live tab shows "offline", which is what it should do with no runtime. */
class OfflineSocket {
  static OPEN = 1
  readyState = 3
  onopen: unknown = null
  onclose: unknown = null
  onerror: unknown = null
  onmessage: unknown = null
  constructor(_url: string) {
    setTimeout(() => (this.onerror as (() => void) | null)?.(), 0)
  }
  send() {}
  close() {}
}
;(globalThis as any).WebSocket = OfflineSocket

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  try {
    localStorage.clear()
    sessionStorage.clear()
  } catch {
    /* nothing stored */
  }
})
