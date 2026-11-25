"""Drop-in tap for toyxyz_RGBMocap's PEAR live path (written against its public source; never run inside it).

Copy this file next to mocap/pear/session.py and add, inside Session.read(), right after the
`with self.lock:` block that stores the frame (still inside the `try`):

    from . import pear_tap; pear_tap.emit(meta, arrays)

emit() never raises and never blocks (one non-blocking UDP send), so the inference and GUI threads are
unaffected. It sends the bridge's input format: {"confidence", "joints": {"pelvis": [x,y,z], ...}}.

Facts taken from the PEAR source (mocap/pear/worker.py, wire.py, session.py):
- A frame arrives as meta['type']=='frame' with arrays['joints'] of shape (55, 3), float32, already
  through PEAR's One Euro filter and root placement. A lost person arrives as meta['type']=='missing'.
- PEAR publishes no measured confidence ("heuristics, not measured confidence" in docs/models/PEAR.md),
  so confidence here is only 1.0 (frame) or 0.0 (missing).
- The worker writes joints with axes remapped [0, 2, 1] and z negated, and the viewer draws a Y-up grid,
  so the array is treated as Y-up and remapped here to the bridge's Z-up.

NOT verified, tune against a recorded test video before trusting any number:
- JOINTS indices are the usual SMPL-X order (0 pelvis, 15 head, 20/21 wrists). The repo's code shows 55
  joints with a parent table but I did not find a name list.
- AXES/SIGNS (handedness and which way is forward) and units (metres only if camera alignment succeeded;
  otherwise PEAR itself says scale and floor are inaccurate).
- ORIGIN: PEAR's origin is not your stage centre. Set the offset so the performer's mark reads (0, 0).
"""
import json
import os
import socket

JOINTS = {"pelvis": 0, "head": 15, "left_wrist": 20, "right_wrist": 21}
AXES = (0, 2, 1)          # output (x, y, z) taken from PEAR (x, z, y): PEAR is Y-up, the bridge is Z-up
SIGNS = (1.0, 1.0, 1.0)
ORIGIN = (0.0, 0.0, 0.0)  # metres, subtracted after the remap

_ADDR = (os.environ.get("PEAR_TAP_HOST", "127.0.0.1"), int(os.environ.get("PEAR_TAP_PORT", "9100")))
_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
_sock.setblocking(False)


def to_stage(row):
    return [SIGNS[i] * float(row[AXES[i]]) - ORIGIN[i] for i in range(3)]


def build(meta, arrays):
    """The frame dict to send, or None when there is nothing to say."""
    kind = meta.get("type")
    if kind == "missing":
        return {"confidence": 0.0, "joints": {}}
    if kind != "frame":
        return None
    joints = arrays["joints"]
    return {"confidence": 1.0, "joints": {name: to_stage(joints[i]) for name, i in JOINTS.items()}}


def emit(meta, arrays):
    try:
        frame = build(meta, arrays)
        if frame is not None:
            _sock.sendto(json.dumps(frame).encode(), _ADDR)
    except Exception:
        pass  # a failing tap must never disturb tracking
