"""PCHI Studio for OBS: an OBS script (Tools > Scripts > +) that connects OBS to a running PCHI runtime.

PCHI already switches OBS scenes (an `obs` device in the rig, over obs-websocket). This is the other direction, from inside OBS:

  1. WATCH   it shows PCHI's current scene, what it is waiting for and what is on hold, in the script panel and,
             if you name a text source, on the picture itself (a confidence monitor on the multiview).
  2. CHECK   it reads which OBS scenes the show will ask for, compares them with the scenes this OBS really has, and tells
             PCHI whether the department is READY. A missing scene is named in the note. That is the point of the script:
             a cue to a scene that does not exist is found before doors, not at the cue.
  3. GO      optionally, one hotkey ("PCHI: send the next event") for the person at this desk. Off unless ticked. It is a
             person's action like a button in Studio; the runtime decides whether to accept it (edit key, GO gate).

It only talks to the runtime's HTTP API (stdlib urllib, no extra packages). It never switches the program scene itself: PCHI does
that through the rig. The pure functions are at the top and are tested without OBS; the OBS glue is at the bottom.

Written against OBS's Python scripting API from its documentation. NOT yet run inside a real OBS: see docs/obs-plugin.md.
"""

import json
import threading
import time
import urllib.error
import urllib.request

POLL_S = 2.0
TIMEOUT_S = 2.0


# ---- pure logic (no OBS needed) ------------------------------------------------------------------------------------------


def obs_devices(describe):
    """The rig's OBS devices: [{id, dept, label}]."""
    return [d for d in (describe.get("rig") or []) if d.get("type") == "obs"]


def scenes_the_show_asks_for(show, dept):
    """Every OBS scene name a cue in this show will switch to, for the given department: ['intro', 'main', ...] in order."""
    chart = show.get("definition") or show.get("showchart") or {}
    seen, out = set(), []

    def take(action):
        if not isinstance(action, dict) or action.get("kind") != "scene.activate":
            return
        p = action.get("payload") or {}
        v = next(iter(p.values()), None) if isinstance(p, dict) else None
        if isinstance(v, (str, int)) and str(v).strip() and str(v).strip() not in seen:
            seen.add(str(v).strip())
            out.append(str(v).strip())

    for st in chart.get("states", []):
        for a in (st.get("enter_actions") or []) + (st.get("exit_actions") or []):
            take(a)
        for lane in (st.get("timeline") or {}).get("lanes", []):
            if lane.get("dept") not in (dept, None, ""):
                continue
            for cue in lane.get("cues", []):
                take(cue.get("action"))
    return out


def check_scenes(needed, have):
    """(ready, note). READY only if every scene the show asks for exists in OBS."""
    missing = [s for s in needed if s not in set(have)]
    if not needed:
        return True, "the show asks OBS for no scenes"
    if missing:
        return False, ("missing in OBS: " + ", ".join(missing))[:200]
    return True, f"all {len(needed)} scenes the show uses exist in OBS"[:200]


def status_line(describe):
    """One short text for the panel / a text source."""
    now = describe.get("now") or {}
    parts = [f"PCHI  {now.get('scene', '?')}"]
    if now.get("held_for_ms"):
        parts.append(f"held {now['held_for_ms'] // 1000}s")
    wait = describe.get("waiting_for_a_person") or []
    if wait:
        parts.append(f"{len(wait)} for a person")
    outs = [w.get("event") for w in (now.get("ways_out") or []) if w.get("would_fire_now")]
    if outs:
        parts.append("next: " + outs[0])
    return "  |  ".join(parts)


def next_event(describe):
    """The first way out of the current scene that would fire right now, or None."""
    for w in (describe.get("now") or {}).get("ways_out") or []:
        if w.get("would_fire_now") and w.get("event"):
            return w["event"]
    return None


class Pchi:
    """The runtime's HTTP API, as much of it as this script uses."""

    def __init__(self, addr, token="", edit_key="", actor="person:obs"):
        self.addr, self.token, self.edit_key, self.actor = addr.strip().rstrip("/"), token, edit_key, actor

    def _call(self, method, path, body=None):
        url = (self.addr if "://" in self.addr else "http://" + self.addr) + path
        headers = {"Content-Type": "application/json", "X-Pchi-Actor": self.actor}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        if self.edit_key:
            headers["X-Pchi-Edit-Key"] = self.edit_key
        req = urllib.request.Request(url, method=method, headers=headers, data=None if body is None else json.dumps(body).encode())
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT_S) as r:
                return r.status, json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            try:
                return e.code, json.loads(e.read() or b"{}")
            except Exception:
                return e.code, {}

    def describe(self):
        st, v = self._call("GET", "/describe")
        if st != 200:
            raise RuntimeError(f"/describe answered {st}")
        return v

    def show(self):
        st, v = self._call("GET", "/state")  # the live state carries the chart's definition
        if st != 200:
            raise RuntimeError(f"/state answered {st}")
        return v

    def ready(self, dept, ready, note):
        return self._call("POST", f"/ready/{dept}", {"ready": ready, "note": note})

    def send(self, event):
        return self._call("POST", "/input", {"event": event})


class Link:
    """One round: read PCHI, compare with this OBS's scenes, report. Returns what to show. Never raises."""

    def __init__(self, pchi):
        self.pchi = pchi
        self.last_report = None  # (dept, ready, note): only a change is sent, a heartbeat is sent every 10 s
        self.last_sent = 0.0

    def round(self, obs_scenes):
        try:
            d = self.pchi.describe()
        except Exception as e:  # noqa: BLE001
            return {"ok": False, "text": f"PCHI unreachable ({e})", "report": "", "describe": {}}
        report = "no OBS device in the rig"
        try:
            for dev in obs_devices(d):
                dept = dev.get("dept") or "visual"
                ready, note = check_scenes(scenes_the_show_asks_for(self.pchi.show(), dept), obs_scenes)
                key = (dept, ready, note)
                if key != self.last_report or time.time() - self.last_sent > 10:
                    st, _ = self.pchi.ready(dept, ready, note)
                    if st == 200:
                        self.last_report, self.last_sent = key, time.time()
                report = f"{dept}: {'READY' if ready else 'NOT READY'} ({note})"
        except Exception as e:  # noqa: BLE001
            report = f"could not report readiness ({e})"
        return {"ok": True, "text": status_line(d), "report": report, "describe": d}


# ---- OBS glue ------------------------------------------------------------------------------------------------------------

try:
    import obspython as obs  # only exists inside OBS
except ImportError:  # imported by the tests
    obs = None

_cfg = {"addr": "127.0.0.1:3100", "token": "", "key": "", "source": "", "go_hotkey": False}
_state = {"text": "starting", "report": "", "describe": {}}
_lock = threading.Lock()
_stop = threading.Event()
_hotkey_id = None


def _scene_names():
    names = obs.obs_frontend_get_scene_names() or []
    return list(names)


def _worker():
    link = Link(Pchi(_cfg["addr"], _cfg["token"], _cfg["key"]))
    scenes = []
    while not _stop.is_set():
        with _lock:
            scenes = list(_state.get("scenes", []))
        out = link.round(scenes)
        with _lock:
            _state.update(text=out["text"], report=out["report"], describe=out["describe"])
        _stop.wait(POLL_S)


def _tick():
    """Runs on OBS's own thread: the only place that touches OBS objects."""
    with _lock:
        _state["scenes"] = _scene_names()
        text = _state["text"]
    name = _cfg["source"]
    if name:
        src = obs.obs_get_source_by_name(name)
        if src is not None:
            s = obs.obs_data_create()
            obs.obs_data_set_string(s, "text", text)
            obs.obs_source_update(src, s)
            obs.obs_data_release(s)
            obs.obs_source_release(src)


def _on_hotkey(pressed):
    if not pressed or not _cfg["go_hotkey"]:
        return
    d = _state.get("describe") or {}
    ev = next_event(d)
    if ev:
        threading.Thread(target=lambda: Pchi(_cfg["addr"], _cfg["token"], _cfg["key"]).send(ev), daemon=True).start()


def script_description():
    return "<b>PCHI Studio</b><br>Shows PCHI's state in OBS, and tells PCHI whether the OBS scenes the show needs exist.<br>PCHI itself switches the scenes."


def script_properties():
    p = obs.obs_properties_create()
    obs.obs_properties_add_text(p, "addr", "PCHI runtime (host:port)", obs.OBS_TEXT_DEFAULT)
    obs.obs_properties_add_text(p, "token", "Runtime token (if set)", obs.OBS_TEXT_PASSWORD)
    obs.obs_properties_add_text(p, "key", "Your edit key (only for the GO hotkey)", obs.OBS_TEXT_PASSWORD)
    obs.obs_properties_add_text(p, "source", "Text source to show PCHI's state in (optional)", obs.OBS_TEXT_DEFAULT)
    obs.obs_properties_add_bool(p, "go_hotkey", "Enable the 'PCHI: send the next event' hotkey (a person's GO)")
    obs.obs_properties_add_text(p, "readout", "Last report", obs.OBS_TEXT_INFO)
    return p


def script_defaults(s):
    obs.obs_data_set_default_string(s, "addr", "127.0.0.1:3100")


def script_update(s):
    _cfg.update(addr=obs.obs_data_get_string(s, "addr"), token=obs.obs_data_get_string(s, "token"), key=obs.obs_data_get_string(s, "key"),
                source=obs.obs_data_get_string(s, "source"), go_hotkey=obs.obs_data_get_bool(s, "go_hotkey"))


def script_load(s):
    global _hotkey_id
    _hotkey_id = obs.obs_hotkey_register_frontend("pchi.next", "PCHI: send the next event", _on_hotkey)
    saved = obs.obs_data_get_array(s, "pchi.next")
    obs.obs_hotkey_load(_hotkey_id, saved)
    obs.obs_data_array_release(saved)
    _stop.clear()
    threading.Thread(target=_worker, daemon=True).start()
    obs.timer_add(_tick, 1000)


def script_save(s):
    arr = obs.obs_hotkey_save(_hotkey_id)
    obs.obs_data_set_array(s, "pchi.next", arr)
    obs.obs_data_array_release(arr)


def script_unload():
    _stop.set()
    obs.timer_remove(_tick)
