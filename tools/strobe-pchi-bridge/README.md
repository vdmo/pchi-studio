# Strobe PCHI Bridge

Connects [Strobe](https://github.com/lnorton89/strobe) — a Synesthesia-style VJ engine in Rust — to the PCHI Conductor's **shared show state**.

```
Strobe --OSC feedback--> this bridge --raw JSON UDP--> PCHI Conductor
PCHI Conductor --WebSocket--> this bridge --OSC--> Strobe
```

Strobe was chosen as the first integration because its control surface is documented and open: a built-in OSC address space (`/strobe/cue/<n>`, `/strobe/control/<name>`, …) and OSC *feedback* on the same addresses, so the bridge needs no plugin inside Strobe.

## What it does

| Direction | Strobe | PCHI |
|---|---|---|
| → PCHI | `/strobe/state/cue N` | `show_state` `enter` for the state mapped to cue N |
| → PCHI | `/strobe/state/blackout B` | `visuals` readiness: `degraded` ("blackout active") or `ready` |
| → PCHI | first feedback of any kind | `visuals` readiness: `ready` (before that: `not_ready`, "waiting for Strobe feedback") |
| → PCHI | `/strobe/control/<name> v` | `control_parameter strobe.<name>` — only names in `forward_controls`, so Only-Lang rules can govern them |
| ← PCHI | `show_state` `enter` from another source | `/strobe/cue/N` |
| ← PCHI | `show_state` `rejected` for a state *Strobe* moved into | `/strobe/cue/N` for the state the show is actually in (Strobe is put back) |

On start it also announces the show's `plan` (the ordered list of states from the config). Transitions are announced once: re-entering the current state is a no-op on both sides, which is what stops Strobe → PCHI → Strobe echo loops.

## Config

`demos/strobe-show/strobe_show.json`:

```json
{
  "show": "Strobe x PCHI demo",
  "department": "visuals",
  "forward_controls": ["strobe_intensity"],
  "states": [ { "state": "PRESHOW", "cue": 1 }, { "state": "BUILD", "cue": 3 } ]
}
```

Cues count from 1 (Strobe's convention); cue 0 means "none". Each state and each cue may appear once.

## Run

```bash
pip install python-osc websocket-client
python strobe_pchi_bridge.py --config ../../demos/strobe-show/strobe_show.json
```

In Strobe: **Mappings → OSC on** (port 9000), **feedback on**, host `127.0.0.1`, port `9001` — Strobe's defaults, which are this bridge's defaults. Strobe's OSC binds to loopback unless you choose otherwise, so run the bridge on the same machine, or enable "all interfaces" in Strobe and pass `--strobe-host`.

No Strobe handy? `fake_strobe.py` speaks the same addresses; `demos/strobe-show/run_demo.sh` uses it.

## Trying it on a real Strobe

See [REAL_STROBE_CHECKLIST.md](REAL_STROBE_CHECKLIST.md): a step-by-step run with what to observe and what to report back.

## Tests

```bash
python3 -m unittest test_strobe_pchi_bridge.py -v
```

## What this does not do — read before relying on it

- **Not tested against a running Strobe.** Strobe is a Windows/GPU app that wasn't available. The OSC addresses and argument types come from Strobe's own source (`crates/strobe-io/src/osc.rs`, `feedback.rs`). Please report mismatches.
- **"Ready" means "Strobe is talking to us", not "the projector is lit".** Strobe sends feedback only on change and has no heartbeat, so the bridge can't see Strobe die. It heartbeats for *itself*; if the bridge stops, `visuals` goes `offline` after 15 s.
- **A denial does not yet change anything in Strobe.** The rule engine attributes a `DENY` to `scene_state`, not to the offending control, so the bridge can't know which control to pull back. Today a denial is visible (Mission Control, the signed log), not enforced. Enforcement needs the rule engine to name its target first.
- Strobe's feedback sends at most 30 Hz and only changed values, so `forward_controls` is cheap, but values are Strobe's normalised 0–1, not the scene's native range — write rules against 0–1.
- Strobe's [licence](https://github.com/lnorton89/strobe/blob/master/LICENSE) is MIT + Commons Clause. This bridge only talks OSC to it and contains none of its code.
