import math
import unittest

import pear_tap
from rgbmocap_pchi_bridge import Limits, Tracker, clamp_stage, pchi_message


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
