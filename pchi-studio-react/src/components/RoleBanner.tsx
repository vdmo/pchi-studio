import { useEffect, useState } from 'react'
import { chooseRuntime, type RoleInfo } from '@pchi/sdk'
import { apiBase, backupBase, feedToken, followRuntime, runtimeWhich, setBackupBase, useRuntimeWhich } from './liveFeed'

type Role = { role: 'active' | 'standby' | 'fenced'; epoch?: number; fenced_by?: { by: string; epoch: number } | null; following?: string; copied_records?: number; last_heard_ms_ago?: number | null; problem?: string | null; stuck?: boolean; takeover?: { mode: 'auto' | 'manual'; after_s?: number }; name?: string }

async function roleAt(base: string): Promise<Role | null> {
  try {
    const t = feedToken()
    const res = await fetch(`${base}/role`, { headers: t ? { Authorization: `Bearer ${t}` } : {} })
    return res.ok ? await res.json() : null
  } catch {
    return null
  }
}

/**
 * Says so when this is not the ordinary single runtime: a standby that is waiting to take over, a runtime that was stopped because a standby took
 * over (do not press anything here), or one running on a takeover. Says nothing for the usual case. With a backup address set, it also watches both
 * runtimes and moves Studio to the one that is running the show.
 */
export default function RoleBanner() {
  const which = useRuntimeWhich()
  const [r, setR] = useState<Role | null>(null)
  const [backupText, setBackupText] = useState(() => backupBase())
  const [backupErr, setBackupErr] = useState('')
  const [moved, setMoved] = useState<string>('')
  const [tick, setTick] = useState(0) // bumped when the backup address changes

  useEffect(() => {
    let live = true
    const poll = async () => {
      const backup = backupBase()
      const [p, b] = await Promise.all([roleAt('/api'), backup ? roleAt(backup) : Promise.resolve(null)])
      if (!live) return
      const before = runtimeWhich()
      const next = chooseRuntime(before, p as RoleInfo, b as RoleInfo, !!backup)
      if (next !== before) {
        followRuntime(next)
        setMoved(next === 'backup' ? `Studio moved to the backup runtime (${backup}): the primary is not running the show. You are signed out; sign in again.` : 'Studio is back on the primary runtime. You are signed out; sign in again.')
      }
      setR(next === 'backup' ? b : p)
    }
    poll()
    const id = setInterval(poll, 2000)
    return () => {
      live = false
      clearInterval(id)
    }
  }, [tick])

  const saveBackup = () => {
    const err = setBackupBase(backupText)
    setBackupErr(err)
    if (!err) {
      setBackupText(backupBase())
      setMoved('')
      setTick((t) => t + 1)
    }
  }

  const note = (() => {
    if (!r) return null
    if (r.role === 'fenced') {
      return (
        <p className="role-banner fenced" role="alert">
          <b>This runtime has been stopped.</b> {r.fenced_by?.by ?? 'Another runtime'} took over the show (takeover {r.fenced_by?.epoch}). Nothing here sends cues, streams lighting or accepts inputs any more: operate the show on that runtime.
        </p>
      )
    }
    if (r.role === 'standby') {
      const heard = r.last_heard_ms_ago == null ? 'has not heard from the primary yet' : `heard from the primary ${(r.last_heard_ms_ago / 1000).toFixed(0)} s ago`
      return (
        <p className={`role-banner standby ${r.stuck ? 'bad' : ''}`} role="status">
          <b>Standby.</b> Following {r.following}: {r.copied_records ?? 0} records copied, {heard}.{' '}
          {r.takeover?.mode === 'auto' ? `Takes over by itself after ${r.takeover.after_s} s of silence.` : 'Takes over only when told (POST /takeover).'}
          {r.problem ? ` ${r.problem}.` : ''}
        </p>
      )
    }
    if ((r.epoch ?? 0) > 0) {
      return (
        <p className="role-banner took" role="status">
          <b>Running as takeover {r.epoch}.</b> This runtime took over from another; lasers were left disarmed and departments need to report READY again.
        </p>
      )
    }
    return null
  })()

  return (
    <>
      {moved && <p className="role-banner took" role="alert">{moved}</p>}
      {which === 'backup' && !moved && <p className="role-banner took" role="status">Studio is talking to the backup runtime at {backupBase()}.</p>}
      {note}
      <details className="backup">
        <summary>Backup runtime{backupBase() ? `: ${backupBase()}` : ''}</summary>
        <form onSubmit={(e) => { e.preventDefault(); saveBackup() }}>
          <input value={backupText} onChange={(e) => setBackupText(e.target.value)} placeholder="10.0.0.6:3100" aria-label="Backup runtime address" spellCheck={false} />
          <button type="submit">Save</button>
        </form>
        <p className="muted">If the runtime Studio talks to stops being the one running the show and this one is, Studio moves to it. The backup must be started with <code>--allow-origin</code> for this page's address. Reading from {apiBase()}.</p>
        {backupErr && <p className="rt-err" role="alert">{backupErr}</p>}
      </details>
    </>
  )
}
