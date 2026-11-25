"""Live view of what the tap sends, with plain-language checks for joint indices and axes.

  python watch.py [--listen 127.0.0.1:9100]     (stop the bridge first: one listener per port)

Stand in view and follow BRINGUP.md. Every 0.25 s it prints pelvis, head and wrists and says what it
sees: which hand is raised, whether you are left or right of the origin. If the words do not match what
you are doing, an index or axis in pear_tap.py is wrong.
"""
import argparse
import json
import socket
import time


def describe(frame):
    """One line in plain words for a frame, or a reason it cannot be read."""
    j = frame.get("joints") or {}
    if frame.get("confidence", 0.0) <= 0.0 or "pelvis" not in j:
        return "no person tracked"
    try:
        pel, head = j["pelvis"], j["head"]
        lw, rw = j["left_wrist"], j["right_wrist"]
    except KeyError as e:
        return f"missing joint {e}"
    notes = []
    up = [("left", lw[2]), ("right", rw[2])]
    raised = [name for name, z in up if z > head[2]]
    notes.append("hands: " + (" and ".join(raised) + " above head" if raised else "both below head"))
    notes.append("head above pelvis" if head[2] > pel[2] else "HEAD BELOW PELVIS (up axis or indices wrong)")
    notes.append(f"{'right' if pel[0] > 0 else 'left'} of origin, {'forward' if pel[1] > 0 else 'back'} of origin")
    return "; ".join(notes)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", default="127.0.0.1:9100")
    a = ap.parse_args()
    h, p = a.listen.rsplit(":", 1)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind((h, int(p)))
    s.settimeout(0.5)
    last = 0.0
    print(f"listening on {a.listen}; Ctrl+C to stop")
    while True:
        try:
            data, _ = s.recvfrom(65535)
        except socket.timeout:
            if time.monotonic() - last > 2.0:
                print("nothing received")
                last = time.monotonic()
            continue
        if time.monotonic() - last < 0.25:
            continue
        last = time.monotonic()
        try:
            f = json.loads(data)
            j = f.get("joints") or {}
            nums = " ".join(f"{k}=({v[0]:+.2f},{v[1]:+.2f},{v[2]:+.2f})" for k, v in j.items())
            print(f"{nums}\n    {describe(f)}")
        except (ValueError, AttributeError, TypeError, IndexError):
            print("unreadable packet")


if __name__ == "__main__":
    main()
