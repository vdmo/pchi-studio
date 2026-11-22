import { vi } from 'vitest'

export type Reply = { status?: number; body: unknown }

/** A stand-in runtime: answers fetch by path. Anything not listed is "the runtime did not answer". Returns the calls made. */
export function standInRuntime(routes: Record<string, Reply | ((init?: RequestInit) => Reply)>) {
  const calls: { path: string; init?: RequestInit }[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      const path = url.replace(/^https?:\/\/[^/]+/, '').replace(/^\/api/, '').split('?')[0]
      calls.push({ path, init })
      const r = routes[path]
      if (!r) throw new TypeError('the runtime did not answer')
      const { status = 200, body } = typeof r === 'function' ? r(init) : r
      return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
    }),
  )
  return calls
}

export const PROFILES = [
  { id: 'x32', name: 'Behringer X32 / M32', dept: 'audio', transport: 'osc', built_in: true, port: null, kinds: ['audio.snapshot'], hint: 'a scene number' },
  {
    id: 'qsys', name: 'Q-SYS Core (QRC)', dept: 'audio', transport: 'jsonrpc-tcp', built_in: true, port: 1710, hint: 'Main Gain=-10', notes: 'from memory',
    kinds: ['audio.control', 'audio.snapshot'],
    actions: [
      { kind: 'audio.control', basis: 'memory', source: 'Q-SYS QRC, from memory' },
      { kind: 'audio.snapshot', basis: 'memory', source: 'Q-SYS QRC, from memory' },
    ],
  },
  {
    id: 'stage-box', name: 'Stage relay box', dept: 'lighting', transport: 'udp', built_in: false, port: 7000, hint: 'a cue number, 1 to 999', notes: '',
    kinds: ['lighting.cue', 'lighting.blackout'],
    actions: [
      { kind: 'lighting.cue', basis: 'vendor', source: 'manual p. 12' },
      { kind: 'lighting.blackout', basis: 'memory', source: 'remembered' },
    ],
  },
]
