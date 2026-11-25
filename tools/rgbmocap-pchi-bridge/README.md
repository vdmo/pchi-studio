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
```

Input frame: `{"confidence": 0.9, "joints": {"pelvis": [x, y, z]}}` in metres, stage-relative, z up.

## What it does

- Receiver and emitter are separate loops: the tracker may run at 15–30 Hz while output is a steady `--rate` Hz.
- Dead reckoning with a capped horizon (70 ms) plus a small look-ahead (`--lead`); velocity decays when frames go stale or confidence drops, so a stalled tracker coasts to a stop.
- Slew limit (`--slew`, m/s) so a single-frame pose flip cannot teleport the point; cylinder clamp (`--radius`, height 0–2.8 m); NaN, wrong-length and malformed frames are dropped and counted.

## What it does not do (read before using on a stage)

- **It is not laser or DMX safety.** Keep hardware interlocks, scan-fail zones and an E-stop independent of this process. Clamping here only smooths.
- **The tap (`pear_tap.py`) is written from reading PEAR's public source, never run inside it.** PEAR's worker sends each frame over a private pipe to the GUI process; the hook point is `Session.read()` in `mocap/pear/session.py` (usage is in the file's docstring). Frames there are already through PEAR's One Euro filter. It needs a one-line edit to your own copy of the tool. That repository has no declared licence, so this folder contains no code from it; do not republish their files.
- **PEAR gives no measured confidence** (its docs say its weights are heuristics). The tap sends 1.0 for a tracked frame and 0.0 when the person is lost, so the bridge's occlusion handling is on/off, not graded.
- **Unverified in the tap:** the joint indices (assumed SMPL-X order: pelvis 0, head 15, wrists 20 and 21; I found no name list), the axis mapping and handedness (PEAR output is treated as Y-up), the units (metres only if camera alignment succeeds), and the origin (PEAR's origin is not your stage centre). Check all of them with a recorded video before use.
- Its built-in network output is Unreal Live Link, which this bridge does not use.
- **Checked against a real Conductor (pchi-studio `primeswarm-pchi`, built and run locally):** it listens for JSON on UDP 127.0.0.1:8888 and parsed and accepted 78 `artist_tracking` messages from this bridge with no warnings or drops. The Conductor stores position and velocity but does not expose them on `/state`, so I confirmed receipt from its log only, not the stored values. Governance fired no rules. Casing is snake_case (confirmed from its Rust types).
- **Your show rules can react to this data.** The default rule file has `if artist_tracking.position.z < 0.0 then evolve(...)`; the bridge clamps z to 0 and above and treats z as up, so check that your venue rules agree on axes.
- Only the pelvis is sent. Hands, spread and tension need extra fields, which `artist_tracking` has no slots for: either `control_parameter` messages or a schema extension, and a decision about it.
- The invariants block sends residual 0 (a single position has no balanced set to check; the Conductor only checks residual on scene_update and warns on non-zero values otherwise). coherence_gap and curvature_signature are descriptive only. Nothing in them is a safety check.
- Latency and frame-rate figures in earlier discussion (30–80 ms inference, filter lag) are estimates, not measurements.
- A "256-byte payload" with `AudioSpatialPayload`/`VisualPayload`, and the KoroNoto, PixLiner and LUZIA consumers, appear nowhere in this workspace's code; they are not implemented here.
