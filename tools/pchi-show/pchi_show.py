#!/usr/bin/env python3
"""
pchi-show — a tiny operator/simulator CLI for the shared show state.

Anything that can send JSON over UDP can drive Mission Control; this is the
smallest tool that does it, for demos, rehearsals and tests. Standard
library only.

  pchi_show.py plan "PRESHOW,INTRO,BUILD,DROP,FINALE" --show "Demo"
  pchi_show.py enter BUILD
  pchi_show.py ready lights ready
  pchi_show.py ready lasers not_ready --detail "safety check pending"
  pchi_show.py dept lasers --source sim_lasers --status ready \\
        --schedule "20:degraded:warming up,35:ready"

`dept` stays running and heartbeats as its own source, so the Conductor sees
the department as alive. Without that, a department that reported once and
went quiet would (correctly) turn "offline" after 15 s.
"""
import argparse
import json
import socket
import sys
import time

STATUSES = ("ready", "degraded", "not_ready")


def wire(source, data, message_type="show_state", payload_type="show_state"):
    return {
        "version": "2.0.0",
        "type": message_type,
        "source_id": source,
        "timestamp": time.time() * 1000,
        "pir_invariants": {
            "equilibrium_check": {"residual": 0.0, "precision": 1e-12, "sign_sequence": []},
            "coherence_gap": 0.0,
            "curvature_signature": "0.0, 0.0, 0.0",
            "residual": 0.0,
        },
        "payload": {"payloadType": payload_type, "data": data},
    }


def heartbeat(source):
    return wire(source, {"target_id": source, "parameter": "heartbeat", "value": 1.0},
                "heartbeat", "control_parameter")


def readiness(source, department, status, detail=None):
    return wire(source, {"kind": "readiness", "department": department,
                         "status": status, "detail": detail})


def parse_schedule(text):
    """'20:degraded:warming up,35:ready' -> [(20.0, 'degraded', 'warming up'), (35.0, 'ready', None)]"""
    steps = []
    for part in filter(None, (p.strip() for p in (text or "").split(","))):
        bits = part.split(":", 2)
        if len(bits) < 2 or bits[1] not in STATUSES:
            raise ValueError(f"bad schedule step {part!r}; want SECONDS:STATUS[:DETAIL] with STATUS in {STATUSES}")
        steps.append((float(bits[0]), bits[1], bits[2] if len(bits) > 2 else None))
    return sorted(steps, key=lambda s: s[0])


def main(argv=None):
    p = argparse.ArgumentParser(description="PCHI show-state operator/simulator")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8888)
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("plan", help="set the ordered journey of states")
    s.add_argument("states", help="comma-separated, e.g. PRESHOW,INTRO,FINALE")
    s.add_argument("--show")
    s.add_argument("--source", default="operator")

    s = sub.add_parser("enter", help="enter a state")
    s.add_argument("state")
    s.add_argument("--next", dest="next_state", help="override the plan's next state")
    s.add_argument("--source", default="operator")

    s = sub.add_parser("ready", help="report a department's readiness once")
    s.add_argument("department")
    s.add_argument("status", choices=STATUSES)
    s.add_argument("--detail")
    s.add_argument("--source", default="operator")

    s = sub.add_parser("dept", help="simulate a department: heartbeat + readiness, until Ctrl+C")
    s.add_argument("department")
    s.add_argument("--source", required=True, help="source id to heartbeat as (unique per department)")
    s.add_argument("--status", choices=STATUSES, default="ready")
    s.add_argument("--detail")
    s.add_argument("--schedule", help="status changes over time: SECONDS:STATUS[:DETAIL],...")

    args = p.parse_args(argv)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    def send(msg):
        sock.sendto(json.dumps(msg).encode(), (args.host, args.port))

    if args.cmd == "plan":
        states = [x.strip() for x in args.states.split(",") if x.strip()]
        if not states:
            p.error("plan needs at least one state")
        send(wire(args.source, {"kind": "plan", "show": args.show, "states": states}))
    elif args.cmd == "enter":
        send(wire(args.source, {"kind": "enter", "state": args.state, "next_state": args.next_state}))
    elif args.cmd == "ready":
        send(readiness(args.source, args.department, args.status, args.detail))
    elif args.cmd == "dept":
        try:
            schedule = parse_schedule(args.schedule)
        except ValueError as e:
            p.error(str(e))
        status, detail = args.status, args.detail
        start = time.monotonic()
        try:
            while True:
                elapsed = time.monotonic() - start
                while schedule and schedule[0][0] <= elapsed:
                    _, status, detail = schedule.pop(0)
                    print(f"[{args.department}] -> {status} {detail or ''}".strip(), flush=True)
                send(heartbeat(args.source))
                send(readiness(args.source, args.department, status, detail))
                time.sleep(2)
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
