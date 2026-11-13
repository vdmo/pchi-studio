import { useEffect, useRef } from 'react'

const GROUPS: { title: string; keys: [string, string][] }[] = [
  {
    title: 'Everywhere',
    keys: [
      ['Ctrl K', 'Type or speak a command'],
      ['Ctrl Z / Ctrl Shift Z', 'Undo / redo'],
      ['?', 'This list'],
    ],
  },
  {
    title: 'Show',
    keys: [
      ['Double-click', 'Add a scene, or rename one'],
      ['F2', 'Rename the selected scene'],
      ['Delete', 'Remove the selected scene or link'],
      ['Drag from a scene edge', 'Connect two scenes'],
    ],
  },
  {
    title: 'Timeline',
    keys: [
      ['Double-click a lane', 'Add a cue'],
      ['Drag / drag the right edge', 'Move / lengthen a cue'],
      ['Space', 'Play or pause the preview'],
    ],
  },
  {
    title: 'Live',
    keys: [
      ['Space', 'GO: send the next event'],
      ['Press GO twice', 'Go on even though a department is on hold'],
    ],
  },
  {
    title: 'Cue sheet',
    keys: [
      ['Edit Q, time, value, note', 'Changes the show; Ctrl Z undoes it'],
      ['Print / CSV', 'A paper or spreadsheet copy'],
    ],
  },
]

export default function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.focus()
    const h = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" tabIndex={-1} ref={ref}>
        <header>
          <h3>Shortcuts</h3>
          <button onClick={onClose} aria-label="Close">×</button>
        </header>
        <div className="dialog-body">
          {GROUPS.map((g) => (
            <section key={g.title}>
              <h5>{g.title}</h5>
              {g.keys.map(([k, d]) => (
                <div className="krow" key={k}>
                  <kbd>{k}</kbd>
                  <span>{d}</span>
                </div>
              ))}
            </section>
          ))}
        </div>
      </div>
    </div>
  )
}
