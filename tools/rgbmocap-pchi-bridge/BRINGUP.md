# Bring-up checklist (needs a camera and a person)

Everything below is **unverified**: none of it has been done on real hardware. Do it in order, with no laser, moving head or amplifier connected. Stop at the first failure.

1. **Install the tap.** Copy `pear_tap.py` into your copy of toyxyz_RGBMocap at `mocap/pear/`, and add the one line from its docstring to `Session.read()`. Start PEAR live tracking.
2. **See frames arrive.** `python watch.py`. Expect coordinates every 0.25 s. "nothing received" means the hook is not firing or the port differs (`PEAR_TAP_PORT`).
3. **Check joints and axes.** Stand in view. Raise your **right** hand above your head: watch.py must say "right above head". Then the left. If left and right are swapped, swap the wrist indices (20 and 21) in `JOINTS`. If it says "HEAD BELOW PELVIS", the up axis or the indices are wrong: adjust `AXES` and `SIGNS` or check index 15 (head) and 0 (pelvis).
4. **Check direction and units.** Step 1 m to your right; the x value should change by about 1.0 in the right direction. Turn it around with `SIGNS`. If 1 m reads as something else, camera alignment failed or the scale is off.
5. **Set the origin.** Stand on your centre mark and set `ORIGIN` in `pear_tap.py` so the pelvis reads about (0, 0).
6. **Record a take.** Stop watch.py, then `python record.py take1.jsonl --seconds 60` while you do fast arm raises, a lunge and a walk behind something. Treat the file as personal data.
7. **Replay it.** Start the bridge and a Conductor, then `python replay.py take1.jsonl`. Look for the glitches `tune.py` assumes: pose flips, jumps on occlusion.
8. **Measure your delay.** Film a clap and a sound together, or use a visible LED, and compare the time between the movement and the output; put that number into `python tune.py --delay <seconds>` and choose `--lead` and `--blend` from its table.
9. **Check tracking lost.** Walk out of view for longer than `--lost-after`. The Conductor should stop receiving position and `performer_01.rgbmocap.tracking` should read 0. Decide what each fixture does then; nothing is wired up yet.
10. **Only then** connect any fixture, with its own hardware limits and E-stop independent of this software.
