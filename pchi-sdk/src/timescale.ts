// Time on a timeline: how it is written, read, snapped, nudged and ruled. Pure functions, shared by every place a timeline is drawn.

/**
 * A time as `m:ss.s`, with exactly as many decimals as the value needs (at least one), so what is shown is what is stored:
 * 12 250 ms reads `0:12.25`, never a rounded `0:12.3`.
 */
export function fmtTime(ms: number): string {
  const whole = Math.max(0, Math.round(ms))
  const m = Math.floor(whole / 60000)
  const rest = whole % 60000
  const s = Math.floor(rest / 1000)
  const frac = rest % 1000
  const f = frac === 0 ? '0' : frac % 100 === 0 ? String(frac / 100) : frac % 10 === 0 ? String(frac / 10).padStart(2, '0') : String(frac).padStart(3, '0')
  return `${m}:${String(s).padStart(2, '0')}.${f}`
}

/** A ruler label: whole seconds when the step is a second or more (`0:12`), else the exact time. */
export const fmtTick = (ms: number, stepMs: number): string => (stepMs >= 1000 ? fmtTime(ms).replace(/\.\d+$/, '') : fmtTime(ms))

/**
 * Read a typed time. Accepts seconds (`12`, `12.5`, `12,5`, `12.5s`) or minutes and seconds (`0:12.5`, `1:02`).
 * Returns whole milliseconds, or null if it is not a time (negative, empty, seconds of 60 or more after a minute).
 */
export function parseTime(text: string): number | null {
  const m = /^(?:(\d+):)?(\d+(?:[.,]\d+)?)\s*s?$/i.exec(text.trim())
  if (!m) return null
  const secs = Number(m[2].replace(',', '.'))
  if (m[1] !== undefined && secs >= 60) return null
  const ms = Math.round(((m[1] !== undefined ? Number(m[1]) * 60 : 0) + secs) * 1000)
  return Number.isFinite(ms) ? ms : null
}

/** Snap to a grid of `stepMs` (0 or less: no grid, just whole milliseconds). Never below zero. */
export const snapMs = (ms: number, stepMs: number): number => Math.max(0, stepMs > 0 ? Math.round(ms / stepMs) * stepMs : Math.round(ms))

/**
 * One keyboard nudge. With a grid, the cue goes to the next grid line in that direction (so a cue that is off the grid lands
 * on it instead of staying off by the same amount); with no grid it moves 100 ms. `big` moves by a second. Never below zero.
 */
export function nudgeMs(ms: number, dir: -1 | 1, stepMs: number, big = false): number {
  if (!big && stepMs <= 0) return Math.max(0, Math.round(ms) + dir * 100)
  const g = big ? 1000 : stepMs
  const q = ms / g
  const next = dir > 0 ? Math.floor(q + 1e-9) + 1 : Math.ceil(q - 1e-9) - 1
  return Math.max(0, Math.round(next * g))
}

export interface Tick {
  ms: number
  /** Labelled (a major tick); the others are unlabelled marks between them. */
  major: boolean
}

// [major step in ms, minor step in ms]
const STEPS: [number, number][] = [
  [500, 100], [1000, 250], [2000, 500], [5000, 1000], [10000, 2000], [15000, 5000], [30000, 5000],
  [60000, 10000], [120000, 30000], [300000, 60000], [600000, 60000], [1800000, 300000], [3600000, 600000],
]
/** Space a label needs, in pixels. */
const LABEL_PX = 64
/** Smallest gap between two minor marks that is still worth drawing. */
const MINOR_PX = 7

/** The marks for a ruler `totalMs` long at `pps` pixels per second: labels never closer than a label's width, minor marks only if they fit. */
export function rulerTicks(totalMs: number, pps: number): { ticks: Tick[]; majorMs: number } {
  const perMs = Math.max(pps, 0.0001) / 1000
  const [major, minor] = STEPS.find(([m]) => m * perMs >= LABEL_PX) ?? STEPS[STEPS.length - 1]
  const drawMinor = minor * perMs >= MINOR_PX
  const step = drawMinor ? minor : major
  const ticks: Tick[] = []
  for (let ms = 0; ms <= totalMs + 1e-6; ms += step) ticks.push({ ms, major: ms % major === 0 })
  return { ticks, majorMs: major }
}
