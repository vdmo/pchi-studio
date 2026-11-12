# Running the bridge against a real Strobe

The bridge has only been tested against `fake_strobe.py`, which implements Strobe's OSC addresses *as documented in its source*. This checklist finds what that simulation hid. Allow ~30 minutes. Write down anything that differs from "Expect" — each difference is a real finding.

## 0. Setup

- Windows machine with Strobe ([release zip](https://github.com/lnorton89/strobe/releases), run `strobe.exe`).
- The PCHI Conductor and the bridge can run on the same Windows machine or in WSL. **WSL2 caveat:** WSL2 has its own network. A Windows Strobe sends feedback to `127.0.0.1`, which is *Windows' loopback*, not WSL's. Simplest: run everything on Windows, or run the conductor and bridge in WSL and use WSL's mirrored networking (`networkingMode=mirrored` in `.wslconfig`). If unsure, start with step 1's loopback test.
- Python 3 with `pip install python-osc websocket-client`; Rust for the conductor.
- Copy `demos/strobe-show/strobe_show.json` and edit `states` so each `cue` is a **real cue number in a playlist you have loaded** in Strobe (cues count from 1). `forward_controls` should name a real control of a scene you'll play (Strobe matches names ignoring case, `_`, `-`, spaces).

## 1. Prove OSC reaches the bridge (before any PCHI)

In Strobe: **Mappings → OSC**: enable, port `9000`, enable **feedback**, host `127.0.0.1`, port `9001`.

```bash
python3 -c "
from pythonosc.dispatcher import Dispatcher
from pythonosc.osc_server import BlockingOSCUDPServer
d = Dispatcher(); d.set_default_handler(lambda a,*r: print(a, r))
BlockingOSCUDPServer(('127.0.0.1', 9001), d).serve_forever()"
```

Load a playlist and play it. **Expect:** lines like `/strobe/state/blackout (0,)`, `/strobe/state/cue (1,)`, `/strobe/control/<name> (0.5,)`, then updates when you move a slider or step the playlist.

- Nothing arrives → feedback is off, wrong port, or the WSL2 network issue above.
- Arrives but cue is always `0` → you're not in a playlist; `/strobe/state/cue` is 0 outside one. Load one.

Stop the listener (it holds port 9001).

## 2. Strobe → PCHI

```bash
cargo run -p primeswarm-pchi -- --rules demos/strobe-show/strobe-show-rules.only-pchi
python3 tools/strobe-pchi-bridge/strobe_pchi_bridge.py --config <your config> -v
# open http://127.0.0.1:3000/mission-control
```

| Do | Expect |
|---|---|
| Start the bridge before touching Strobe | `visuals` appears as **Not ready — waiting for Strobe feedback** |
| Change anything in Strobe | `visuals` → **Ready** |
| Step the playlist to a mapped cue | **Current state** follows; history shows `via strobe_bridge` |
| Step to an unmapped cue | Bridge logs "not mapped to any state"; state does not change |
| Press **B** (blackout) | `visuals` → **Degraded: blackout active**; press again → Ready |
| Quit Strobe | **Nothing changes** (known limit: no Strobe heartbeat) — confirm, and note how long you'd have taken to notice |
| Stop the bridge | `visuals` → **Offline** after ~15 s |

## 3. PCHI → Strobe

```bash
python3 tools/pchi-show/pchi_show.py enter <a state in your config>
```

**Expect:** Strobe jumps to that cue; Mission Control shows the state once (no flapping back and forth). Try a state with no cue: bridge logs it, Strobe doesn't move.

Check there is no echo loop: watch the bridge log for ~10 s after the jump; it should not keep sending `/strobe/cue/N`.

## 4. Governance

Move the forwarded control above 0.8 (the demo rule's limit, in Strobe's normalised 0–1). **Expect:** a denial in Mission Control and lower confidence for 60 s. Strobe's control is **not** changed (denials aren't enforced at the tool yet).

Then check a gate: with `strobe-show-rules.only-pchi`, `pchi_show.py enter FINALE` is refused until lasers are ready (`pchi_show.py ready lasers ready`). Make `FINALE` a state with a cue, step Strobe into that cue by hand while lasers are not ready, and **expect Strobe to be sent back** to the previous cue within a moment.

## 5. Things I could not check from here — please look at these

- Does Strobe act on `/strobe/cue/N` sent with the integer argument `1`? (The source says "none or 1".) Does a cue jump *restart* the playlist timing, or fight an auto-advance?
- When Strobe's playlist auto-advances, is `/strobe/state/cue` sent promptly, and is the number the cue's position from 1 as assumed?
- Transition/crossfade: is feedback sent at the start or the end of a crossfade? (Affects when the shared state changes.)
- With OSC bound to loopback only, can the bridge on another machine reach it? (Needs "all interfaces" in Strobe's OSC settings.)
- Windows Firewall prompts on first run; allow private networks only.

## Report back

For each step: what happened, and the bridge's `-v` log. The most useful single artefact is the output of step 1.
