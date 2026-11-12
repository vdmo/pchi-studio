#!/usr/bin/env python3
"""
Strobe PCHI Bridge
Connects Strobe (https://github.com/lnorton89/strobe, a Synesthesia-style VJ
engine) to the PCHI Conductor's shared show state.

    Strobe --OSC feedback--> this bridge --raw JSON UDP--> PCHI Conductor
    PCHI Conductor --WebSocket--> this bridge --OSC--> Strobe

What it carries, and what it deliberately does not:

  Strobe -> PCHI
    /strobe/state/cue N       -> show_state "enter" for the state mapped to cue N
    /strobe/state/blackout B  -> department readiness: degraded while blacked out
    /strobe/control/<name> v  -> control_parameter strobe.<name> (only for the
                                 names listed under "forward_controls", so the
                                 Conductor's rules can govern them)
    first feedback of any kind -> department readiness: ready

  PCHI -> Strobe
    show_state "enter" from another source -> /strobe/cue/N

Honest limits (also in the README):
  * Strobe sends OSC feedback only when something changes. There is no Strobe
    heartbeat, so this bridge cannot detect Strobe dying — only that the
    bridge itself is alive (it heartbeats every 5 s) and that Strobe has been
    heard from at least once. "ready" means "Strobe is talking to us", not
    "the projector is lit".
  * Not tested against a running Strobe (a Windows/GPU app); the OSC addresses
    are taken from Strobe's own source (crates/strobe-io/src/{osc,feedback}.rs).
    tools/strobe-pchi-bridge/fake_strobe.py speaks the same addresses.

Strobe setup: Mappings -> OSC on (port 9000 by default), feedback on, feedback
host 127.0.0.1, feedback port 9001 (Strobe's defaults; match --feedback-port).
"""

import argparse
import json
import logging
import socket
import threading
import time
from typing import Any, Callable, Dict, List, Optional

log = logging.getLogger("strobe_pchi_bridge")

SOURCE_ID = "strobe_bridge"
DEFAULT_DEPARTMENT = "visuals"


def zero_pir_invariants() -> Dict[str, Any]:
    """Structurally complete, honestly-zero invariants. Show-state and control
    messages carry no multi-value equilibrium claim (see docs/LIMITATIONS.md
    item 7), but the schema requires the full shape on every message."""
    return {
        "equilibrium_check": {"residual": 0.0, "precision": 1e-12, "sign_sequence": []},
        "coherence_gap": 0.0,
        "curvature_signature": "0.0, 0.0, 0.0",
        "residual": 0.0,
    }


def wire_message(message_type: str, payload_type: str, data: Dict[str, Any],
                 source_id: str = SOURCE_ID) -> Dict[str, Any]:
    """The exact JSON the Conductor's UDP loop parses (wire names, not Python's)."""
    return {
        "version": "2.0.0",
        "type": message_type,
        "source_id": source_id,
        "timestamp": time.time() * 1000,
        "pir_invariants": zero_pir_invariants(),
        "payload": {"payloadType": payload_type, "data": data},
    }


def load_config(path: str) -> Dict[str, Any]:
    with open(path, "r", encoding="utf-8") as f:
        config = json.load(f)
    validate_config(config)
    return config


def validate_config(config: Dict[str, Any]) -> None:
    states = config.get("states")
    if not isinstance(states, list) or not states:
        raise ValueError("config needs a non-empty 'states' list")
    seen_states, seen_cues = set(), set()
    for entry in states:
        state, cue = entry.get("state"), entry.get("cue")
        if not isinstance(state, str) or not state.strip():
            raise ValueError(f"state entry needs a non-empty 'state': {entry!r}")
        if not isinstance(cue, int) or isinstance(cue, bool) or cue < 1:
            raise ValueError(f"state {state!r}: 'cue' must be an integer >= 1 (Strobe cues count from 1)")
        if state in seen_states:
            raise ValueError(f"duplicate state {state!r}")
        if cue in seen_cues:
            raise ValueError(f"cue {cue} is mapped to two states")
        seen_states.add(state)
        seen_cues.add(cue)


class StrobeBridge:
    """Pure bridge logic. I/O is injected so it can be tested without sockets:

    send_pchi(dict)             -> deliver one PCHI message to the Conductor
    send_osc(address, *args)    -> deliver one OSC message to Strobe
    """

    def __init__(self, config: Dict[str, Any],
                 send_pchi: Callable[[Dict[str, Any]], None],
                 send_osc: Callable[..., None],
                 source_id: str = SOURCE_ID):
        validate_config(config)
        self.source_id = source_id
        self.show = config.get("show")
        self.department = config.get("department", DEFAULT_DEPARTMENT)
        self.forward_controls = set(config.get("forward_controls", []))
        self.states: List[str] = [e["state"] for e in config["states"]]
        self.cue_to_state = {e["cue"]: e["state"] for e in config["states"]}
        self.state_to_cue = {e["state"]: e["cue"] for e in config["states"]}
        self.send_pchi = send_pchi
        self.send_osc = send_osc

        self.current_state: Optional[str] = None
        self.strobe_heard = False
        self.blackout = False
        self._last_readiness: Optional[tuple] = None

    # ---- PCHI messages -------------------------------------------------

    def _show_state(self, data: Dict[str, Any]) -> Dict[str, Any]:
        return wire_message("show_state", "show_state", data, self.source_id)

    def startup_messages(self) -> List[Dict[str, Any]]:
        """Announce the plan and that visuals are not ready until Strobe talks."""
        msgs = [self._show_state({"kind": "plan", "show": self.show, "states": self.states})]
        msgs.extend(self._readiness_if_changed())
        return msgs

    def heartbeat(self) -> Dict[str, Any]:
        return wire_message("heartbeat", "control_parameter",
                            {"target_id": self.source_id, "parameter": "heartbeat", "value": 1.0},
                            self.source_id)

    def _readiness(self) -> tuple:
        if not self.strobe_heard:
            return ("not_ready", "waiting for Strobe feedback")
        if self.blackout:
            return ("degraded", "blackout active")
        return ("ready", None)

    def _readiness_if_changed(self) -> List[Dict[str, Any]]:
        current = self._readiness()
        if current == self._last_readiness:
            return []
        self._last_readiness = current
        status, detail = current
        return [self._show_state({"kind": "readiness", "department": self.department,
                                  "status": status, "detail": detail})]

    # ---- Strobe -> PCHI -----------------------------------------------

    def on_strobe_osc(self, address: str, *args) -> None:
        """Handle one OSC message from Strobe's feedback sender."""
        out: List[Dict[str, Any]] = []
        self.strobe_heard = True

        if address == "/strobe/state/cue" and args:
            cue = _as_int(args[0])
            state = self.cue_to_state.get(cue) if cue is not None else None
            if cue and state is None:
                log.warning("Strobe is on cue %s, which is not mapped to any state", cue)
            elif state is not None and state != self.current_state:
                self.current_state = state
                out.append(self._show_state({"kind": "enter", "state": state}))
        elif address == "/strobe/state/blackout" and args:
            self.blackout = bool(_as_int(args[0]))
        elif address.startswith("/strobe/control/") and args:
            name = address[len("/strobe/control/"):]
            value = _as_float(args[0])
            if name in self.forward_controls and value is not None:
                out.append(wire_message("control_parameter", "control_parameter",
                                        {"target_id": "strobe", "parameter": name, "value": value},
                                        self.source_id))

        # Readiness first, so a state change never lands before its context.
        for msg in self._readiness_if_changed() + out:
            self.send_pchi(msg)

    # ---- PCHI -> Strobe -----------------------------------------------

    def on_pchi_message(self, msg: Dict[str, Any]) -> None:
        """Handle one message broadcast by the Conductor over its WebSocket."""
        if msg.get("type") != "show_state" or msg.get("source_id") == self.source_id:
            return
        payload = msg.get("payload", {})
        data = payload.get("data", {}) if payload.get("payloadType") == "show_state" else {}
        if data.get("kind") == "rejected":
            self._on_rejected(data)
            return
        if data.get("kind") != "enter":
            return
        state = data.get("state")
        cue = self.state_to_cue.get(state)
        if cue is None:
            log.info("PCHI entered %r, which has no Strobe cue; ignoring", state)
            return
        # Record first: Strobe will report this cue back, and that must not be
        # re-announced as a new transition.
        self.current_state = state
        self.send_osc(f"/strobe/cue/{cue}", 1)


    def _on_rejected(self, data: Dict[str, Any]) -> None:
        """The Conductor refused a transition (a rule said no).

        Only matters if it was OUR announcement, i.e. Strobe moved on its own
        into the refused state. Then Strobe is put back on the cue for the
        state the show is actually in. A refusal of someone else's request is
        none of Strobe's business: Strobe was never moved.
        """
        if data.get("state") != self.current_state:
            return
        current = data.get("current")
        cue = self.state_to_cue.get(current)
        if cue is None:
            # Nothing to go back to (no state entered yet, or one with no cue).
            log.warning("Conductor refused %r (%s) but there is no Strobe cue to return to; "
                        "Strobe stays where it is", data.get("state"), data.get("reason"))
            return
        log.warning("Conductor refused %r (%s); returning Strobe to %r (cue %d)",
                    data.get("state"), data.get("reason"), current, cue)
        self.current_state = current
        self.send_osc(f"/strobe/cue/{cue}", 1)


def _as_int(v) -> Optional[int]:
    try:
        return int(v)
    except (TypeError, ValueError):
        return None


def _as_float(v) -> Optional[float]:
    try:
        f = float(v)
    except (TypeError, ValueError):
        return None
    return f if f == f and f not in (float("inf"), float("-inf")) else None


# ---- Runtime wiring (sockets, threads) -----------------------------------

def run(args) -> None:
    from pythonosc import udp_client
    from pythonosc.dispatcher import Dispatcher
    from pythonosc.osc_server import BlockingOSCUDPServer
    try:
        import websocket  # websocket-client
    except ImportError:
        websocket = None

    config = load_config(args.config)
    pchi_socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    strobe = udp_client.SimpleUDPClient(args.strobe_host, args.strobe_port)

    def send_pchi(msg: Dict[str, Any]) -> None:
        try:
            pchi_socket.sendto(json.dumps(msg).encode("utf-8"), (args.pchi_host, args.pchi_port))
        except OSError as e:
            log.error("sending to the Conductor failed: %s", e)

    def send_osc(address: str, *a) -> None:
        strobe.send_message(address, list(a))
        log.info("-> Strobe %s %s", address, list(a))

    bridge = StrobeBridge(config, send_pchi, send_osc)
    running = threading.Event()
    running.set()

    def heartbeats():
        while running.is_set():
            send_pchi(bridge.heartbeat())
            time.sleep(5)

    def watch_conductor():
        if websocket is None:
            log.warning("websocket-client not installed (pip install websocket-client): "
                        "PCHI -> Strobe is disabled; Strobe -> PCHI still works.")
            return

        def on_message(_ws, raw):
            try:
                bridge.on_pchi_message(json.loads(raw))
            except json.JSONDecodeError:
                pass

        while running.is_set():
            try:
                websocket.WebSocketApp(f"ws://{args.pchi_host}:{args.pchi_ws_port}",
                                       on_message=on_message).run_forever()
            except Exception as e:  # noqa: BLE001 - long-lived watcher, reconnect on anything
                log.warning("conductor WebSocket failed: %s", e)
            if running.is_set():
                time.sleep(2)

    for msg in bridge.startup_messages():
        send_pchi(msg)
    threading.Thread(target=heartbeats, daemon=True).start()
    threading.Thread(target=watch_conductor, daemon=True).start()

    dispatcher = Dispatcher()
    dispatcher.set_default_handler(bridge.on_strobe_osc)
    server = BlockingOSCUDPServer((args.feedback_host, args.feedback_port), dispatcher)
    log.info("Listening for Strobe OSC feedback on %s:%d; commands go to %s:%d",
             args.feedback_host, args.feedback_port, args.strobe_host, args.strobe_port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        running.clear()


def main() -> None:
    p = argparse.ArgumentParser(description="Strobe PCHI Bridge")
    p.add_argument("--config", required=True, help="state<->cue mapping JSON (see demos/strobe-show/strobe_show.json)")
    p.add_argument("--feedback-host", default="127.0.0.1", help="interface to receive Strobe's OSC feedback on")
    p.add_argument("--feedback-port", type=int, default=9001, help="Strobe's feedback port (Strobe default 9001)")
    p.add_argument("--strobe-host", default="127.0.0.1", help="host running Strobe")
    p.add_argument("--strobe-port", type=int, default=9000, help="Strobe's OSC listener port (Strobe default 9000)")
    p.add_argument("--pchi-host", default="127.0.0.1")
    p.add_argument("--pchi-port", type=int, default=8888, help="Conductor UDP port")
    p.add_argument("--pchi-ws-port", type=int, default=8889, help="Conductor WebSocket port")
    p.add_argument("-v", "--verbose", action="store_true")
    args = p.parse_args()
    logging.basicConfig(level=logging.DEBUG if args.verbose else logging.INFO,
                        format="%(asctime)s %(message)s")
    run(args)


if __name__ == "__main__":
    main()
