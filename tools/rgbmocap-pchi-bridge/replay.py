"""Feed the bridge from a recording (or a built-in synthetic performance) so limits can be tuned with no camera.

  python replay.py --synthetic           # sway, arm raises, a glitch, an occlusion
  python replay.py recording.jsonl       # lines of {"t": seconds, "confidence": .., "joints": {...}}

Sends to the bridge's input port at the recorded timing. A recording can be captured by pointing
nc/socat or a few lines of Python at the tap's port.
"""
import argparse
import json
import math
import socket
import sys
import time


def synthetic(seconds=8.0, hz=20.0):
    n = int(seconds * hz)
    for i in range(n):
        t = i / hz
        x = 0.8 * math.sin(t * 1.3)
        raise_arms = 0.5 + 0.5 * math.sin(t * 2.0)
        wz = 0.9 + 1.0 * raise_arms
        f = {"t": t, "confidence": 1.0, "joints": {
            "pelvis": [x, 0.2 * math.cos(t), 1.0], "head": [x, 0.2 * math.cos(t), 1.6],
            "left_wrist": [x - 0.6, 0.2, wz], "right_wrist": [x + 0.6, 0.2, wz]}}
        if 3.0 <= t < 3.05:   # single-frame tracker glitch far off stage
            f["joints"]["pelvis"] = [9.0, 9.0, 4.0]
        if 5.0 <= t < 5.8:    # performer lost behind the booth
            f = {"t": t, "confidence": 0.0, "joints": {}}
        yield f


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file", nargs="?")
    ap.add_argument("--synthetic", action="store_true")
    ap.add_argument("--to", default="127.0.0.1:9100")
    a = ap.parse_args()
    if a.synthetic == bool(a.file):
        sys.exit("give a recording file or --synthetic")
    frames = synthetic() if a.synthetic else (json.loads(l) for l in open(a.file) if l.strip())
    host, port = a.to.rsplit(":", 1)
    s, start = socket.socket(socket.AF_INET, socket.SOCK_DGRAM), time.monotonic()
    for f in frames:
        time.sleep(max(0.0, f["t"] - (time.monotonic() - start)))
        s.sendto(json.dumps(f).encode(), (host, int(port)))


if __name__ == "__main__":
    main()
