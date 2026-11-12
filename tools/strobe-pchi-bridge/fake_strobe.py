#!/usr/bin/env python3
"""
Fake Strobe — stands in for the real app so the demo runs anywhere.

It speaks the OSC addresses documented in Strobe's own source
(crates/strobe-io/src/{osc,feedback}.rs):

  listens (default :9000)          sends feedback (default 127.0.0.1:9001)
    /strobe/cue/<n>                  /strobe/state/cue <int>
    /strobe/cmd/next_scene           /strobe/state/blackout <int>
    /strobe/control/<name> <f>       /strobe/control/<name> <f>

Like the real thing, it sends everything once at start and afterwards only
what changed. It renders nothing and knows nothing about Strobe beyond those
addresses — it proves the bridge's wiring, not Strobe's behaviour.
"""
import argparse
import logging
import threading
import time

from pythonosc import udp_client
from pythonosc.dispatcher import Dispatcher
from pythonosc.osc_server import ThreadingOSCUDPServer

log = logging.getLogger("fake_strobe")


class FakeStrobe:
    def __init__(self, send, cues: int, start_cue: int = 1):
        self.send = send  # send(address, value)
        self.cues = cues
        self.cue = start_cue
        self.controls = {"strobe_intensity": 0.2, "speed": 0.5}
        self.lock = threading.Lock()

    def announce_all(self):
        with self.lock:
            self.send("/strobe/state/blackout", 0)
            self.send("/strobe/state/cue", self.cue)
            for name, v in self.controls.items():
                self.send(f"/strobe/control/{name}", v)

    def set_cue(self, n: int):
        with self.lock:
            if 1 <= n <= self.cues and n != self.cue:
                self.cue = n
                log.info("cue -> %d", n)
                self.send("/strobe/state/cue", n)

    def set_control(self, name: str, v: float):
        with self.lock:
            if name in self.controls and abs(self.controls[name] - v) > 1e-3:
                self.controls[name] = max(0.0, min(1.0, v))
                self.send(f"/strobe/control/{name}", self.controls[name])

    def handle(self, address: str, *args):
        if address.startswith("/strobe/cue/"):
            try:
                self.set_cue(int(address.rsplit("/", 1)[1]))
            except ValueError:
                pass
        elif address == "/strobe/cmd/next_scene":
            self.set_cue(self.cue + 1)
        elif address.startswith("/strobe/control/") and args:
            self.set_control(address[len("/strobe/control/"):], float(args[0]))


def main():
    p = argparse.ArgumentParser(description="Fake Strobe (OSC stand-in)")
    p.add_argument("--listen-port", type=int, default=9000)
    p.add_argument("--feedback-host", default="127.0.0.1")
    p.add_argument("--feedback-port", type=int, default=9001)
    p.add_argument("--cues", type=int, default=5, help="number of cues in the fake playlist")
    p.add_argument("--start-cue", type=int, default=1, help="cue playing at start (0 = none)")
    p.add_argument("--auto-advance", type=float, default=0, metavar="SECS",
                   help="advance the playlist every SECS seconds (0 = only when told)")
    p.add_argument("--ramp-strobe-after", type=float, default=0, metavar="SECS",
                   help="after SECS, an 'operator' pushes strobe_intensity up in steps past 0.8")
    args = p.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")

    client = udp_client.SimpleUDPClient(args.feedback_host, args.feedback_port)
    fake = FakeStrobe(client.send_message, args.cues, start_cue=args.start_cue)

    dispatcher = Dispatcher()
    dispatcher.set_default_handler(fake.handle)
    server = ThreadingOSCUDPServer(("127.0.0.1", args.listen_port), dispatcher)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    log.info("fake Strobe listening on :%d, feedback to %s:%d", args.listen_port,
             args.feedback_host, args.feedback_port)

    time.sleep(0.5)
    fake.announce_all()

    start = time.monotonic()
    last_advance = start
    ramped = 0.2
    try:
        while True:
            time.sleep(0.25)
            now = time.monotonic()
            if args.auto_advance and now - last_advance >= args.auto_advance:
                last_advance = now
                fake.set_cue(fake.cue + 1 if fake.cue < args.cues else 1)
            if args.ramp_strobe_after and now - start >= args.ramp_strobe_after and ramped < 0.95:
                ramped = min(0.95, ramped + 0.1)
                log.info("operator raises strobe_intensity to %.2f", ramped)
                fake.set_control("strobe_intensity", ramped)
                time.sleep(0.75)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
