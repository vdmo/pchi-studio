// Guards against the kind of damage a bad edit does to source files: characters re-encoded into garbage ("›" becoming "âº"), or a
// replacement character left behind. This happened once to RigPanel.tsx and was only found by looking at the app.
import { describe, expect, it } from 'vitest'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f)
    return statSync(p).isDirectory() ? files(p) : /\.(tsx?|css|html)$/.test(f) ? [p] : []
  })

// a UTF-8 lead byte read as Latin-1 / Windows-1252, followed by a continuation byte read the same way
const MOJIBAKE = /[Â-ô][\u0080-¿ŒœŠšŸŽžƒˆ˜–—‘-„†-•…‰‹›€™]/

describe('source hygiene', () => {
  const sources = [...files('src'), ...files('../pchi-sdk/src')]

  it('finds the sources', () => {
    expect(sources.length).toBeGreaterThan(20)
  })

  it.each(sources)('%s has no re-encoded characters', (f) => {
    const text = readFileSync(f, 'utf8')
    const bad = text.split('\n').map((l, i) => [i + 1, l] as const).filter(([, l]) => MOJIBAKE.test(l) || l.includes('�'))
    expect(bad.map(([n, l]) => `${f}:${n}: ${l.trim().slice(0, 80)}`)).toEqual([])
  })

  it('would catch the damage it guards against', () => {
    expect(MOJIBAKE.test('<span>âº</span>')).toBe(true) // "›" re-encoded
    expect(MOJIBAKE.test('ï¼‹ Add')).toBe(true)
    expect(MOJIBAKE.test('Café × … › ＋ ✓')).toBe(false) // real text is fine
  })
})
