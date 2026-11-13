// Brief → show draft. Turns a plain description of a show into command
// sentences (the same ones a person would type), so the draft can be read and
// edited before anything is built. It is a small, deterministic parser, not an
// AI: it understands one scene per line (or sentence), an optional length, and
// a few department words. What it did not understand is reported, never guessed.

export interface BriefDraft {
  /** Command sentences joined for runCommandText, in order. */
  commands: string[]
  /** Scenes found, in order. */
  scenes: { id: string; seconds?: number; cues: { dept: string; value: string }[] }[]
  /** Anything left over or assumed, in words. */
  notes: string[]
}

/** What people call the departments out loud (also in live.ts and commands.ts). */
const DEPT_WORDS: Record<string, string> = {
  visual: 'visual', visuals: 'visual', vj: 'visual', video: 'visual',
  lighting: 'lighting', lights: 'lighting', light: 'lighting',
  laser: 'laser', lasers: 'laser',
  audio: 'audio', sound: 'audio',
}

const NUM_WORDS: Record<string, number> = { one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7, eight: 8, nine: 9, ten: 10, fifteen: 15, twenty: 20, thirty: 30, forty: 40, sixty: 60 }
const COLOURS = new Set(['red', 'green', 'blue', 'white', 'amber', 'purple', 'pink', 'orange', 'yellow', 'cyan', 'magenta', 'warm', 'cold', 'cool', 'dark', 'black'])
const DEPT_LABEL: Record<string, { cmd: string; field: string }> = {
  visual: { cmd: 'activate scene', field: 'scene' },
  lighting: { cmd: 'lighting look', field: 'look' },
  laser: { cmd: 'laser cue', field: 'cue' },
  audio: { cmd: 'audio snapshot', field: 'snapshot' },
}
const STOP = new Set(['a', 'an', 'the', 'and', 'then', 'with', 'for', 'of', 'big', 'some', 'small', 'short', 'long', 'final', 'first', 'next', 'last', 'lasting', 'about', 'around', 'roughly', 'at'])
const SCENE_STOP = new Set(['with', 'for', 'lasting', 'lasts', 'about', 'around', 'roughly', 'of', 'where', 'plus', 'featuring'])

const toSeconds = (n: number, unit: string) => (/^m/i.test(unit) ? n * 60 : n)
const num = (w: string) => (/^\d+(\.\d+)?$/.test(w) ? Number(w) : NUM_WORDS[w.toLowerCase()])

/** Break a brief into one chunk per scene: lines, sentences, semicolons, arrows and "then". */
function chunks(text: string): string[] {
  return text
    .replace(/\r/g, '')
    .split(/\n|;|\.(?=\s|$)|→|->|=>|\bthen\b|\band then\b/i)
    .map((c) => c.replace(/^[\s\-*•\d.)]+/, '').trim())
    .filter(Boolean)
}

export function briefToCommands(brief: string): BriefDraft {
  const draft: BriefDraft = { commands: [], scenes: [], notes: [] }
  const used = new Set<string>()
  for (const raw of chunks(brief)) {
    const words = raw.replace(/[,()]/g, ' ').split(/\s+/).filter(Boolean)
    // 1. the scene name: leading words until a keyword or a number
    const name: string[] = []
    let i = 0
    for (; i < words.length; i++) {
      const w = words[i].toLowerCase()
      if (num(w) !== undefined || SCENE_STOP.has(w) || DEPT_WORDS[w] !== undefined || COLOURS.has(w)) break
      if (STOP.has(w) && name.length === 0) continue // a leading "a" or "the"
      if (STOP.has(w)) break
      name.push(words[i])
      if (name.length >= 3) {
        i++
        break
      }
    }
    if (name.length === 0) {
      draft.notes.push(`"${raw}": could not find a scene name, skipped`)
      continue
    }
    let id = name.join(' ').toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '')
    if (used.has(id)) {
      let n = 2
      while (used.has(`${id}_${n}`)) n++
      draft.notes.push(`"${id}" appears twice: the second one is called ${id}_${n}`)
      id = `${id}_${n}`
    }
    used.add(id)

    // 2. a length ("30 seconds", "2 min", "for ten seconds")
    let seconds: number | undefined
    for (let j = i; j < words.length - 1; j++) {
      const n = num(words[j])
      if (n !== undefined && /^(s|sec|secs|second|seconds|m|min|mins|minute|minutes)$/i.test(words[j + 1])) {
        seconds = toSeconds(n, words[j + 1])
        break
      }
    }

    // 3. department words, each optionally followed by a value ("blue lights", "lasers sweep", "visuals tunnel")
    const cues: { dept: string; value: string }[] = []
    const rest = words.slice(i)
    for (let j = 0; j < rest.length; j++) {
      const w = rest[j].toLowerCase()
      const dept = DEPT_WORDS[w]
      if (!dept || !DEPT_LABEL[dept]) continue
      // a colour just before the word ("blue lights") or a plain word just after ("lasers sweep")
      const before = rest[j - 1]?.toLowerCase()
      const after = rest[j + 1]?.toLowerCase()
      let value = ''
      if (before && COLOURS.has(before)) value = before
      else if (after && !STOP.has(after) && num(after) === undefined && DEPT_WORDS[after] === undefined && !SCENE_STOP.has(after)) value = after
      if (!cues.some((c) => c.dept === dept)) cues.push({ dept, value })
    }
    draft.scenes.push({ id, seconds, cues })
  }

  if (draft.scenes.length === 0) {
    draft.notes.push('No scenes found. Write one scene per line, e.g. "Intro, 30 seconds, blue lights".')
    return draft
  }
  const ids = draft.scenes.map((s) => s.id)
  draft.commands.push(`chain ${ids.join(', ')}`)
  draft.scenes.forEach((s, k) => {
    const next = draft.scenes[k + 1]
    if (s.seconds !== undefined) draft.commands.push(next ? `from ${s.id} to ${next.id} after ${s.seconds} seconds` : `set ${s.id} length to ${s.seconds} seconds`)
    for (const c of s.cues) {
      const d = DEPT_LABEL[c.dept]
      draft.commands.push(`at 0 seconds ${d.cmd}${c.value ? ` ${c.value}` : ''} in ${s.id}`)
      if (!c.value) draft.notes.push(`${s.id}: a ${c.dept} cue was added at 0 s with no value: name it in the Cue sheet`)
    }
  })
  return draft
}

/** The draft as one string for the command bar ("a; b; c"). */
export const draftText = (d: BriefDraft) => d.commands.join('; ')
