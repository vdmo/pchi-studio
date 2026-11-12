// PCHI Core client. Studio only ever speaks events over the network.
import type { PchiEvent } from '@pchi/sdk'

export type Status = 'connecting' | 'online' | 'offline'

export class CoreClient {
  private ws?: WebSocket
  private timer?: number
  private closed = false

  constructor(
    private url: string,
    private onEvent: (e: PchiEvent) => void,
    private onStatus: (s: Status) => void,
  ) {}

  connect() {
    this.onStatus('connecting')
    try {
      this.ws = new WebSocket(this.url)
    } catch {
      this.onStatus('offline')
      this.retry()
      return
    }
    this.ws.onopen = () => this.onStatus('online')
    this.ws.onmessage = (m) => {
      try {
        this.onEvent(JSON.parse(String(m.data)))
      } catch {
        this.onEvent({ type: 'raw', data: String(m.data) })
      }
    }
    this.ws.onclose = () => {
      this.onStatus('offline')
      this.retry()
    }
  }

  private retry() {
    if (this.closed) return
    this.timer = window.setTimeout(() => this.connect(), 2000)
  }

  send(e: PchiEvent) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(e))
      return true
    }
    return false
  }

  close() {
    this.closed = true
    window.clearTimeout(this.timer)
    this.ws?.close()
  }
}

export const coreUrl = () =>
  `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/core/ws`
