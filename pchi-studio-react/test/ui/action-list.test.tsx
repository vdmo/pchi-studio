import { describe, expect, it } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import ActionList from '../../src/components/ActionList'
import { standInRuntime } from './helpers'

const profile = { id: 'stage-box', name: 'Stage relay box', hint: 'a cue number, 1 to 999', kinds: ['lighting.cue', 'lighting.blackout'] }
const action = (kind: string, value: unknown) => [{ kind, payload: { value } }]
const wait = { timeout: 3000 }

describe('cue editor', () => {
  it('names the profile, what it takes, and its hint; and warns about a kind it does not take', () => {
    const { rerender } = render(<ActionList title="Action" profile={profile} actions={action('lighting.cue', '')} onChange={() => {}} />)
    expect(screen.getByText(/Stage relay box takes lighting.cue, lighting.blackout\./).textContent).toContain('a cue number, 1 to 999')
    expect(screen.queryByText(/not one of them/)).toBeNull()
    rerender(<ActionList title="Action" profile={profile} actions={action('lighting.look', '')} onChange={() => {}} />)
    expect(screen.getByText(/This kind is not one of them/)).toBeTruthy()
  })

  it('shows what the runtime says a cue would send', async () => {
    const calls = standInRuntime({ '/devices/u1/check': { body: { ok: true, sent_as: [{ address: 'Stage relay box UDP to 10.0.0.5:7000: GO 12', args: [] }] } } })
    render(<ActionList title="Action" profile={profile} deviceId="u1" actions={action('lighting.cue', 12)} onChange={() => {}} />)
    const line = await screen.findByText(/Would send:/, {}, wait)
    expect(line.textContent).toContain('GO 12')
    const sent = calls.find((c) => c.path === '/devices/u1/check')!
    expect(JSON.parse(String(sent.init?.body))).toEqual({ kind: 'lighting.cue', value: '12' })
  })

  it('shows the reason a cue would be refused, in the profile words', async () => {
    standInRuntime({ '/devices/u1/check': { status: 400, body: { ok: false, error: "lighting.blackout for Stage relay box takes no value, but the cue has '5'" } } })
    render(<ActionList title="Action" profile={profile} deviceId="u1" actions={action('lighting.blackout', 5)} onChange={() => {}} />)
    const line = await screen.findByText(/would refuse this/, {}, wait)
    expect(line.textContent).toContain("takes no value, but the cue has '5'")
  })

  it('does not call a missing device a fault in the cue', async () => {
    standInRuntime({ '/devices/u1/check': { status: 400, body: { ok: false, error: "no device 'u1' in the rig" } } })
    render(<ActionList title="Action" deviceId="u1" actions={action('lighting.cue', 1)} onChange={() => {}} />)
    const line = await screen.findByText(/Not checked/, {}, wait)
    expect(line.textContent).toContain('Send the show to the runtime')
    expect(screen.queryByText(/would refuse/)).toBeNull()
  })

  it('says the runtime did not answer rather than showing a stale result', async () => {
    standInRuntime({})
    render(<ActionList title="Action" deviceId="u1" actions={action('lighting.cue', 1)} onChange={() => {}} />)
    const line = await screen.findByText(/Not checked/, {}, wait)
    expect(line.textContent).toContain('did not answer')
  })

  it('asks nothing when there is no device, and edits the value', () => {
    const calls = standInRuntime({})
    let changed: unknown
    render(<ActionList title="Action" profile={profile} actions={action('lighting.cue', '')} onChange={(a) => (changed = a)} />)
    fireEvent.change(screen.getByLabelText('value value'), { target: { value: '7' } })
    expect(changed).toEqual([{ kind: 'lighting.cue', payload: { value: 7 } }])
    expect(calls).toHaveLength(0)
  })

  it('a typed kind the profile takes keeps a plain value field', () => {
    let changed: any
    render(<ActionList title="Action" profile={profile} actions={[{ kind: 'scene.activate', payload: { scene: '' } }]} onChange={(a) => (changed = a)} />)
    fireEvent.change(screen.getByLabelText('Action kind'), { target: { value: 'lighting.cue' } })
    expect(changed[0].kind).toBe('lighting.cue')
    expect(changed[0].payload).toEqual({ value: '' })
  })
})
