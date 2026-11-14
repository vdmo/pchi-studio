// The lines someone typed or said to the command bar while editing, kept so they can travel with the change when it is
// sent to the running show: the record then holds the words as well as the edit. Module state on purpose: the command bar
// and the send dialog are far apart in the tree.

let lines: string[] = []

/** Remember a line (the last eight are kept; repeating the same line twice in a row counts once). */
export function addSaid(text: string) {
  const t = text.replace(/\s+/g, ' ').trim()
  if (!t || lines[lines.length - 1] === t) return
  lines = [...lines, t].slice(-8)
}

export const peekSaid = () => lines.join('; ').slice(0, 300)
export const clearSaid = () => {
  lines = []
}
