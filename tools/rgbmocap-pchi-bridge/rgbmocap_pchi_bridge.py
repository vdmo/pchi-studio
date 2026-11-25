#!/usr/bin/env python3
"""RGBMoCap -> PCHI bridge (prototype, standard library only).

Input : UDP JSON frames on 127.0.0.1:9100 from a tap in toyxyz_RGBMocap's live (PEAR) worker:
        {"confidence": 0..1, "joints": {"pelvis": [x,y,z], ...}}
        Metres, stage-relative, z up. (The tap does not exist yet; see README.)
Output: PCHI v2.0 `artist_tracking` messages (prime-market/pchi-schema shape) as UDP JSON to the
        Conductor (default 127.0.0.1:8888), at a fixed rate, independent of the tracker's rate.

Two loops: the receiver stores the latest tracked state; the emitter ticks at --rate Hz and sends a
dead-reckoned position with a capped horizon, velocity decay on stale frames, a slew limiter and a
stage-boundary clamp. These clamps smooth the signal. They are NOT laser/DMX safety: keep hardware
interlocks and scan-fail zones independent of this process.
"""
import argparse
import json
import math
import socket
import threading
import time

SOURCE_ID = "rgbmocap_bridge"
ARTIST_ID = "performer_01"


class Limits:
    def __init__(self, radius=1.5, zmin=0.0, zmax=2.8, slew=6.0, min_conf=0.45,
                 horizon=0.07, lead=0.03, decay=0.85):
        self.radius, self.zmin, self.zmax, self.slew = radius, zmin, zmax, slew
        self.min_conf, self.horizon, self.lead, self.decay = min_conf, horizon, lead, decay


def clamp_stage(p, lim):
    """Cylinder clamp. Returns (point, was_clamped). Callers reject non-finite input first."""
    x, y, z = p
    clamped = False
    r = math.hypot(x, y)
    if r > lim.radius:
        s = lim.radius / r
        x, y, clamped = x * s, y * s, True
    z2 = min(lim.zmax, max(lim.zmin, z))
    return (x, y, z2), clamped or z2 != z


def slew_limit(prev, cur, dt, max_speed):
    d = math.dist(prev, cur)
    step = max_speed * max(dt, 1e-3)
    if d <= step or d < 1e-9:
        return cur
    k = step / d
    return tuple(a + (b - a) * k for a, b in zip(prev, cur))


class Tracker:
    """Latest accepted state (position, velocity, time). Callers serialise access."""

    def __init__(self, lim):
        self.lim = lim
        self.pos = (0.0, 0.0, 1.0)
        self.vel = (0.0, 0.0, 0.0)
        self.t = None
        self.conf = 0.0
        self.clamped = False
        self.rejected = 0
        self.wrists = None     # (left, right) from the last accepted frame that had both

    def update(self, frame, now):
        try:
            conf = float(frame.get("confidence", 0.0))
            if not math.isfinite(conf):
                raise ValueError
            self.conf = max(0.0, min(1.0, conf))
            if self.conf < self.lim.min_conf:
                return  # occluded or person lost: keep the last good state; predict() coasts it to a stop
            raw = tuple(float(v) for v in frame["joints"]["pelvis"])
        except (KeyError, TypeError, ValueError, AttributeError):
            self.rejected += 1
            return
        if len(raw) != 3 or not all(map(math.isfinite, raw)):
            self.rejected += 1
            return
        if self.t is None:
            self.pos, self.vel, self.t, self.clamped = clamp_stage(raw, self.lim)[0], (0.0, 0.0, 0.0), now, False
            self.wrists = self._wrists(frame)
            return
        dt = max(now - self.t, 1e-3)
        limited = slew_limit(self.pos, raw, dt, self.lim.slew)
        new, clamped = clamp_stage(limited, self.lim)
        self.vel = tuple((n - o) / dt for n, o in zip(new, self.pos))
        self.pos, self.t, self.clamped = new, now, clamped or limited != raw
        self.wrists = self._wrists(frame)

    def lost(self, now, after):
        """True when no usable frame has been accepted for `after` seconds (or ever). Low-confidence frames do
        not count as usable, so a long occlusion also becomes 'lost'."""
        return self.t is None or now - self.t > after

    def _wrists(self, frame):
        """Both wrists as finite 3-tuples, or None (then the expressive values simply hold)."""
        try:
            j = frame["joints"]
            lw = tuple(float(v) for v in j["left_wrist"])
            rw = tuple(float(v) for v in j["right_wrist"])
        except (KeyError, TypeError, ValueError):
            return None
        if len(lw) != 3 or len(rw) != 3 or not all(map(math.isfinite, lw + rw)):
            return None
        return lw, rw

    def predict(self, now):
        """Dead-reckoned (position, velocity, clamped) at `now`."""
        if self.t is None:
            return self.pos, self.vel, False
        age = now - self.t
        h = min(age, self.lim.horizon)
        v = self.vel
        if age > self.lim.horizon or self.conf < self.lim.min_conf:
            f = self.lim.decay ** max(1.0, (age - self.lim.horizon) * 60)
            v = tuple(c * f for c in v)
        p = tuple(a + c * (h + self.lim.lead) for a, c in zip(self.pos, v))
        p, clamped = clamp_stage(p, self.lim)
        return p, v, clamped or self.clamped


def ptm_invariants(window):
    """Invariants block for a tracking message.

    A single performer position has no set of values built to balance, so there is no equilibrium to
    report: residual is 0 and the sign sequence is the default. (The pchi-studio Conductor makes the same
    point: it only checks residual on scene_update and warns on any non-zero residual otherwise.)
    The descriptive fields are real: coherence_gap is the variance of recent x, curvature_signature the
    second differences. Nothing in this block is a safety check."""
    vals = list(window)[-4:] or [0.0]
    mean = sum(vals) / len(vals)
    gap = sum((v - mean) ** 2 for v in vals) / len(vals)
    curv = [abs(vals[i + 1] - 2 * vals[i] + vals[i - 1]) for i in range(1, len(vals) - 1)] or [0.0]
    return {
        "equilibrium_check": {"residual": 0.0, "precision": 1e-12, "sign_sequence": [1, -1, -1, 1]},
        "coherence_gap": gap,
        "curvature_signature": ", ".join(f"{c:.6f}" for c in curv),
        "residual": 0.0,
    }


def expressive(trk, vel):
    """0..1 values for the show: wingspan, highest hand, whole-body speed. None until both wrists were seen."""
    if trk.wrists is None:
        return None
    lw, rw = trk.wrists
    return {
        "spread": min(1.0, math.dist(lw, rw) / 1.8),
        "hand_elevation": min(1.0, max(0.0, (max(lw[2], rw[2]) - 0.5) / 1.7)),
        "energy": min(1.0, math.sqrt(sum(c * c for c in vel)) / 5.0),
    }


def control_message(parameter, value):
    m = pchi_message((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), [])
    m["type"] = "control_parameter"
    m["payload"] = {"payloadType": "control_parameter", "data": {
        "target_id": ARTIST_ID, "parameter": parameter, "value": round(value, 4)}}
    return m


def pchi_message(pos, vel, window):
    return {
        "version": "2.0.0",
        "type": "artist_tracking",
        "source_id": SOURCE_ID,
        "timestamp": time.time() * 1000.0,
        "pir_invariants": ptm_invariants(window),
        "payload": {"payloadType": "artist_tracking", "data": {
            "artist_id": ARTIST_ID,
            "transform": {"position": list(pos), "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]},
            "velocity": list(vel)}},
    }


def main():
    ap = argparse.ArgumentParser(description="RGBMoCap -> PCHI artist_tracking bridge")
    ap.add_argument("--listen", default="127.0.0.1:9100")
    ap.add_argument("--pchi", default="127.0.0.1:8888")
    ap.add_argument("--rate", type=float, default=60.0)
    ap.add_argument("--radius", type=float, default=1.5)
    ap.add_argument("--slew", type=float, default=6.0, help="max m/s the tracked point may move")
    ap.add_argument("--param-rate", type=float, default=20.0, help="Hz for spread/hand_elevation/energy; 0 = off")
    ap.add_argument("--lost-after", type=float, default=2.0, help="seconds without a usable frame before position output stops")
    ap.add_argument("--lead", type=float, default=0.03, help="seconds of look-ahead")
    a = ap.parse_args()

    trk, lock = Tracker(Limits(radius=a.radius, slew=a.slew, lead=a.lead)), threading.Lock()
    rx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    h, p = a.listen.rsplit(":", 1)
    rx.bind((h, int(p)))
    host, port = a.pchi.rsplit(":", 1)
    tx, dest = socket.socket(socket.AF_INET, socket.SOCK_DGRAM), (host, int(port))

    def receive():
        while True:
            data, _ = rx.recvfrom(65535)
            try:
                frame = json.loads(data)
            except ValueError:
                continue
            with lock:
                trk.update(frame, time.monotonic())

    threading.Thread(target=receive, daemon=True).start()
    window, period, nxt = [], 1.0 / a.rate, time.monotonic()
    next_params, last_tracking, tracking_at = nxt, None, 0.0
    print(f"listening {a.listen}; sending artist_tracking to {a.pchi} at {a.rate:g} Hz")
    while True:
        nxt += period
        time.sleep(max(0.0, nxt - time.monotonic()))
        with lock:
            now = time.monotonic()
            pos, vel, _ = trk.predict(now)
            extra = expressive(trk, vel)
            lost = trk.lost(now, a.lost_after)
        live = 0.0 if lost else 1.0
        # Say so when tracking starts or stops, and repeat once a second so a late listener learns it.
        if live != last_tracking or time.monotonic() - tracking_at >= 1.0:
            last_tracking, tracking_at = live, time.monotonic()
            tx.sendto(json.dumps(control_message("rgbmocap.tracking", live)).encode(), dest)
        if lost:
            continue  # never keep steering the show with a position nobody is tracking any more
        window.append(pos[0])
        del window[:-8]
        tx.sendto(json.dumps(pchi_message(pos, vel, window)).encode(), dest)
        if a.param_rate > 0 and extra and time.monotonic() >= next_params:
            next_params = time.monotonic() + 1.0 / a.param_rate
            for name, value in extra.items():
                tx.sendto(json.dumps(control_message(f"rgbmocap.{name}", value)).encode(), dest)


if __name__ == "__main__":
    main()
