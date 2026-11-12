"""Tests for the bridge's logic (no sockets, no Strobe). Run:

    python3 -m unittest tools/strobe-pchi-bridge/test_strobe_pchi_bridge.py -v
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from strobe_pchi_bridge import StrobeBridge, validate_config  # noqa: E402

CONFIG = {
    "show": "Test",
    "department": "visuals",
    "forward_controls": ["strobe_intensity"],
    "states": [
        {"state": "INTRO", "cue": 1},
        {"state": "BUILD", "cue": 2},
        {"state": "DROP", "cue": 3},
    ],
}


def data(msg):
    return msg["payload"]["data"]


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.pchi, self.osc = [], []
        self.b = StrobeBridge(CONFIG, self.pchi.append, lambda a, *args: self.osc.append((a, args)))

    def kinds(self):
        return [(data(m).get("kind"), data(m).get("state") or data(m).get("status")) for m in self.pchi
                if m["type"] == "show_state"]

    def test_startup_announces_plan_and_not_ready_visuals(self):
        msgs = self.b.startup_messages()
        self.assertEqual(data(msgs[0]), {"kind": "plan", "show": "Test", "states": ["INTRO", "BUILD", "DROP"]})
        self.assertEqual(data(msgs[1])["status"], "not_ready")
        self.assertEqual(data(msgs[1])["department"], "visuals")

    def test_first_feedback_makes_visuals_ready_before_any_state_change(self):
        self.b.startup_messages()
        self.b.on_strobe_osc("/strobe/state/cue", 2)
        self.assertEqual(self.kinds(), [("readiness", "ready"), ("enter", "BUILD")])

    def test_cue_zero_is_no_cue_and_enters_nothing(self):
        self.b.on_strobe_osc("/strobe/state/cue", 0)
        self.assertNotIn("enter", [k for k, _ in self.kinds()])

    def test_unmapped_cue_enters_nothing(self):
        self.b.on_strobe_osc("/strobe/state/cue", 9)
        self.assertNotIn("enter", [k for k, _ in self.kinds()])

    def test_repeated_cue_feedback_is_announced_once(self):
        for _ in range(3):
            self.b.on_strobe_osc("/strobe/state/cue", 1)
        self.assertEqual([k for k, _ in self.kinds()].count("enter"), 1)

    def test_blackout_degrades_then_recovers_without_spamming(self):
        self.b.on_strobe_osc("/strobe/state/blackout", 0)
        self.b.on_strobe_osc("/strobe/state/blackout", 0)
        self.b.on_strobe_osc("/strobe/state/blackout", 1)
        self.b.on_strobe_osc("/strobe/state/blackout", 0)
        self.assertEqual([s for _, s in self.kinds()], ["ready", "degraded", "ready"])

    def test_only_listed_controls_are_forwarded_as_control_parameters(self):
        self.b.on_strobe_osc("/strobe/control/strobe_intensity", 0.9)
        self.b.on_strobe_osc("/strobe/control/speed", 0.3)
        sent = [m for m in self.pchi if m["type"] == "control_parameter"]
        self.assertEqual(len(sent), 1)
        self.assertEqual(data(sent[0]), {"target_id": "strobe", "parameter": "strobe_intensity", "value": 0.9})

    def test_non_finite_control_values_are_dropped(self):
        self.b.on_strobe_osc("/strobe/control/strobe_intensity", float("nan"))
        self.assertEqual([m for m in self.pchi if m["type"] == "control_parameter"], [])

    def test_pchi_enter_from_elsewhere_drives_strobe(self):
        self.b.on_pchi_message({"type": "show_state", "source_id": "operator",
                                "payload": {"payloadType": "show_state",
                                            "data": {"kind": "enter", "state": "DROP"}}})
        self.assertEqual(self.osc, [("/strobe/cue/3", (1,))])

    def test_strobe_reporting_back_the_driven_cue_is_not_re_announced(self):
        self.b.on_pchi_message({"type": "show_state", "source_id": "operator",
                                "payload": {"payloadType": "show_state",
                                            "data": {"kind": "enter", "state": "DROP"}}})
        self.b.on_strobe_osc("/strobe/state/cue", 3)
        self.assertNotIn("enter", [k for k, _ in self.kinds()], "that would echo the transition")

    def test_own_messages_and_unknown_states_are_ignored(self):
        own = {"type": "show_state", "source_id": "strobe_bridge",
               "payload": {"payloadType": "show_state", "data": {"kind": "enter", "state": "DROP"}}}
        self.b.on_pchi_message(own)
        unknown = {"type": "show_state", "source_id": "operator",
                   "payload": {"payloadType": "show_state", "data": {"kind": "enter", "state": "NOPE"}}}
        self.b.on_pchi_message(unknown)
        self.b.on_pchi_message({"type": "governance_event", "source_id": "x", "payload": {}})
        self.assertEqual(self.osc, [])

    def rejected(self, state, current):
        return {"type": "show_state", "source_id": "pchi-conductor",
                "payload": {"payloadType": "show_state",
                            "data": {"kind": "rejected", "state": state, "reason": "no",
                                     "current": current}}}

    def test_refused_transition_that_strobe_made_itself_puts_strobe_back(self):
        self.b.on_strobe_osc("/strobe/state/cue", 1)   # show is in INTRO
        self.b.on_strobe_osc("/strobe/state/cue", 3)   # Strobe advances itself to DROP
        self.b.on_pchi_message(self.rejected("DROP", "INTRO"))
        self.assertEqual(self.osc, [("/strobe/cue/1", (1,))])
        n = len(self.pchi)
        self.b.on_strobe_osc("/strobe/state/cue", 1)   # Strobe confirms it went back
        self.assertNotIn("enter", [k for k, _ in self.kinds()][n:], "going back is not news")

    def test_refusal_of_someone_elses_request_does_not_touch_strobe(self):
        self.b.on_strobe_osc("/strobe/state/cue", 1)
        self.b.on_pchi_message(self.rejected("DROP", "INTRO"))
        self.assertEqual(self.osc, [])

    def test_refusal_with_nowhere_to_return_leaves_strobe_alone(self):
        self.b.on_strobe_osc("/strobe/state/cue", 3)
        self.b.on_pchi_message(self.rejected("DROP", None))
        self.assertEqual(self.osc, [])

    def test_refresh_reannounces_plan_and_readiness_but_never_enter(self):
        self.b.on_strobe_osc("/strobe/state/cue", 2)
        self.pchi.clear()
        msgs = self.b.refresh_messages()
        kinds = [data(m)["kind"] for m in msgs]
        self.assertEqual(kinds, ["plan", "readiness"])
        self.assertEqual(data(msgs[1])["status"], "ready")
        self.assertNotIn("enter", kinds, "a stale enter would drag the show back")

    def test_wire_shape_matches_what_the_conductor_parses(self):
        m = self.b.startup_messages()[0]
        self.assertEqual(m["version"], "2.0.0")
        self.assertEqual(m["type"], "show_state")
        self.assertEqual(m["payload"]["payloadType"], "show_state")
        self.assertIn("equilibrium_check", m["pir_invariants"])


class ConfigTests(unittest.TestCase):
    def test_rejects_cue_zero_duplicate_cues_and_duplicate_states(self):
        for bad in (
            {"states": []},
            {"states": [{"state": "A", "cue": 0}]},
            {"states": [{"state": "A", "cue": 1}, {"state": "B", "cue": 1}]},
            {"states": [{"state": "A", "cue": 1}, {"state": "A", "cue": 2}]},
            {"states": [{"state": " ", "cue": 1}]},
        ):
            with self.assertRaises(ValueError, msg=bad):
                validate_config(bad)


if __name__ == "__main__":
    unittest.main()
