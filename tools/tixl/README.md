# TiXL as a rig device

PCHI Studio's runtime can send scene cues to [TiXL](https://github.com/tixl3d/tixl) (v4.3.0.4 was the release at the time of writing; Windows only) over OSC. TiXL receives OSC through its **OscInput** operator, so **TiXL has no fixed control addresses: you build the graph that reads them.** This folder gives you the sending side and a way to check it.

> This is separate from `tools/tixl-pchi-bridge` and `tools/tixl-native-operators`, which connect TiXL to the older PCHI *Conductor* protocol. This one is the Studio **runtime** driving TiXL as a device in the rig.

| File | What |
|---|---|
| `profiles/tixl.json` | the `tixl` device profile (how a `scene.activate` cue becomes OSC) |
| `demo/tixl-demo.pchi` | a small show: INTRO → DROP → OUTRO, cues by scene number and by name |
| `osc_listen.py` | a stand-in for TiXL's OscInput that prints what PCHI sends |

## What PCHI sends

For a `scene.activate` cue to a `tixl` device:

| Cue value | OSC message |
|---|---|
| `3` (a number) | `/pchi/scene` with the float `3.0` |
| `INTRO` (a name) | `/pchi/scene/INTRO` with the float `1.0` |
| `/my/address` | exactly that address, float `1.0` |

Numbers go out as floats because TiXL's OscInput exposes float values. A name sends one address per scene, so a graph can watch the one it cares about.

## Check it without TiXL

```bash
python3 tools/tixl/osc_listen.py 9000                                         # terminal 1
pchi-studio-api tools/tixl/demo/tixl-demo.pchi --profiles tools/tixl/profiles --send   # terminal 2
curl -X POST localhost:3100/input -H 'Content-Type: application/json' -d '{"event":"DROP"}'
```

The listener prints `/pchi/scene [1.0]`, `/pchi/scene/INTRO [1.0]`, then the DROP ones. (This was run: see the test notes below.)

## Connect real TiXL

1. Install TiXL and open a project.
2. Add an **OscInput** operator. Set its **Port** to the port in the show's `tixl-1` node (9000 in the demo; the port is yours to choose, nothing is assumed). Leave **Address** empty to collect everything, or filter it.
3. Start the runtime with `--profiles tools/tixl/profiles --send`, and make `tixl-1`'s `host` the machine TiXL runs on.
4. Send an event (as above, or press GO in Studio's Live tab). In TiXL, OscInput should list the addresses `/pchi/scene` and `/pchi/scene/...` once they arrive. TiXL's own OSC help page covers the log-messages option for seeing incoming messages.
5. Wire OscInput's values into your graph, for example the `/pchi/scene` float into a switch that picks which scene to show.

**If the runtime runs in WSL and TiXL on Windows:** use WSL's gateway address as `host` (find it with `ip route | awk '/default/{print $3}'` in WSL), and allow UDP on that port through Windows Firewall.

## Prove it on the real tool

All three rules are marked `memory`, not `vendor`: nothing here has been run against a running TiXL, and TiXL's own documentation does not spell out the OscInput address and value mapping. The Doors check says so. Once you have seen TiXL react correctly, run `pchi-gearcheck tools/tixl/demo/tixl-demo.pchi --only tixl` and note the result in each rule's `proven` field (see `docs/profiles.md`).

UDP has no acknowledgement and OscInput does not answer, so PCHI can say "sent", not "TiXL did it". The device shows no link; only a test or a READY report from a person shows TiXL is there.

## What is not proven

- Against a real TiXL: nothing. The wire format was checked with a listener written for this, not with TiXL.
- The exact address-matching and value rules of OscInput (the earlier bridge notes describe it as float-only; confirm in your TiXL version).
- Other cue kinds (lighting, audio) are not mapped: TiXL is a visuals tool here.
