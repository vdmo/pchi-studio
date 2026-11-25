"""Record the tap's frames to a .jsonl file so a real session can be replayed and tuned offline.

  python record.py session.jsonl --seconds 60 [--listen 127.0.0.1:9100]
Stop the bridge first (or record on another port): two listeners cannot share the port.

Each line is the frame as received plus "t", seconds since the first frame, which is what replay.py
reads. Frames that are not valid JSON objects are skipped and counted. Recordings contain body positions
of a real person: treat them as personal data and do not commit them.
"""
import argparse
import json
import socket
import sys
import time


def record(sock, path, seconds):
    """Write frames from `sock` to `path` for `seconds`. Returns (frames, skipped)."""
    sock.settimeout(0.2)
    end = time.monotonic() + seconds
    t0, frames, skipped = None, 0, 0
    with open(path, "w", encoding="utf-8") as out:
        while time.monotonic() < end:
            try:
                data, _ = sock.recvfrom(65535)
            except socket.timeout:
                continue
            try:
                f = json.loads(data)
            except ValueError:
                f = None
            if not isinstance(f, dict):
                skipped += 1
                continue
            now = time.monotonic()
            t0 = now if t0 is None else t0
            f["t"] = round(now - t0, 4)
            out.write(json.dumps(f) + "\n")
            frames += 1
    return frames, skipped


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--seconds", type=float, default=30.0)
    ap.add_argument("--listen", default="127.0.0.1:9100")
    a = ap.parse_args()
    h, p = a.listen.rsplit(":", 1)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.bind((h, int(p)))
    except OSError as e:
        sys.exit(f"cannot listen on {a.listen}: {e} (is the bridge still running?)")
    print(f"recording {a.seconds:g}s from {a.listen} to {a.path}")
    frames, skipped = record(s, a.path, a.seconds)
    print(f"{frames} frames, {skipped} skipped")


if __name__ == "__main__":
    main()
