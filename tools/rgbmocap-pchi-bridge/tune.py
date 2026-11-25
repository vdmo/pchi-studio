"""Offline look-ahead tuning on a simulated performer. NO camera, no real latency: you set the delay.

A known path is sampled by a tracker at --track-hz; each sample shows where the performer was --delay
seconds earlier (capture + inference + filter lag). The bridge's Tracker is driven on a fake clock and
sampled at --out-hz; error is output position minus the TRUE position at that instant.

  python tune.py --delay 0.08
Prints RMS error, worst error, and worst overshoot past a stop, for several --lead values.
Pick the lead from the column you care about; measure your real delay first (see README).
"""
import argparse
import math

from rgbmocap_pchi_bridge import Limits, Tracker


def truth(t):
    """Slow sway plus a lunge of 1 m in 0.25 s at t=2 that stops dead, and another back at t=4."""
    x = 0.5 * math.sin(1.3 * t)
    def ease(a, b, u):
        u = min(1.0, max(0.0, (t - a) / (b - a)))
        return u * u * (3 - 2 * u)
    return x + 1.0 * ease(2.0, 2.25, 0) - 1.0 * ease(4.0, 4.25, 0)


def run(lead, delay, track_hz, out_hz, horizon, seconds=6.0):
    lim = Limits(lead=lead, horizon=horizon)
    trk, errs, over = Tracker(lim), [], 0.0
    next_track, dt_track, dt_out = 0.0, 1.0 / track_hz, 1.0 / out_hz
    t = 0.0
    while t < seconds:
        while next_track <= t:
            trk.update({"confidence": 0.9, "joints": {"pelvis": [truth(next_track - delay), 0.0, 1.0]}}, next_track)
            next_track += dt_track
        if trk.t is not None:
            p, _, _ = trk.predict(t)
            e = p[0] - truth(t)
            errs.append(e)
            if 2.25 <= t <= 2.6 or 4.25 <= t <= 4.6:     # just after each stop
                over = max(over, abs(e) if (e > 0) == (truth(t) > truth(t - 0.3)) else 0.0)
        t += dt_out
    rms = math.sqrt(sum(e * e for e in errs) / len(errs))
    return rms, max(abs(e) for e in errs), over


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--delay", type=float, default=0.08)
    ap.add_argument("--track-hz", type=float, default=20.0)
    ap.add_argument("--out-hz", type=float, default=60.0)
    ap.add_argument("--horizon", type=float, default=0.07)
    a = ap.parse_args()
    print(f"delay {a.delay*1000:.0f} ms, tracker {a.track_hz:g} Hz, output {a.out_hz:g} Hz, horizon {a.horizon*1000:.0f} ms")
    print(f"{'lead ms':>8} {'rms cm':>8} {'worst cm':>9} {'overshoot cm':>13}")
    for lead in (0.0, 0.02, 0.04, 0.06, 0.08, 0.10):
        rms, worst, over = run(lead, a.delay, a.track_hz, a.out_hz, a.horizon)
        print(f"{lead*1000:8.0f} {rms*100:8.1f} {worst*100:9.1f} {over*100:13.1f}")


if __name__ == "__main__":
    main()
