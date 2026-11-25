import json
import math
import os
import socket
import tempfile
import threading
import time
import unittest

import pear_tap
import record
import tune
from rgbmocap_pchi_bridge import Limits, Tracker, clamp_stage, control_message, expressive, pchi_message


def frame(p, conf=0.9):
    return {"confidence": conf, "joints": {"pelvis": p}}


class Bridge(unittest.TestCase):
    def test_radius_clamp(self):
        p, c = clamp_stage((3.0, 4.0, 1.0), Limits())
        self.assertTrue(c)
        self.assertAlmostEqual(math.hypot(p[0], p[1]), 1.5)

    def test_teleport_is_slewed(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        t.update(frame((1.4, 0, 1)), 0.05)  # 28 m/s: a tracker glitch, not a person
        self.assertLessEqual(t.pos[0], 6.0 * 0.05 + 1e-9)

    def test_non_finite_and_malformed_rejected(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        for bad in [frame((float("nan"), 0, 1)), {"joints": {}}, frame((1, 2)),
                    frame((0, 0, 1), conf=float("nan")), {"joints": None}]:
            t.update(bad, 0.1)
        self.assertEqual(t.pos, (0.0, 0.0, 1.0))
        self.assertEqual(t.rejected, 3)  # frames with no confidence count as low confidence, not errors

    def test_low_confidence_keeps_last_good(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        t.update(frame((0.1, 0, 1)), 0.05)
        t.update(frame((1.0, 0, 1), conf=0.1), 0.1)
        self.assertAlmostEqual(t.pos[0], 0.1)

    def test_missing_person_frame_is_occlusion_not_error(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        t.update(pear_tap.build({"type": "missing"}, {}), 0.1)
        self.assertEqual(t.rejected, 0)
        self.assertEqual(t.conf, 0.0)

    def test_tap_remaps_y_up_to_z_up(self):
        joints = [[0.0, 0.0, 0.0]] * 55
        joints[0] = [0.5, 1.0, 0.2]  # PEAR (x, y up, z)
        f = pear_tap.build({"type": "frame"}, {"joints": joints})
        self.assertEqual(f["joints"]["pelvis"], [0.5, 0.2, 1.0])
        self.assertEqual(set(f["joints"]), {"pelvis", "head", "left_wrist", "right_wrist"})
        self.assertIsNone(pear_tap.build({"type": "status"}, {}))

    def test_tap_never_raises(self):
        pear_tap.emit({"type": "frame"}, {})  # missing 'joints' array

    def test_expressive_values_are_normalised(self):
        t = Tracker(Limits())
        fr = {"confidence": 0.9, "joints": {"pelvis": [0, 0, 1], "left_wrist": [-0.9, 0, 2.2], "right_wrist": [0.9, 0, 1.0]}}
        t.update(fr, 0.0)
        e = expressive(t, (3.0, 4.0, 0.0))
        self.assertAlmostEqual(e["spread"], 1.0)
        self.assertAlmostEqual(e["hand_elevation"], 1.0)
        self.assertAlmostEqual(e["energy"], 1.0)
        self.assertTrue(all(0.0 <= v <= 1.0 for v in e.values()))

    def test_expressive_absent_without_wrists_and_bad_wrists_ignored(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        self.assertIsNone(expressive(t, (0, 0, 0)))
        t.update({"confidence": 0.9, "joints": {"pelvis": [0, 0, 1], "left_wrist": [float("nan"), 0, 1], "right_wrist": [0, 0, 1]}}, 0.05)
        self.assertIsNone(expressive(t, (0, 0, 0)))

    def test_control_message_shape(self):
        m = control_message("rgbmocap.spread", 0.123456)
        self.assertEqual(m["type"], "control_parameter")
        d = m["payload"]["data"]
        self.assertEqual((d["target_id"], d["parameter"], d["value"]), ("performer_01", "rgbmocap.spread", 0.1235))

    def test_lookahead_trades_lag_for_overshoot_in_simulation(self):
        rms0, _, over0 = tune.run(0.0, 0.08, 20.0, 60.0, 0.07)
        rms4, _, over4 = tune.run(0.04, 0.08, 20.0, 60.0, 0.07)
        self.assertLess(rms4, rms0)      # some look-ahead lowers average error ...
        self.assertGreater(over4, over0)  # ... and costs overshoot after a sudden stop

    def test_recording_round_trips_through_replay_format(self):
        rx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        rx.bind(("127.0.0.1", 0))
        port = rx.getsockname()[1]
        path = os.path.join(tempfile.mkdtemp(), "s.jsonl")
        result = {}
        th = threading.Thread(target=lambda: result.update(r=record.record(rx, path, 1.0)))
        th.start()
        tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sent = [{"confidence": 0.9, "joints": {"pelvis": [0.1 * i, 0, 1]}} for i in range(5)]
        for f in sent:
            tx.sendto(json.dumps(f).encode(), ("127.0.0.1", port))
            time.sleep(0.02)
        tx.sendto(b"not json", ("127.0.0.1", port))
        tx.sendto(b"[1, 2]", ("127.0.0.1", port))
        th.join()
        rx.close()
        tx.close()
        self.assertEqual(result["r"], (5, 2))
        lines = [json.loads(l) for l in open(path)]
        self.assertEqual([l["joints"] for l in lines], [f["joints"] for f in sent])
        self.assertEqual(lines[0]["t"], 0.0)
        self.assertTrue(all(b["t"] >= a["t"] for a, b in zip(lines, lines[1:])))
        for l in lines:  # replay.py needs "t" and the frame fields only
            self.assertIn("t", l)
            self.assertIn("confidence", l)

    def test_lost_after_silence_or_long_occlusion_and_recovers(self):
        t = Tracker(Limits())
        self.assertTrue(t.lost(0.0, 2.0))                     # never saw a frame
        t.update(frame((0, 0, 1)), 1.0)
        self.assertFalse(t.lost(2.5, 2.0))
        self.assertTrue(t.lost(3.5, 2.0))                     # tracker went silent
        t.update(frame((0, 0, 1), conf=0.1), 3.6)             # occluded frames do not count as usable
        self.assertTrue(t.lost(3.7, 2.0))
        t.update(frame((0.1, 0, 1)), 3.8)
        self.assertFalse(t.lost(3.9, 2.0))

    def test_extrapolation_capped_and_decays(self):
        t = Tracker(Limits())
        t.update(frame((0, 0, 1)), 0.0)
        t.update(frame((0.1, 0, 1)), 0.05)  # 2 m/s
        near, _, _ = t.predict(0.06)
        far, v, _ = t.predict(5.0)  # tracker stalled for 5 s
        self.assertGreater(near[0], 0.1)
        self.assertLess(far[0], 0.1 + 2.0 * (0.07 + 0.03) + 1e-9)
        self.assertLess(abs(v[0]), 0.01)

    def test_message_shape(self):
        m = pchi_message((0, 0, 1), (0, 0, 0), [0.0, 0.1, 0.2, 0.3])
        self.assertEqual((m["version"], m["type"]), ("2.0.0", "artist_tracking"))
        d = m["payload"]["data"]
        self.assertEqual(len(d["transform"]["position"]), 3)
        self.assertEqual(len(d["transform"]["rotation"]), 4)
        self.assertTrue(all(s in (-1, 1) for s in m["pir_invariants"]["equilibrium_check"]["sign_sequence"]))


if __name__ == "__main__":
    unittest.main()
