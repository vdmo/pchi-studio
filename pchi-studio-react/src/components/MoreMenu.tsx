import { useEffect, useRef, useState } from 'react'

export interface MoreItem {
  label: string
  onClick: () => void
  disabled?: boolean
  hint?: string
  /** a rule above this item */
  group?: boolean
}

/** The ⋯ menu in the bottom-right corner: the things you reach for on every tab (undo, redo, fit, help, settings). */
export default function MoreMenu({ items }: { items: MoreItem[] }) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const down = (e: MouseEvent) => root.current && !root.current.contains(e.target as Node) && setOpen(false)
    const key = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    window.addEventListener('mousedown', down)
    window.addEventListener('keydown', key)
    return () => {
      window.removeEventListener('mousedown', down)
      window.removeEventListener('keydown', key)
    }
  }, [open])
  return (
    <div className="more" ref={root}>
      {open && (
        <div className="more-menu" role="menu">
          {items.map((it) => (
            <button
              key={it.label}
              role="menuitem"
              className={it.group ? 'sep' : ''}
              disabled={it.disabled}
              onClick={() => {
                setOpen(false)
                it.onClick()
              }}
            >
              <span>{it.label}</span>
              {it.hint && <kbd>{it.hint}</kbd>}
            </button>
          ))}
        </div>
      )}
      <button className="more-btn" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open} aria-label="More: undo, redo, fit, help and settings" title="More">
        ⋯
      </button>
    </div>
  )
}
