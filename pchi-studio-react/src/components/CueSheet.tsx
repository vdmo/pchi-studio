import { useMemo, useState } from 'react'
import { DEPTS, buildCueSheet, cueSheetCsv, cueTime, deptOf, duplicateCueNumbers, laneDevice, removeCue, updateCue, type PchiNode, type Showchart } from '@pchi/sdk'
import type { History } from './useHistory'

interface Props {
  chart: Showchart
  history: History
  scene: string | null
  onScene: (id: string | null) => void
  onOpenTimeline: () => void
  nodes: PchiNode[]
}

const KIND_NAME: Record<string, string> = {
  'scene.activate': 'Scene',
  'renderer.activate': 'Renderer',
  'lighting.look': 'Look',
  'laser.cue': 'Laser cue',
  'audio.snapshot': 'Snapshot',
  'audio.scene': 'Audio scene',
  'cue.trigger': 'Cue',
}

/**
 * The cue sheet: every action in the show in calling order, grouped by scene,
 * the way a stage manager reads it. It is another view of the same showchart:
 * edits here change the chart (and join the same undo history).
 */
export default function CueSheet({ chart, history, scene, onScene, onOpenTimeline, nodes }: Props) {
  const { commit } = history
  const [dept, setDept] = useState<string | null>(null)
  const [q, setQ] = useState('')
  const blocks = useMemo(() => buildCueSheet(chart), [chart])
  const dups = useMemo(() => duplicateCueNumbers(chart), [chart])
  const total = blocks.reduce((n, b) => n + b.rows.length, 0)

  const match = (r: { dept: string; label: string; kind: string; value: string; laneName: string }) => {
    if (dept && r.dept !== dept) return false
    const t = q.trim().toLowerCase()
    return !t || [r.label, r.kind, r.value, r.laneName].some((x) => x.toLowerCase().includes(t))
  }

  const download = () => {
    const blob = new Blob([cueSheetCsv(chart)], { type: 'text/csv;charset=utf-8' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${chart.name ?? chart.id ?? 'show'}-cue-sheet.csv`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  if (chart.states.length === 0) {
    return (
      <div className="cv">
        <div className="cv-canvas tl-canvas">
          <div className="tl-empty">
            <h3>No scenes yet</h3>
            <p>The cue sheet lists what happens in each scene. Create scenes on the <b>Show</b> tab first.</p>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="cv">
      <div className="cv-canvas cs">
        <div className="cs-bar">
          <div className="cs-title">
            <b>{chart.name ?? chart.id ?? 'Show'}</b>
            <span className="muted">
              {total} {total === 1 ? 'cue' : 'cues'} in {blocks.length} {blocks.length === 1 ? 'scene' : 'scenes'}
            </span>
          </div>
          <span className="spacer" />
          <div className="cs-filters" role="group" aria-label="Filter by department">
            <button className={dept === null ? 'on' : ''} onClick={() => setDept(null)} aria-pressed={dept === null}>All</button>
            {DEPTS.filter((d) => d.id !== 'other').map((d) => (
              <button key={d.id} className={dept === d.id ? 'on' : ''} onClick={() => setDept(dept === d.id ? null : d.id)} aria-pressed={dept === d.id}>
                <i style={{ background: d.color }} aria-hidden /> {d.label}
              </button>
            ))}
          </div>
          <input className="cs-search" type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search cues" aria-label="Search cues" />
          <button onClick={download} title="Download as a spreadsheet (CSV)">CSV</button>
          <button onClick={() => window.print()} title="Print the cue sheet">Print</button>
        </div>

        <div className="cs-scroll">
          <table className="cs-table">
            <thead>
              <tr>
                <th className="c-q">Q</th>
                <th className="c-t">Time</th>
                <th className="c-d">Dept</th>
                <th className="c-l">Lane</th>
                <th className="c-dev">Device</th>
                <th className="c-a">Action</th>
                <th className="c-v">Value</th>
                <th className="c-n">Note</th>
                <th className="c-x" aria-label="Remove" />
              </tr>
            </thead>
            {blocks.map((b) => {
              const rows = b.rows.filter(match)
              if ((dept || q.trim()) && rows.length === 0) return null
              return (
                <tbody key={b.id} className={scene === b.id ? 'cs-on' : ''}>
                  <tr className="cs-scene" onClick={() => onScene(b.id)}>
                    <td colSpan={9}>
                      <b>{b.id}</b>
                      {b.label && <span className="muted"> · {b.label}</span>}
                      <span className="cs-leaves">{b.leaves.length ? `leaves: ${b.leaves.join(' · ')}` : 'the show ends here'}</span>
                      <button
                        className="cs-edit"
                        onClick={(e) => {
                          e.stopPropagation()
                          onScene(b.id)
                          onOpenTimeline()
                        }}
                      >
                        Edit timeline
                      </button>
                    </td>
                  </tr>
                  {rows.length === 0 && (
                    <tr className="cs-none">
                      <td colSpan={9} className="muted">Nothing happens in this scene yet.</td>
                    </tr>
                  )}
                  {rows.map((r, i) => {
                    const d = deptOf(r.dept)
                    const editable = r.source === 'cue' && r.cueId !== undefined
                    const key = (f: string) => `cs:${r.scene}:${r.cueId}:${f}`
                    return (
                      <tr key={`${r.scene}-${r.cueId ?? `${r.source}-${i}`}`} className={`cs-row ${r.source}`} style={{ ['--dept' as string]: d.color }}>
                        <td className="c-q">
                          {editable ? (
                            <label className={dups.has(r.num ?? -1) ? 'dup' : ''} title={dups.has(r.num ?? -1) ? 'Another cue has this number' : 'Cue number: kept when cues are added, moved or deleted. Use 12.5 to slot one in.'}>
                              Q
                              <input
                                type="number"
                                min={0.001}
                                step={0.5}
                                value={r.num ?? ''}
                                aria-label={`Number of the cue at ${cueTime(r.at_ms)} in ${r.scene}`}
                                onChange={(e) => e.target.value !== '' && Number(e.target.value) > 0 && commit(updateCue(chart, r.scene, r.cueId!, { num: Number(e.target.value) }), key('q'))}
                              />
                            </label>
                          ) : (
                            <span className="muted">–</span>
                          )}
                        </td>
                        <td className="c-t">
                          {editable ? (
                            <label title="Seconds after the scene starts">
                              <input
                                type="number"
                                min={0}
                                step={0.25}
                                value={(r.at_ms ?? 0) / 1000}
                                aria-label={`Time of Q${r.num ?? ''} in seconds`}
                                onChange={(e) => e.target.value !== '' && commit(updateCue(chart, r.scene, r.cueId!, { at_ms: Number(e.target.value) * 1000 }), key('t'))}
                              />
                              <small>{cueTime(r.at_ms)}</small>
                            </label>
                          ) : (
                            <span className="muted">{r.source === 'enter' ? 'on enter' : 'on leave'}</span>
                          )}
                        </td>
                        <td className="c-d">
                          <span className="cs-dept"><i style={{ background: d.color }} aria-hidden />{d.label}</span>
                        </td>
                        <td className="c-l">{r.laneName}</td>
                        <td className="c-dev">
                          {r.source !== 'cue' ? (
                            <span className="muted">–</span>
                          ) : (
                            (() => {
                              const dev = laneDevice(nodes, { dept: r.dept, device: r.device })
                              if (dev) return dev.label
                              return <span className="warn" title={r.device ? 'The device was removed from the rig' : 'No single device handles this lane. Set one in the Timeline.'}>⚠ {r.device ? 'removed' : 'none'}</span>
                            })()
                          )}
                        </td>
                        <td className="c-a" title={r.kind}>{KIND_NAME[r.kind] ?? r.kind}</td>
                        <td className="c-v">
                          {editable ? (
                            <input value={r.value} aria-label={`Value of Q${r.num ?? ''}`} placeholder="(blank)" onChange={(e) => commit(updateCue(chart, r.scene, r.cueId!, { value: e.target.value }), key('v'))} />
                          ) : (
                            r.value
                          )}
                        </td>
                        <td className="c-n">
                          {editable ? (
                            <input value={r.label} aria-label={`Note for Q${r.num ?? ''}`} placeholder="note or cue name" onChange={(e) => commit(updateCue(chart, r.scene, r.cueId!, { label: e.target.value }), key('n'))} />
                          ) : (
                            <span className="muted">set in the Show tab</span>
                          )}
                        </td>
                        <td className="c-x">
                          {editable && (
                            <button className="x" aria-label={`Delete Q${r.num ?? ''}`} title="Delete this cue (Ctrl+Z undoes)" onClick={() => commit(removeCue(chart, r.scene, r.cueId!))}>×</button>
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              )
            })}
          </table>
          {total === 0 && <p className="muted pad">No cues yet. Add them on the Timeline tab, or type "at 4 seconds lighting look blue in intro".</p>}
        </div>
      </div>
    </div>
  )
}
