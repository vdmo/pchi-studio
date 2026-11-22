// The whole Studio, rendered with no runtime running (or a stand-in one): does it start, does every tab open, and do the main
// flows work. This cannot see layout or colour; it catches a screen that throws, a button that does nothing, a flow that breaks.
import { describe, expect, it, vi } from 'vitest'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import App from '../../src/App'
import { PROFILES, standInRuntime } from './helpers'

const tab = (name: string) => within(screen.getByRole('tablist', { name: 'Views' })).getByRole('tab', { name })
const click = async (el: HTMLElement) => {
  await act(async () => {
    fireEvent.click(el)
  })
}
const button = (text: string | RegExp) => screen.getAllByRole('button').find((b) => (typeof text === 'string' ? b.textContent?.includes(text) : text.test(b.textContent ?? '')))!

describe('Studio smoke', () => {
  it('starts with no runtime and every tab opens without an error', async () => {
    standInRuntime({})
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<App />)
    for (const name of ['Show', 'Timeline', 'Cue sheet', 'Rig', 'Live', 'Show']) {
      await click(tab(name))
    }
    // the Live tab says it has no runtime instead of pretending
    await click(tab('Live'))
    await waitFor(() => expect(document.body.textContent).toMatch(/offline|not connected|no runtime|connecting/i))
    expect(errors.mock.calls.filter((c) => /Uncaught|Error:|The above error/.test(String(c[0])))).toEqual([])
  })

  it('adds a device from a runtime profile on the Rig tab', async () => {
    standInRuntime({ '/profiles': { body: PROFILES } })
    render(<App />)
    await click(tab('Rig'))
    await click(button('Add a system'))
    const heading = await screen.findByText('From a tool profile')
    // built-ins with their own entry are not offered a second time in this section; the profile with none is
    const section = heading.parentElement as HTMLElement
    const chip = await within(section).findByRole('button', { name: /\+ Stage relay box/ })
    expect(within(section).queryByRole('button', { name: /X32/ })).toBeNull()
    expect(within(section).queryByRole('button', { name: /Q-SYS/ })).toBeNull()
    await click(chip)
    // the new device is selected with its profile, port and department filled in
    await waitFor(() => expect(screen.getByDisplayValue('stage-box')).toBeTruthy())
    expect(screen.getByDisplayValue('7000')).toBeTruthy()
    expect((screen.getByDisplayValue('Stage relay box') as HTMLInputElement).value).toBe('Stage relay box')
  })

  it('a runtime that is not running leaves the Rig tab usable and says why the profile list is missing', async () => {
    standInRuntime({})
    render(<App />)
    await click(tab('Rig'))
    await click(button('Add a system'))
    await screen.findByText(/profile list could not be read/)
    // the built-in systems can still be added
    expect(button('+ Vork')).toBeTruthy()
  })

  it('a new cue to a profile device starts as a kind its profile takes, with the profile hint', async () => {
    standInRuntime({ '/profiles': { body: PROFILES } })
    render(<App />)
    await click(tab('Rig'))
    await click(button('Add a system'))
    await click(await screen.findByRole('button', { name: /\+ Stage relay box/ }))
    await click(tab('Timeline'))
    await click(button('Add Visuals, Lighting'))
    const tracks = document.querySelectorAll('.tl-track')
    expect(tracks.length).toBeGreaterThanOrEqual(2)
    // the Lighting lane is the second one; double-click it to add a cue
    await act(async () => {
      fireEvent.doubleClick(tracks[1], { clientX: 200, clientY: 20 })
    })
    const kind = (await screen.findByLabelText('Action kind')) as HTMLInputElement
    expect(kind.value).toBe('lighting.cue')
    expect(screen.getByText(/Stage relay box takes lighting.cue, lighting.blackout\./).textContent).toContain('a cue number, 1 to 999')
  })
})
