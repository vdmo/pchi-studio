import { useCallback, useEffect, useRef, useState } from 'react'
import { ASSIST_SYSTEM, assistPrompt, canApply, proposalStatus, readProposal, type PchiNode, type Proposal, type Showchart } from '@pchi/sdk'
import type { History } from './useHistory'
import { DEFAULT_MODEL, getAiSettings } from '../lib/aiSettings'

/**
 * The AI helper's conversation. It lives in the app (one copy) so the floating prompt and the side panel show the same proposal.
 * It can only PROPOSE: Apply edits the draft through the normal undo history and nothing else.
 */
export function useAssistant(chart: Showchart, nodes: PchiNode[], history: History) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [proposal, setProposal] = useState<Proposal | null>(null)
  // the draft as the model was shown it: a proposal replaces the whole show, so it is only safe to apply to exactly that
  const [base, setBase] = useState<Showchart | null>(null)
  const [hasApplied, setHasApplied] = useState(false)
  const abort = useRef<AbortController | null>(null)
  useEffect(() => () => abort.current?.abort(), [])

  const send = useCallback(async () => {
    const { key, model } = getAiSettings()
    if (!key.trim() || !text.trim() || busy) return
    setBusy(true)
    setError('')
    setProposal(null)
    setBase(null)
    setHasApplied(false)
    abort.current?.abort()
    const ac = new AbortController()
    abort.current = ac
    try {
      const r = await fetch('https://api.anthropic.com/v1/messages', {
        method: 'POST',
        signal: ac.signal,
        headers: {
          'content-type': 'application/json',
          'x-api-key': key.trim(),
          'anthropic-version': '2023-06-01',
          'anthropic-dangerous-direct-browser-access': 'true',
        },
        body: JSON.stringify({ model: model.trim() || DEFAULT_MODEL, max_tokens: 16000, system: ASSIST_SYSTEM, messages: [{ role: 'user', content: assistPrompt(chart, nodes, text) }] }),
      })
      const body = await r.json().catch(() => null)
      if (!r.ok) {
        setError(r.status === 401 ? 'The key was not accepted. Check it in Settings.' : `The AI service said no (${r.status}): ${body?.error?.message ?? 'no reason given'}`)
        return
      }
      if (body?.stop_reason === 'max_tokens') {
        setError('The reply was cut off. Ask for a smaller change, or one scene at a time.')
        return
      }
      const reply = (body?.content ?? []).filter((b: { type: string }) => b.type === 'text').map((b: { text: string }) => b.text).join('')
      const res = readProposal(reply, chart, nodes)
      if (!res.ok) setError(res.error)
      else {
        setProposal(res.proposal)
        setBase(chart) // the draft as it was when this request was sent (the closure's copy), not as it is when the reply lands
      }
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setError('Could not reach the AI service. Check the connection.')
    } finally {
      setBusy(false)
    }
  }, [chart, nodes, text, busy])

  // ready: apply it. applied: it is in the draft. stale: the draft changed since, so applying would overwrite those edits.
  const status = proposal && base ? proposalStatus(chart, base, proposal.chart) : null
  const apply = () => {
    if (!proposal || !canApply(proposal) || status !== 'ready') return
    history.commit(proposal.chart)
    setHasApplied(true)
  }
  // Undo is offered only while the draft is still exactly what Apply made it: after any other edit it would undo that edit instead
  const undo = () => {
    if (status === 'applied') history.undo()
  }

  return {
    text, setText, busy, error, proposal, status, hasApplied,
    send, apply, undo,
    cancel: () => abort.current?.abort(),
    discard: () => {
      setProposal(null)
      setBase(null)
    },
  }
}

export type Assistant = ReturnType<typeof useAssistant>

/** What the model proposed: its words, exactly what differs, any problems it would add, and Apply / Discard / Undo. */
export function ProposalCard({ ai }: { ai: Assistant }) {
  const { proposal } = ai
  if (!proposal) return null
  return (
    <section className="proposal" aria-label="Proposal">
      <h4>Proposal</h4>
      <p>{proposal.summary}</p>
      {proposal.changes.length === 0 ? (
        <p className="side-note">It changes nothing in the show.</p>
      ) : (
        <ul className="side-list">
          {proposal.changes.map((c, i) => (
            <li key={i}>{c}</li>
          ))}
        </ul>
      )}
      {proposal.added.length > 0 && (
        <>
          <p className="side-err">This would add problems, so it cannot be applied:</p>
          <ul className="side-list">
            {proposal.added.map((v, i) => (
              <li key={i}>
                {v.level === 'error' ? '⛔' : '⚠'} {v.text}
              </li>
            ))}
          </ul>
        </>
      )}
      {proposal.fixed.length > 0 && (
        <p className="side-note">
          It also fixes {proposal.fixed.length} existing problem{proposal.fixed.length > 1 ? 's' : ''}.
        </p>
      )}
      <div className="side-bar">
        <button className="primary" onClick={ai.apply} disabled={!canApply(proposal) || ai.status !== 'ready' || proposal.changes.length === 0}>
          {ai.status === 'applied' ? 'Applied' : 'Apply to the draft'}
        </button>
        <button onClick={ai.discard}>Discard</button>
      </div>
      {ai.status === 'applied' && (
        <p className="side-note" role="status">
          Applied to the draft. Nothing was sent to the runtime. <button onClick={ai.undo}>Undo this change</button>
        </p>
      )}
      {ai.status === 'stale' && (
        <p className="side-err" role="status">
          {ai.hasApplied ? 'This was applied, and the show has been edited since.' : 'The show has changed since you asked, so this proposal no longer fits it, and applying it would overwrite those changes.'} Ask again to get a proposal for the show as it is now.
        </p>
      )}
    </section>
  )
}

export const AI_PRIVACY =
  'When you press Send, the scenes and cues, and each device’s name, type and allowed program names, go to Anthropic. Addresses, ports and passwords are never sent. It can only propose: nothing is applied until you press Apply, and Apply edits only this draft, which you can undo. It cannot change the rig, arm a device or send anything to the runtime.'
