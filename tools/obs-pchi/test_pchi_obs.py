"""Tests for pchi_obs.py without OBS. Run: python3 -m unittest tools/obs-pchi/test_pchi_obs.py

Unit tests use a small fake runtime. LIVE=host:port also runs them against a real `pchi-studio-api` serving demo/obs-demo.pchi.
"""
import http.server
import json
import os
import threading
import unittest

import pchi_obs as p

SHOW = json.load(open(os.path.join(os.path.dirname(__file__), "demo", "obs-demo.pchi")))
DESCRIBE = {"rig": [{"id": "obs-1", "type": "obs", "dept": "visual"}, {"id": "x", "type": "x32", "dept": "audio"}],
            "now": {"scene": "STARTING_SOON", "held_for_ms": 0, "ways_out": [{"event": "LIVE", "would_fire_now": True}]}, "waiting_for_a_person": []}


class Fake(http.server.BaseHTTPRequestHandler):
    calls = []

    def log_message(self, *a):
        pass

    def _send(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        Fake.calls.append(("GET", self.path, self.headers.get("X-Pchi-Actor")))
        self._send(200, DESCRIBE if self.path == "/describe" else {"definition": SHOW["showchart"]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        Fake.calls.append(("POST", self.path, body))
        self._send(200, {"ok": True})


class Base(unittest.TestCase):
    def setUp(self):
        Fake.calls = []
        self.srv = http.server.HTTPServer(("127.0.0.1", 0), Fake)
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.addr = "127.0.0.1:%d" % self.srv.server_port

    def tearDown(self):
        self.srv.shutdown()
        self.srv.server_close()


class T(Base):

    def test_reads_the_scenes_the_show_asks_for_in_order(self):
        self.assertEqual(p.scenes_the_show_asks_for({"definition": SHOW["showchart"]}, "visual"), ["STARTING_SOON", "HOST", "SLIDES"])
        self.assertEqual(p.scenes_the_show_asks_for({"definition": SHOW["showchart"]}, "audio"), [], "another department's lane is not OBS's business")

    def test_ready_only_if_every_scene_exists_and_the_missing_one_is_named(self):
        self.assertEqual(p.check_scenes(["A", "B"], ["A", "B", "C"])[0], True)
        ok, note = p.check_scenes(["A", "SLIDES"], ["A"])
        self.assertFalse(ok)
        self.assertIn("SLIDES", note)
        self.assertTrue(p.check_scenes([], [])[0])

    def test_status_and_next_event(self):
        self.assertIn("STARTING_SOON", p.status_line(DESCRIBE))
        self.assertIn("next: LIVE", p.status_line(DESCRIBE))
        self.assertEqual(p.next_event(DESCRIBE), "LIVE")
        self.assertIsNone(p.next_event({"now": {"ways_out": [{"event": "X", "would_fire_now": False}]}}))

    def test_a_round_reports_not_ready_for_a_missing_scene_then_ready_once_it_exists(self):
        link = p.Link(p.Pchi(self.addr))
        out = link.round(["STARTING_SOON", "HOST"])
        self.assertTrue(out["ok"])
        self.assertIn("NOT READY", out["report"])
        posts = [c for c in Fake.calls if c[0] == "POST"]
        self.assertEqual(posts[-1][1:], ("/ready/visual", {"ready": False, "note": "missing in OBS: SLIDES"}))
        link.round(["STARTING_SOON", "HOST"])
        self.assertEqual(len([c for c in Fake.calls if c[0] == "POST"]), 1, "an unchanged answer is not sent again straight away")
        out = link.round(["STARTING_SOON", "HOST", "SLIDES"])
        self.assertIn("READY", out["report"])
        self.assertEqual([c for c in Fake.calls if c[0] == "POST"][-1][2]["ready"], True)

    def test_it_says_who_it_is_and_never_claims_to_be_an_agent(self):
        p.Link(p.Pchi(self.addr)).round([])
        self.assertTrue(all(c[2] == "person:obs" for c in Fake.calls if c[0] == "GET"))

    def test_an_unreachable_runtime_is_a_status_not_a_crash(self):
        out = p.Link(p.Pchi("127.0.0.1:9")).round(["A"])
        self.assertFalse(out["ok"])
        self.assertIn("unreachable", out["text"])


class FakeObs:
    """Just enough of obspython to run the script's own entry points. Not OBS: it proves the glue's calls make sense, no more."""
    OBS_TEXT_DEFAULT = OBS_TEXT_PASSWORD = OBS_TEXT_INFO = 0

    def __init__(self):
        self.text, self.timers, self.props = None, [], []

    def obs_properties_create(self): return self
    def obs_properties_add_text(self, p, name, label, kind): self.props.append(name)
    def obs_properties_add_bool(self, p, name, label): self.props.append(name)
    def obs_hotkey_register_frontend(self, *a): return 1
    def obs_data_get_array(self, *a): return object()
    def obs_hotkey_load(self, *a): pass
    def obs_data_array_release(self, *a): pass
    def timer_add(self, fn, ms): self.timers.append(fn)
    def timer_remove(self, fn): self.timers.remove(fn)
    def obs_frontend_get_scene_names(self): return ["STARTING_SOON", "HOST", "SLIDES"]
    def obs_get_source_by_name(self, n): return self if n == "PCHI status" else None
    def obs_data_create(self): return {}
    def obs_data_set_string(self, d, k, v): d[k] = v
    def obs_source_update(self, src, d): self.text = d["text"]
    def obs_data_release(self, *a): pass
    def obs_source_release(self, *a): pass
    def obs_data_get_string(self, s, k): return s[k]
    def obs_data_get_bool(self, s, k): return s[k]
    def obs_hotkey_save(self, *a): return object()
    def obs_data_set_array(self, *a): pass


class Glue(Base):
    def test_the_script_runs_in_a_stand_in_obs_and_writes_status_to_a_text_source(self):
        fo = FakeObs()
        p.obs = fo
        try:
            p._stop.clear()
            p.script_update({"addr": self.addr, "token": "", "key": "", "source": "PCHI status", "go_hotkey": True})
            p.script_properties()
            self.assertEqual(fo.props, ["addr", "token", "key", "source", "go_hotkey", "readout"])
            p.script_load({})
            p._tick()  # gives the worker the scene names
            for _ in range(80):  # the first round may run before OBS lists its scenes: wait for the READY report
                if [c for c in Fake.calls if c[0] == "POST" and c[2].get("ready")]:
                    break
                threading.Event().wait(0.1)
            p._tick()
            self.assertIn("STARTING_SOON", fo.text or "")
            self.assertTrue([c for c in Fake.calls if c[0] == "POST" and c[1] == "/ready/visual" and c[2]["ready"]])
            p._on_hotkey(True)
            threading.Event().wait(0.5)
            self.assertIn(("POST", "/input", {"event": "LIVE"}), Fake.calls)
            p._cfg["go_hotkey"] = False
            n = len(Fake.calls)
            p._on_hotkey(True)
            threading.Event().wait(0.3)
            self.assertEqual(len(Fake.calls), n, "with the hotkey off, nothing is sent")
            p.script_unload()
        finally:
            p._stop.set()
            p.obs = None


@unittest.skipUnless(os.environ.get("LIVE"), "set LIVE=host:port to run against a real runtime")
class Live(unittest.TestCase):
    def test_against_the_real_runtime(self):
        addr = os.environ["LIVE"]
        link = p.Link(p.Pchi(addr))
        bad = link.round(["STARTING_SOON", "HOST"])
        self.assertTrue(bad["ok"], bad)
        self.assertIn("NOT READY", bad["report"])
        self.assertEqual(p.Pchi(addr).describe()["readiness"]["visual"]["ready"], False)
        good = link.round(["STARTING_SOON", "HOST", "SLIDES"])
        self.assertIn("READY", good["report"])
        r = p.Pchi(addr).describe()["readiness"]["visual"]
        self.assertTrue(r["ready"] and r["source"] == "http", r)
        self.assertEqual(p.next_event(p.Pchi(addr).describe()), "LIVE")
        self.assertIn("STARTING_SOON", good["text"])


if __name__ == "__main__":
    unittest.main()
