# Motion: robots and moving rigs

Step 1 of the [robotics plan](robotics-plan.md), and the safety behaviour that every robot driver will sit on. It is the `motion` department: a robot arm, a moving rig or any machine that can hurt someone. **There is no driver for KUKA yet**; Universal Robots has one ([ur.md](ur.md)), tested only against a pretend UR. What exists is the gate, the roles, the stop, the drill and a pretend device to build against.

## The one rule

**PCHI asks. The machine's own safety system decides.** PCHI starts named programs that someone qualified wrote and validated. It does not stream motion, does not power a robot on, does not clear a safety stop, and its **STOP MOTION is a software request over a network: it is not an emergency stop.** The machine's own emergency stop, within reach of a person, is the only emergency stop. Studio, the Doors check, the drill and the agent descriptions all say so.

## It behaves like lasers

A motion device is **armed** like a laser ([safety-drill.md](safety-drill.md)):

- It **starts disarmed**: every runtime start, restart, loss of the witness lease and takeover leaves it disarmed, and a live edit of the show neither arms nor disarms it.
- A cue to a disarmed motion device is **held**, not sent. So is a hand-sent cue (Try) and a test. The delivery record says `held` and why.
- **Arming** needs a person with the right: an operator or the **motion safety officer**. Arming never starts anything by itself: it only lets cues through.
- **Disarming and STOP MOTION** need no key and no role, and an agent may use them: the safe direction is never locked.
- **STOP MOTION** (`POST /motion/stop`, Studio's button, an OSC input or MIDI binding with the action `motion.stop`, the agent tool `pchi_stop_motion`) disarms every motion device and sends each device's profile stop message, if it has one. A device with none is said so in the answer ("use its own stop"). It leaves lasers armed, and **STOP LASERS leaves motion armed**: they are separate on purpose.

## The role: `mso`

The **motion safety officer** may arm, try and test **motion devices only**, and nothing else (no GO, no edits). The **laser safety officer** (`lso`) may no longer be mistaken for one: an `lso` cannot arm a robot, and an `mso` cannot arm a laser. An operator may arm either. Roles: [roles.md](roles.md). A roster entry can limit anyone to the `motion` department (`"depts": ["motion"]`).

## In a show

- A device of type `motion` serves the `motion` department. Cues use the action `motion.run` with a `program` name (Studio's cue sheet shows "Motion: run program"; the Timeline has a Motion lane).
- `motion.pause` and `motion.stop` exist as cue actions too (no value).
- The built-in profile `motion` is **PCHI's own generic OSC**: `/pchi/motion/run "<program>"`, `/pchi/motion/pause`, `/pchi/motion/stop`. It is for the twin and for a bridge written to receive it, **not any vendor's dialect**; the Doors check says its rules rest on memory, not on a vendor's documentation. Real robots get their own profile or driver.
- The program name is letters, digits and `_ . -`, at most 64 characters. A name with anything else is refused before it is sent.

## What the checks say

- **Doors check**: each motion device is `ok: disarmed` or `warn: ARMED: cues will move it. Anyone in its reach must be clear, and its own emergency stop must be within reach`.
- **Drill** (`pchi-drill`): a new drill, **Motion stays held** (held until armed, passes while armed, held after STOP MOTION, and STOP MOTION does not disarm a laser); the roles drill checks both safety officers only for their own kind of device; a restart brings motion back disarmed; and the human questions ask for a person at the machine's own emergency stop who has pressed it and seen the machine stop. These prove the **software gate**, not the machine's safety.
- **Gear check** (`pchi-gearcheck`): a motion device is skipped unless the person types `AREA CLEAR`, is armed only inside the check and disarmed again after, and is told what a PCHI stop is not.

## Try it with no robot

```bash
pchi-twin tools/motion/demo/motion-demo.pchi --out /tmp/motion.twin.pchi     # a pretend robot on loopback
pchi-studio-api /tmp/motion.twin.pchi --send                                  # in another terminal
curl -X POST localhost:3100/input -H 'Content-Type: application/json' -d '{"event":"REVEAL"}'   # the cue is HELD: disarmed
curl -X POST 'localhost:3100/devices/arm-1/arm?armed=1'                                          # arm it (needs a key if the runtime has one)
curl localhost:3400/seen/arm-1                                                                   # what the pretend robot received
curl -X POST localhost:3100/motion/stop                                                          # STOP MOTION: no key needed
```

## What was tested, and what was not

- **Tested**: the existing 244 runtime tests (every laser behaviour) pass unchanged after arming was made generic; 8 new tests for motion (arming, the stop, the roles, a lost lease and a takeover, the HTTP routes, the Doors check, the drill, the gear check); the SDK tests (97) and Studio type-check; the process-level tests; and a live run of a runtime with a roster against the twin's pretend robot: cue held while disarmed and the robot received nothing, the laser officer and an editor refused to arm it, the motion officer allowed, the cue then reached the robot, STOP MOTION without any key disarmed it, and each decision is in the signed record under the person's name.
- **Not tested**: any real robot, a real stop message from a real tool, Studio's Live tab buttons in a browser (they type-check and follow the laser buttons, but I have not clicked them), the command bar (typing "motion run X" is not understood yet: use the cue sheet or the Timeline), the Rig panel's department list in a browser.
- **Not done**: the UR and KUKA drivers, feedback in from a robot (state, faults, "not ready" pulling the show to a held state), and honest delivery states beyond "sent" ([robotics-kuka-ur.md](robotics-kuka-ur.md) has the plan).
