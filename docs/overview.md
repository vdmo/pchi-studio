# Overview: what PCHI Studio is

PCHI Studio runs live shows. A show is a small state machine: **scenes** (INTRO, DROP, FINALE) joined by **transitions** (on an input, on a timer, on a condition), and inside each scene **lanes of timed cues** that tell lighting, lasers, audio and visuals what to do. The **runtime** plays that show in real time, sends each cue to the right device, and writes down everything that happens.

Three ideas run through the whole product:

1. **The show is data you can check before the night.** A show can be validated, simulated, rehearsed against a recorded run, and drilled for failures, all without gear.
2. **Whatever happens is recorded and replayable.** Every input, cue, delivery, edit and decision goes into a log. A run can be replayed, handed over as a one-page report, and its decisions verified from signed receipts.
3. **People and agents have roles, and the runtime enforces them.** An agent can look, rehearse and *propose*. Only a person approves. Lasers start disarmed and only a person arms them.

## The parts

| Part | Where | What it is |
|---|---|---|
| **Studio** | `pchi-studio-react/` | The browser editor and operator screen. Tabs: Show, Timeline, Cue sheet, Rig, Live |
| **Runtime** | `crates/pchi-studio-api/` (`pchi-studio-api`) | The process that runs the show: HTTP and WebSocket API, timers, routing, recording, roles |
| **Showchart engine** | `crates/pchi-showchart/` | The state machine: scenes, transitions, guards, timelines, validation, simulation |
| **Event store** | `crates/pchi-eventstore/` | SQLite log of a run, with replay |
| **Event bus** | `crates/pchi-eventbus/` | The in-process publish and subscribe the engine emits actions on |
| **Memory** | `crates/pchi-memory/` | Agent memory: typed notes that fade, with who vouched for each |
| **SDK** | `pchi-sdk/` | TypeScript: the command parser, cue sheet, rig checks, live feed, replay. Used by Studio and testable on its own |
| **Command line tools** | `pchi`, `pchi-drill`, `pchi-gearcheck`, `pchi-netcheck`, ... | See [tools-reference.md](tools-reference.md) |
| **Protocol layer** | `pchi-schema/`, `primeswarm-pchi/`, `tools/` | The original PCHI protocol: Only-Lang rules, the conductor, bridges for Resolume, TouchDesigner, Ableton, Strobe, TiXL |

```
  Studio (browser)           department pages, Stream Deck, button box, desks
        |  WebSocket /ws + HTTP            |  HTTP, OSC inputs, MIDI, LTC
        v                                  v
  +--------------------------------- runtime ---------------------------------+
  |  showchart engine --- timers --- cue scheduler --- router --- OSC / DMX out |-----> rig
  |        |                                                        |           |
  |   event store (SQLite)   --record file   signed receipts    roles + keys    |
  |        |                                                                     |
  |   memory (optional, never in the show's path)                                |
  +--------------------------------------------------------------------------+
        ^ copies the log                      ^ lease
   standby runtime  ---------------------- witness
```

## Concepts you will meet

- **Show file (`.pchi`).** One JSON file with the showchart *and* the rig. See [show-file.md](show-file.md).
- **Scene (state).** Where the show is. Entering a scene runs its *enter actions*, starts its timelines and timers; leaving cancels cues that have not fired.
- **Transition.** A move between scenes, on a named event (`NEXT`, `GO`), after a delay (`after_ms`), and optionally guarded by a condition on the show's *context*.
- **Context.** Named values (`energy`, audience votes, timecode) that guards read. Set by inputs, sensors and votes.
- **Lane and cue.** A lane belongs to a department (visual, lighting, laser, audio, other) and optionally a specific device. A cue fires one action at an offset from entering the scene, and has a cue number a stage manager can call.
- **Action.** What a cue does, as `{kind, payload}`, for example `lighting.look`. The router turns it into OSC for the device.
- **Rig.** The systems that take part: devices with a host, port and department. Profiles say how an action becomes that tool's own messages.
- **Readiness.** Each department says "standing by" or "on hold". It informs the operator and makes GO ask for a second press; it never blocks the show by itself.
- **Dry run and `--send`.** The runtime reports where cues *would* go until you start it with `--send`. Opening a show can never fire a laser by accident.
- **Roles.** operator, editor, lso (laser safety officer), guest, agent, button. See [roles.md](roles.md).
- **Governance.** Signed, hash-chained receipts for every decision, verifiable offline. See [governance.md](governance.md).
- **Recording, replay, report.** See [replay.md](replay.md) and [report.md](report.md).
- **Failover.** A standby copies the log and takes over; an optional witness grants a lease so only one runtime sends. See [failover.md](failover.md).

## What a run looks like

1. You build or import a show and set up the rig in Studio. Save as `show.pchi`.
2. You start the runtime on the show: `pchi-studio-api show.pchi --db show.db`. It is a dry run.
3. You walk the pre-show checks: doors check, drill, gear check, network check.
4. You restart with `--send`, doors open, and the operator presses GO from the Live tab (or a button box does).
5. Cues leave as OSC. Departments report ready. If the machine dies, the standby takes over or `--resume` carries on.
6. After the show you export the recording, produce the hand-over report, verify the receipts and review what memory drafted.

## What PCHI does not do

It does not render video, generate sound, or replace your lighting desk or media server; it is the layer that calls them. "Sent" means a message left the runtime. UDP has no acknowledgement, so only a device's own READY or feedback says it acted. See [LIMITATIONS.md](LIMITATIONS.md) and [foh-proof.md](foh-proof.md) for what has and has not been proven on real gear.
