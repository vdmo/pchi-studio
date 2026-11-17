# Checking the UR driver against URSim, then a real robot

The driver ([ur.md](ur.md)) has only met a pretend UR that I wrote from UR's manual. This is the procedure that replaces "matches my reading of the manual" with "matches what a UR controller actually says": first against **URSim** (UR's own simulator, which runs the real controller software), then against the real robot. Each stage uses a tool that is **read-only** until the one step where a person confirms the area is clear.

Nothing below has been run yet: URSim was not available where the tools were written. What was run is the checker (`pchi-linkcheck`) against the pretend UR. Steps that depend on UR's software are marked **(from memory: check UR's own URSim documentation)**.

## 0. Review what PCHI can send, with the robot's programmer

```bash
pchi-linkcheck ur --commands
```

It prints the whole of what PCHI can ever send to a UR: nine commands. Whoever is responsible for the robot should read it and agree. Nothing else is ever sent, and the runtime will not load a profile that could send `power on`, `brake release`, `unlock protective stop` and the others.

## 1. Get URSim running

Either of these. Both need a person to do the install: it is a system change, and the download needs a free UR account.

- **Docker (the easier one):** the e-Series image is `universalrobots/ursim_e-series`. **(from memory)** something like:
  ```bash
  docker run --rm -it -p 5900:5900 -p 6080:6080 -p 29999:29999 -p 30001-30004:30001-30004 \
    -v "$HOME/ursim/programs:/ursim/programs" universalrobots/ursim_e-series
  ```
  `6080` serves PolyScope in a browser (`http://localhost:6080`). `29999` is the Dashboard Server PCHI uses: publishing it with `-p` should expose it whether or not the image's own examples do, but **that is the first thing to confirm**.
- **The Linux download** of "Offline Simulator (UR Series and e-Series)" from universal-robots.com (free account). UR's own docs cover running it.

## 2. Get the simulated robot ready (a person, on PolyScope)

PCHI will not do any of this, on purpose (it never powers on, releases brakes or changes modes):

1. Initialise the robot: power on and release the brakes, so the mode is `RUNNING`.
2. Enable and switch on **Remote Control** (the Dashboard Server refuses program commands without it). **(from memory: a setting under Settings, System, Remote Control, then the mode switch at the top right)**.
3. Put two or three small, harmless programs on it, for example `PARK` and `OPEN_ARM` (a program that only waits is enough). They live in the controller's programs folder: with the Docker command above, in `$HOME/ursim/programs`.

## 3. Read it (read-only)

```bash
pchi-linkcheck ur --host 127.0.0.1 --port 29999 --out docs/gear-results/ursim-readings.json
```

It connects and sends only the four status reads. Look at each line:

- **`ok`** means the reply is in the form the profile expects.
- **`DIFFERENT`** means the profile was written from a manual and the controller disagrees. **Fix the profile** (`profiles/link/ur.json`) and the pretend UR to match what URSim said, not the other way round, and re-run.
- **The two things I was unsure of** will now be settled: whether it **greets on connect** (the profile and the pretend UR assume `Connected: Universal Robots Dashboard Server`; the profile works either way), and whether **`programState` carries the program name** after the state. Update [ur.md](ur.md)'s "not proven" list with what you saw.
- It should read **READY** with the robot initialised and in Remote Control. Put it in Local control, then run it again: it should say NOT READY with the reason. Trigger a protective stop in URSim: same.

Keep the `--out` file. It is the evidence of what the controller said, with nothing of PCHI's in it.

## 4. Gear check against URSim (a person confirms the area, even for a simulator)

Put the simulator in a rig file as a `ur` device (host `127.0.0.1`, `programs`: the names you made):

```bash
pchi-gearcheck my-ur-show.pchi --only ur-1 --report gear-ur.md
```

It asks for `AREA CLEAR`, shows the exact commands, sends one cue, and reports the **robot's own answer** (`running`, `accepted`, `refused` with its words). Try the failure cases too, because they are what the driver exists for:

- a program that is **not** on the `programs` list (nothing should reach URSim at all);
- a program on the list that does **not exist** on the controller (`File not found`: `refused`);
- Remote Control **off** (`refused`, in URSim's words);
- a **protective stop** while the runtime is running with `--send` (the device goes NOT READY, an armed one is disarmed, cues are held, and it stays disarmed after the fault clears).

## 5. The runtime and the drill against URSim

```bash
pchi-studio-api my-ur-show.pchi --send --roster people.json
pchi-drill my-ur-show.pchi --interactive --sign ./receipts
```

Run the show, arm the device as the motion safety officer, press the events, and watch the Live tab and the record. Press STOP MOTION while a program plays: URSim should show it stopped, and the record should say `accepted`.

## 6. Only then, the real robot

All of this again, but on the real controller, with: its **programmer present**, the **area clear**, **its own emergency stop within reach of a person who has pressed it that day**, and programs the programmer wrote and validated for this venue. Start with `pchi-linkcheck` (read-only). A real robot may differ from the simulator in ways only the real one shows; the differences are what this stage is for.

Record the results in `docs/gear-results/`, and note in [ur.md](ur.md) what has now been seen on what software version. Until step 6 is done and written down, the driver is **tested against a simulator at best**, and the docs must keep saying so.

## What this does not settle

Whether a PCHI stop works when the network does not, how the real robot behaves under load or with a different Polyscope version, CB-series robots, and anything that happens in the physical world. Those stay with the robot's own safety system and the people responsible for it.
