import { useState, type ReactNode } from 'react'

/** A floating panel over the canvas, collapsible, so the diagram stays the focus. */
export default function Card({
  title,
  className = '',
  onClose,
  tabs,
  children,
}: {
  title: ReactNode
  className?: string
  onClose?: () => void
  /** Extra controls shown in the header (e.g. tab buttons). */
  tabs?: ReactNode
  children: ReactNode
}) {
  const [collapsed, setCollapsed] = useState(false)
  return (
    <section className={`card ${className}`}>
      <header>
        <h4>{title}</h4>
        {tabs}
        <span className="spacer" />
        <button className="icon" onClick={() => setCollapsed((c) => !c)} aria-expanded={!collapsed} aria-label={collapsed ? 'Expand panel' : 'Collapse panel'}>
          {collapsed ? '▸' : '▾'}
        </button>
        {onClose && (
          <button className="icon" onClick={onClose} aria-label="Close panel">
            ×
          </button>
        )}
      </header>
      {!collapsed && <div className="card-body">{children}</div>}
    </section>
  )
}
