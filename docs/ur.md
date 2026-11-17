# Universal Robots (step 2 of the robotics plan)

PCHI can start, pause and stop **named programs** on a Universal Robots arm, and read its state, through the controller's **Dashboard Server** (TCP, port 29999, plain text). It is built on the [motion department](motion.md): the same arming, roles and stop. **It has been run only against a pretend UR written from UR's own manual. Not against URSim, and not against a real robot.** Do those before you rely on it ([What is not proven](#what-is-not-proven)).

## What PCHI does and does not do

It runs programs **the robot's programmer wrote and validated**. The speeds, paths, zones and limits are in the program and in the robot's own safety configuration, not in PCHI. PCHI never moves a joint, never powers the robot on, never releases brakes, never clears a protective stop, never changes the robot's mode. A PCHI stop is a software request over a network, **not an emergency stop**: the robot's own emergency stop is the only one.

## Set up

On the robot (this part is the programmer's, not PCHI's): put it in **Remote Control** mode (the Dashboard Server refuses program commands otherwise), and have the programs on the controller.

In the rig, add a **Universal Robots (Dashboard)** device (type `ur`):

| Parameter | Meaning |
|---|---|
| `host` | the controller's address |
| `port` | `29999` unless the controller is set otherwise |
| `programs` | **the names it may be asked to run**, comma separated, without `.urp`: `PARK, OPEN_ARM`. Nothing else is ever run; with no list, every cue to this device is refused |

A cue is **Motion: run program** with the program's name (`OPEN_ARM`). The device is in the `motion` department: it starts **disarmed**, and a person arms it on the Live tab (an operator or the motion safety officer).

Start the runtime with `--send` as usual. Without it, nothing is sent and not even a connection is made.

## What a cue does, step by step

`motion.run OPEN_ARM` to a `ur` device:

1. **Checked before anything is sent**: the name matches `[A-Za-z0-9_.-]{1,64}` and is on the device's `programs` list. If not, nothing at all reaches the robot, and the Doors check already fails that cue.
2. Held if the device is **disarmed**, or if it **reads as not ready** (below).
3. Commands, one line each, each answer checked: `is in remote control` (must be `true`), `safetystatus` (must be `NORMAL`), `get loaded program`, `load OPEN_ARM.urp` (skipped if it is already loaded), `play` (must answer `Starting program`).
4. A read-back: `programState` until it says `PLAYING` (up to 3 s).

The delivery record then says exactly how far it got:

| Status | Means |
|---|---|
| `held` | not sent: disarmed, or not ready (with the robot's own reason) |
| `refused` | the robot answered and said no, in its own words (`Failed to execute: play`; "not in Remote Control mode") |
| `failed` | no answer, not reachable, or PCHI's own check said no |
| `accepted` | the robot said yes, but the read-back did not show it running (a short program may already be finished) |
| `running` | the robot said yes and a read-back shows it playing |

There is no "done": PCHI does not know when a program finishes, or what it did.

## Ready, and a fault

While the runtime is sending, the robot's state is read about once a second: remote control, safety status, robot mode, program state. It is **ready** when remote control is on, the safety status is `NORMAL` and the mode is `RUNNING`. Otherwise it is **not ready**, and the reason is shown in the robot's own words (`its safety status is PROTECTIVE_STOP`).

- Cues to it are **held** while it is not ready.
- If it is **armed when it faults, it is disarmed** and a `fault` message and a signed receipt are written. When the fault clears it stays disarmed: **a person must arm it again**.
- PCHI does **not** try to recover it: no unlock, no restart, no power on. Someone on site inspects it.
- Its readiness is on the Live tab like a department's, and in the Doors check.

## Stop

**STOP MOTION** disarms the robot and sends `stop`. The record says `accepted` (it acknowledged) or `refused` / `failed` ("use the machine's own stop"). A dropped connection is retried once on a fresh one.

## What PCHI will never send

`power on`, `power off`, `brake release`, `unlock protective stop`, `close safety popup`, `restart safety`, `load installation`, `shutdown`, `set operational mode`. The profile lists them under `never`, and **the runtime refuses to load a profile that could send one**. The whole of what the robot is ever told is the 9 commands in the profile ([link-profiles.md](link-profiles.md)).

## Try it with no robot

The twin has a pretend UR:

```bash
pchi-twin tools/motion/demo/ur-demo.pchi --out /tmp/ur.twin.pchi          # a pretend controller on loopback
pchi-studio-api /tmp/ur.twin.pchi --send
curl -X POST 'localhost:3100/devices/ur-1/arm?armed=1'                     # needs a key if the runtime has a roster
curl -X POST localhost:3100/input -H 'Content-Type: application/json' -d '{"event":"REVEAL"}'
curl localhost:3400/seen/ur-1                                              # every command the robot was sent
curl -X POST localhost:3400/fault -H 'Content-Type: application/json' -d '{"device":"ur-1","fault":"protective-stop"}'
```

Faults for the pretend UR: `protective-stop`, `local-control` (out of Remote Control mode), `refuse-play`, `down`, `healthy`.

## What is not proven

- **No real UR and no URSim.** The pretend UR is written from UR's manual, so it shares any mistake in my reading of it. Run `pchi-gearcheck` against URSim first, then the robot, with its programmer present and its area clear.
- **Two details are from memory, not the manual page**: that the controller sends a greeting line when you connect, and that `programState` may carry the program name after the state. The profile accepts both present or absent.
- **CB-series robots** have a separate Dashboard document that I have not read. Treat them as untested.
- `load` is documented to fail if the installation needs a safety confirmation; PCHI reports that as `refused`, and a person must confirm on the pendant.
- Whether polling once a second disturbs a real controller is unknown.
- That a PCHI stop works when the network does not is **not** something software can promise.
