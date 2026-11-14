import { useEffect, useState } from 'react'
import { feedToken } from './liveFeed'

type Role = { role: 'active' | 'standby' | 'fenced'; epoch?: number; fenced_by?: { by: string; epoch: number } | null; following?: string; copied_records?: number; last_heard_ms_ago?: number | null; problem?: string | null; stuck?: boolean; takeover?: { mode: 'auto' | 'manual'; after_s?: number }; name?: string }

/**
 * Says so when this is not the ordinary single runtime: a standby that is waiting to take over, a runtime that was stopped because a standby took
 * over (do not press anything here), or one running on a takeover. Says nothing for the usual case.
 */
export default function RoleBanner() {
  const [r, setR] = useState<Role | null>(null)
  useEffect(() => {
    let live = true
    const tick = async () => {
      try {
        const t = feedToken()
        const res = await fetch('/api/role', { headers: t ? { Authorization: `Bearer ${t}` } : {} })
        if (live && res.ok) setR(await res.json())
      } catch {
        /* the runtime is not there: the feed pill already says so */
      }
    }
    tick()
    const id = setInterval(tick, 3000)
    return () => {
      live = false
      clearInterval(id)
    }
  }, [])

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
}
