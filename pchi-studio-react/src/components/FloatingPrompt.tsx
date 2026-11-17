import { useEffect, useRef } from 'react'
import { ProposalCard, type Assistant } from './assistant'
import { useAiSettings } from '../lib/aiSettings'

/**
 * A small prompt floating at the bottom of the editor: ask for a change without opening the side panel. It is the same helper as
 * the AI tab (one conversation), so everything it says about proposing instead of doing applies here too.
 */
export default function FloatingPrompt({ ai, onClose, onSidebar, onSettings }: { ai: Assistant; onClose: () => void; onSidebar: () => void; onSettings: () => void }) {
  const { key } = useAiSettings()
  const ready = key.trim() !== ''
  const box = useRef<HTMLTextAreaElement>(null)
  useEffect(() => box.current?.focus(), [])
  return (
    <div className="float-ai" role="dialog" aria-label="Ask AI to update this show">
      <button className="icon-btn float-x" onClick={onClose} aria-label="Close">×</button>
      <textarea
        ref={box}
        rows={2}
        value={ai.text}
        onChange={(e) => ai.setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onClose()
          else if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault()
            if (ready) ai.send()
          }
        }}
        placeholder="Ask AI to update this show…"
        aria-label="What should change?"
      />
      <div className="float-bar">
        <button className="link" onClick={onSidebar} title="Move this to the side panel, where there is more room">Open in sidebar</button>
        <span className="spacer" />
        {!ready && <button onClick={onSettings}>Add your key</button>}
        {ai.busy && <button onClick={ai.cancel}>Cancel</button>}
        <button className="primary" onClick={ai.send} disabled={ai.busy || !ready || !ai.text.trim()} aria-label="Send">
          {ai.busy ? '…' : '↑'}
        </button>
      </div>
      {ai.error && <p className="side-err" role="alert">{ai.error}</p>}
      {ai.proposal && (
        <div className="float-result">
          <ProposalCard ai={ai} />
        </div>
      )}
      <p className="side-note float-note">A proposal only: nothing changes until you press Apply, and the rig and runtime are never touched. Your show (without addresses or passwords) goes to Anthropic when you send.</p>
    </div>
  )
}
