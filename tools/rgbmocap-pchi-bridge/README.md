# RGBMoCap → PCHI bridge (prototype)

Turns a live body-tracking stream from [toyxyz_RGBMocap](https://github.com/toyxyz/toyxyz_RGBMocap) into PCHI v2.0 `artist_tracking` messages for the Conductor.

**Status: prototype. Unit-tested on synthetic frames only. Not run against a camera, the real tracker, a Conductor, or any laser, DMX or audio hardware.**

```
toyxyz_RGBMocap (live worker) --UDP JSON--> this bridge --UDP JSON artist_tracking--> PCHI Conductor
        (tap: not written yet)            127.0.0.1:9100                              127.0.0.1:8888
```

## Run

```
python rgbmocap_pchi_bridge.py --listen 127.0.0.1:9100 --pchi 127.0.0.1:8888 --rate 60
python test_bridge.py
python replay.py --synthetic     # sway, arm raises, a glitch, an occlusion; no camera needed
```

Input frame: `{"confidence": 0.9, "joints": {"pelvis": [x, y, z]}}` in metres, stage-relative, z up.

## What it does

- Receiver and emitter are separate loops: the tracker may run at 15–30 Hz while output is a steady `--rate` Hz.
- Dead reckoning with a capped horizon (70 ms) plus a small look-ahead (`--lead`); velocity decays when frames go stale or confidence drops, so a stalled tracker coasts to a stop.
- Slew limit (`--slew`, m/s) so a single-frame pose flip cannot teleport the point; cylinder clamp (`--radius`, height 0–2.8 m); NaN, wrong-length and malformed frames are dropped and counted.

## Venue rules (second check, in the Conductor)

`rgbmocap-stage.only-pchi` makes the Conductor itself flag a performer outside the stage footprint (box of ±1.5 m, height 0 to 2.8 m). Run it with `primeswarm-pchi --rules rgbmocap-stage.only-pchi`, and `check_rules.py` exercises it.

Tested against a locally built Conductor: in-bounds positions raise nothing, an out-of-bounds position signs a DENY receipt, and so does every later message while that position is still the stored one, until an in-bounds position arrives.

**A DENY here is an alarm, not a gate.** The Conductor stores the position before it evaluates the rules, so the bad value is already in its state. Do not treat "the Conductor accepted it" as "it was in bounds".
I did not run `scripts/verify_governance_chain.py` on the receipts (it needs `pynacl`, which I did not install), so the signature chain is unchecked.

## What it does not do (read before using on a stage)

- **It is not laser or DMX safety.** Keep hardware interlocks, scan-fail zones and an E-stop independent of this process. Clamping here only smooths.
- **The tap (`pear_tap.py`) is written from reading PEAR's public source, never run inside it.** PEAR's worker sends each frame over a private pipe to the GUI process; the hook point is `Session.read()` in `mocap/pear/session.py` (usage is in the file's docstring). Frames there are already through PEAR's One Euro filter. It needs a one-line edit to your own copy of the tool. That repository has no declared licence, so this folder contains no code from it; do not republish their files.
- **PEAR gives no measured confidence** (its docs say its weights are heuristics). The tap sends 1.0 for a tracked frame and 0.0 when the person is lost, so the bridge's occlusion handling is on/off, not graded.
- **Unverified in the tap:** the joint indices (assumed SMPL-X order: pelvis 0, head 15, wrists 20 and 21; I found no name list), the axis mapping and handedness (PEAR output is treated as Y-up), the units (metres only if camera alignment succeeds), and the origin (PEAR's origin is not your stage centre). Check all of them with a recorded video before use.
- Its built-in network output is Unreal Live Link, which this bridge does not use.
- **Checked against a real Conductor (pchi-studio `primeswarm-pchi`, built and run locally):** it listens for JSON on UDP 127.0.0.1:8888 and parsed and accepted every `artist_tracking` and `control_parameter` message in two replays (the first 78 position messages, then a scripted 8 s performance: 228 of each of the three parameters) with no warnings or drops. The Conductor stores position and velocity but does not expose them on `/state`, so I confirmed receipt from its log only, not the stored values. Governance fired no rules. Casing is snake_case (confirmed from its Rust types).
- **Your show rules can react to this data.** The default rule file has `if artist_tracking.position.z < 0.0 then evolve(...)`; the bridge clamps z to 0 and above and treats z as up, so check that your venue rules agree on axes.
- Position and velocity go out as `artist_tracking` (pelvis only). Wrists, if the tap sends them, add three `control_parameter` messages at `--param-rate` (default 20 Hz) with target `performer_01`: `rgbmocap.spread` (wrist distance / 1.8 m), `rgbmocap.hand_elevation` (higher wrist, 0.5 to 2.2 m mapped to 0..1) and `rgbmocap.energy` (speed / 5 m/s), each clamped to 0..1. Those scalings are my guesses, not tuned. The Conductor stores them as `performer_01.rgbmocap.<name>`. Head and the other joints are still unused.
- The invariants block sends residual 0 (a single position has no balanced set to check; the Conductor only checks residual on scene_update and warns on non-zero values otherwise). coherence_gap and curvature_signature are descriptive only. Nothing in them is a safety check.
- Latency and frame-rate figures in earlier discussion (30–80 ms inference, filter lag) are estimates, not measurements.
- A "256-byte payload" with `AudioSpatialPayload`/`VisualPayload`, and the KoroNoto, PixLiner and LUZIA consumers, appear nowhere in this workspace's code; they are not implemented here.
