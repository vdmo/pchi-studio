import { transitionLabel, type Showchart, type ShowTransition } from '@pchi/sdk'

/** What the show does around one scene (state): how it is entered and left, and when it leaves by itself. */
export function sceneInfo(chart: Showchart, id: string) {
  const into = chart.transitions.filter((t) => t.target === id && t.source !== id)
  const out = chart.transitions.filter((t) => t.source === id && t.target !== id)
  const timed = out.filter((t) => t.after_ms !== undefined).sort((a, b) => a.after_ms! - b.after_ms!)
  return {
    into,
    out,
    /** the earliest timer that moves the show on: the scene's natural length */
    exit: timed[0] ? { ms: timed[0].after_ms!, target: timed[0].target, guard: timed[0].guard } : undefined,
    describe: (t: ShowTransition) => transitionLabel(t),
  }
}
