import { useCallback, useRef, useState } from 'react'
import type { Showchart } from '@pchi/sdk'

/**
 * One undo history for every kind of edit (commands, dragging, deleting, form
 * fields). The chart itself stays owned by the parent; this only remembers
 * earlier values and hands them back through `onChange`.
 *
 * `commit(next, key)`: edits that share a `key` and arrive within a second of
 * each other (typing in one field) collapse into a single undo step.
 */
export function useHistory(chart: Showchart, onChange: (c: Showchart) => void) {
  const stacks = useRef<{ past: Showchart[]; future: Showchart[] }>({ past: [], future: [] })
  const current = useRef(chart)
  current.current = chart
  const last = useRef<{ key?: string; t: number }>({ t: 0 })
  const [, bump] = useState(0)

  const commit = useCallback(
    (next: Showchart, key?: string) => {
      const before = current.current
      if (next === before) return
      const now = Date.now()
      const merge = key !== undefined && last.current.key === key && now - last.current.t < 1000
      last.current = { key, t: now }
      if (!merge) {
        stacks.current.past.push(before)
        if (stacks.current.past.length > 100) stacks.current.past.shift()
      }
      stacks.current.future = []
      current.current = next
      onChange(next)
      bump((n) => n + 1)
    },
    [onChange],
  )

  const undo = useCallback(() => {
    const prev = stacks.current.past.pop()
    if (!prev) return false
    stacks.current.future.unshift(current.current)
    last.current = { t: 0 }
    current.current = prev
    onChange(prev)
    bump((n) => n + 1)
    return true
  }, [onChange])

  const redo = useCallback(() => {
    const next = stacks.current.future.shift()
    if (!next) return false
    stacks.current.past.push(current.current)
    last.current = { t: 0 }
    current.current = next
    onChange(next)
    bump((n) => n + 1)
    return true
  }, [onChange])

  /** Forget all history (after opening a different show). */
  const reset = useCallback(() => {
    stacks.current = { past: [], future: [] }
    last.current = { t: 0 }
    bump((n) => n + 1)
  }, [])

  return { commit, undo, redo, reset, canUndo: stacks.current.past.length > 0, canRedo: stacks.current.future.length > 0 }
}

export type History = ReturnType<typeof useHistory>
