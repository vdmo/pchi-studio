import { describe, expect, it } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProfilesPanel from '../../src/components/ProfilesPanel'
import { PROFILES, standInRuntime } from './helpers'

describe('Tool profiles dialog', () => {
  it('lists every profile by how it is reached, with what each command rests on', async () => {
    standInRuntime({ '/profiles': { body: PROFILES } })
    render(<ProfilesPanel />)
    await screen.findByText(/3 profiles loaded/)
    expect(screen.getByText('OSC (UDP)')).toBeTruthy()
    expect(screen.getByText('JSON-RPC over TCP')).toBeTruthy()
    expect(screen.getByText('UDP text')).toBeTruthy()
    // a profile opens to show its commands and their basis
    fireEvent.click(screen.getByText('Stage relay box'))
    expect(screen.getByText('lighting.cue')).toBeTruthy()
    expect(screen.getByText('vendor')).toBeTruthy()
    expect(screen.getByText('remembered', { exact: false })).toBeTruthy()
  })

  it('says nothing is proven on real gear', async () => {
    standInRuntime({ '/profiles': { body: PROFILES } })
    render(<ProfilesPanel />)
    await screen.findByText(/none of these has run on real gear/i)
  })

  it('finds a profile by a cue kind, and says when nothing matches', async () => {
    standInRuntime({ '/profiles': { body: PROFILES } })
    render(<ProfilesPanel />)
    await screen.findByText(/3 profiles loaded/)
    const box = screen.getByLabelText('Find a profile')
    fireEvent.change(box, { target: { value: 'audio.control' } })
    expect(screen.queryByText('Stage relay box')).toBeNull()
    expect(screen.getByText('Q-SYS Core (QRC)')).toBeTruthy()
    fireEvent.change(box, { target: { value: 'zzz' } })
    expect(screen.getByText('Nothing matches.')).toBeTruthy()
  })

  it('says plainly when the runtime is not running, and when it refuses', async () => {
    standInRuntime({})
    const { unmount } = render(<ProfilesPanel />)
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/did not answer/))
    unmount()
    standInRuntime({ '/profiles': { status: 401, body: { error: 'token required' } } })
    render(<ProfilesPanel />)
    await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/refused it \(401\)/))
  })
})
