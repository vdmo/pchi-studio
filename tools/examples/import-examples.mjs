// Puts the three examples into a runtime's show library (see docs/library.md).
//
//   node tools/examples/import-examples.mjs http://127.0.0.1:3101            # a runtime started with --library DIR
//   node tools/examples/import-examples.mjs http://127.0.0.1:3101 <token>    # when the runtime has a token
//
// It adds each example once: a show with the same name that is already in the library is left alone, so running it twice does not
// make duplicates. It sends no password (there are none in these files) and changes nothing else on the runtime.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const [base, token] = process.argv.slice(2)
if (!base) {
  console.error('usage: node tools/examples/import-examples.mjs <runtime url> [token]')
  process.exit(2)
}

const NOTE = 'EXAMPLE with invented content: song titles, timings, cue values, addresses and ports are made up, and it is not any real tour. Replace the placeholder addresses in the Rig tab before using it with real gear. '

const EXAMPLES = [
  {
    file: 'stadium-pop-tour.pchi',
    name: 'Stadium pop tour: full show (example)',
    venue: 'Stadium, 80,000 capacity (example)',
    planned_for: '2027-07-18T19:30',
    status: 'planned',
    visibility: 'team',
    notes:
      NOTE +
      'The scale of the biggest pop tours: 22 scenes and 200 cues over doors, countdown, intro film, three acts, a B-stage acoustic set with stage travel, an encore and the house-up. Six devices: LED walls, lighting, lasers, FOH desk, playback and stage automation. ' +
      'Lasers and the stage automation are held until a person arms them, and a PCHI stop is not an emergency stop: a real moving stage and real lasers need their own safety systems and sign-off.',
  },
  {
    file: 'arena-pop-show.pchi',
    name: 'Arena pop show: condensed (example)',
    venue: 'Arena, 15,000 capacity (example)',
    planned_for: '',
    status: 'draft',
    visibility: 'team',
    notes: NOTE + 'The same show shape at arena scale: 14 scenes and 108 cues, a smaller rig (walls, lights, FOH desk, playback), no lasers or stage automation. Use it to compare with the stadium file or as a starting point.',
  },
  {
    file: 'stadium-preshow-doors.pchi',
    name: 'Stadium doors and pre-show (example)',
    venue: 'Stadium, 80,000 capacity (example)',
    planned_for: '',
    status: 'draft',
    visibility: 'team',
    notes: NOTE + 'A day run by the clock: gates open (90 min), support act (45), changeover (30), countdown, intro film, then hand over to the main show file. Scenes move on by a timer, and the operator can skip ahead with NEXT.',
  },
]

const headers = { 'content-type': 'application/json', ...(token ? { authorization: `Bearer ${token}` } : {}) }
const get = async (path) => (await fetch(base + path, { headers })).json()

const have = new Set(((await get('/library')).shows ?? []).map((s) => s.name))
for (const e of EXAMPLES) {
  if (have.has(e.name)) {
    console.log(`already there: ${e.name}`)
    continue
  }
  const { file, ...meta } = e
  const r = await fetch(base + '/library', { method: 'POST', headers, body: JSON.stringify({ ...meta, show: JSON.parse(readFileSync(join(here, file), 'utf8')) }) })
  const body = await r.json().catch(() => ({}))
  console.log(r.status === 201 ? `added: ${e.name} (${body.meta?.id})` : `FAILED ${r.status}: ${e.name}: ${body.error ?? JSON.stringify(body)}`)
  if (r.status !== 201) process.exitCode = 1
}
