import { useEffect, useRef } from 'react'
import { DEFAULT_MODEL, setAiSettings, useAiSettings } from '../lib/aiSettings'

/** Settings: for now only the AI helper's key and model. More goes here when there is something real to set. */
export default function SettingsDialog({ onClose }: { onClose: () => void }) {
  const { key, model } = useAiSettings()
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    box.current?.focus()
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Settings" ref={box} tabIndex={-1}>
        <header>
          <h3>Settings</h3>
          <button className="icon-btn" onClick={onClose} aria-label="Close">×</button>
        </header>
        <div className="dialog-body">
          <h4 className="set-h">AI helper</h4>
          <p className="side-note">
            Your own Anthropic key. It is kept in <b>this browser tab only</b> (gone when the tab closes) and is never saved in a show, the runtime or the server.
            It is sent only to Anthropic, when you press Send in the AI helper.
          </p>
          <label className="side-field">
            API key
            <input type="password" value={key} onChange={(e) => setAiSettings({ key: e.target.value })} autoComplete="off" spellCheck={false} placeholder="sk-ant-…" />
          </label>
          <label className="side-field">
            Model
            <input value={model} onChange={(e) => setAiSettings({ model: e.target.value })} spellCheck={false} placeholder={DEFAULT_MODEL} />
          </label>
          <div className="side-bar">
            <button onClick={() => setAiSettings({ key: '' })} disabled={!key}>Forget the key</button>
            <button className="primary" onClick={onClose}>Done</button>
          </div>
        </div>
      </div>
    </div>
  )
}
