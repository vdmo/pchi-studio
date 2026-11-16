# OBS and PCHI Studio: the plugin demo

PCHI already drives OBS: an `obs` device in the rig takes `scene.activate` cues and makes that OBS scene the program, over obs-websocket. This page covers the **other direction**: a script that runs *inside OBS* and connects it to a running runtime.

```
        cue "Slides"                 obs-websocket
PCHI runtime ─────────────────────────────────────────► OBS  (switches the program scene)
      ▲                                                   │
      │   GET /describe, /state     POST /ready/visual    │   tools/obs-pchi/pchi_obs.py
      └───────────────────────────────────────────────────┘   (runs inside OBS)
```

## What the script does

| | |
|---|---|
| **Watch** | Shows PCHI's current scene, how long it has been held, what waits for a person, and the next event that would fire. In the script panel, and in a **text source** you name, so it can sit on the multiview as a confidence monitor. |
| **Check** | Reads every OBS scene name the show's cues will ask for, compares them with the scenes this OBS has, and tells PCHI whether the department is **READY**. A missing scene is named in the note ("missing in OBS: Slides"). A cue to a scene that does not exist is found before doors, not at the cue. |
| **GO hotkey** | Optional, off by default: one OBS hotkey, "PCHI: send the next event". It is a *person's* action, like the GO button in Studio; the runtime decides (edit key, GO gate). |

It never switches the program scene itself. PCHI does that through the rig, so there is one place that cuts the picture and one record of it.

It identifies itself as `person:obs`. It does not claim to be an agent, and it does not try to approve, arm or edit anything.

## Run the demo

1. **OBS:** make three scenes named `Starting Soon`, `Host`, `Slides`. In *Tools → WebSocket Server Settings* enable the server and note the port (usually 4455) and the password. Optionally add a text source called `PCHI status` to any scene.
2. **Runtime:** edit the `obs-1` node in `tools/obs-pchi/demo/obs-demo.pchi` if your port is not 4455, set the password with the environment variable (not the file), and start it sending:

   ```bash
   export PCHI_OBS_PASSWORD='your OBS websocket password'
   pchi-studio-api tools/obs-pchi/demo/obs-demo.pchi --bind 127.0.0.1:3100 --send
   ```

3. **Studio:** open it (`npm run dev` in `pchi-studio-react`) and look at the Rig tab: `OBS (stream)` should show as linked.
4. **Plugin:** OBS *Tools → Scripts → +* and choose `tools/obs-pchi/pchi_obs.py`. (OBS needs a Python install set under the *Python Settings* tab.) Fill in the runtime address `127.0.0.1:3100` and the text source name.
5. **Watch it check:** rename the `Slides` scene in OBS. Within a couple of seconds the Live tab shows the `visual` department **not ready** with the note `missing in OBS: Slides`; rename it back and it turns ready.
6. **Run it:** press GO for `LIVE` in Studio (or the OBS hotkey, if you ticked it). OBS cuts to `Host`; `SLIDES` cuts to `Slides`; the text source follows each step.

## Tests

```bash
cd tools/obs-pchi
python3 -m unittest test_pchi_obs                       # a fake runtime, and a stand-in obspython
LIVE=127.0.0.1:3190 python3 -m unittest test_pchi_obs.Live   # against a real pchi-studio-api serving demo/obs-demo.pchi
```

What these cover: reading the scene names from the chart, the ready/not-ready decision and its note, that an unchanged answer is not re-sent, that it says who it is, that an unreachable runtime is a status line not a crash, the real runtime's readiness changing as scenes appear, and the script's OBS entry points run against a stand-in `obspython` (text source written, hotkey sends the next event only when ticked).

## What is not proven

- **It has not been run inside a real OBS.** The OBS calls are written from OBS's scripting documentation and exercised only against a stand-in module. Expect to find a call that behaves differently, and fix it in the part below the "OBS glue" line; the logic above it is tested.
- The **runtime → OBS** half (cue switches scene, password proof) has only been run against a fake obs-websocket server written from the protocol document ([runtime-reference.md](runtime-reference.md)).
- The scene check compares **names only**. It does not know whether a scene has the right sources, or whether the stream key works.
- The script polls every 2 s and reports readiness every 10 s at most when nothing changes. It is a monitor, not a hard real-time link.
- The password for obs-websocket is the runtime's concern (`PCHI_OBS_PASSWORD`); the script's own token and edit key are stored in OBS's script settings in clear text. Use a key with the narrowest role you have.

## Related

[studio-guide.md](studio-guide.md) (Rig and Live tabs), [security.md](security.md) (keys and roles), [show-file.md](show-file.md) (device parameters).
