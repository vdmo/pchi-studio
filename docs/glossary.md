# Glossary

**Action.** What a cue or a scene transition does: `{kind, payload}`, for example `lighting.look`. The router turns it into OSC or DMX for a device.

**Agent.** A named non-person client (an assistant, a script). Its edits become proposals; it cannot press GO, arm a laser, approve or undo. See [roles.md](roles.md), [mcp.md](mcp.md).

**Arm / disarm.** A laser must be armed, per device, by an operator or the lso before its cues go out. Runtimes start disarmed. STOP LASERS disarms all.

**Audience gateway.** Your web page or chat bot that sends votes to `POST /vote` with the audience token. [virtual-events.md](virtual-events.md)

**Button.** A role for machine inputs (a button box, a timecode source) that may press the few things an input may.

**Conductor.** `primeswarm-pchi`, the PCHI protocol's central state manager that enforces Only-Lang rules. Separate from the Studio runtime.

**Context.** Named values in the running show (`energy`, votes, timecode) that guards read.

**Cue.** One action at an offset from entering a scene, in a lane, with a cue number a stage manager can call.

**Dept (department).** visual, lighting, laser, audio or other. Lanes, devices and readiness are per department.

**Doors check.** A check of the whole show against the rig and each tool: what will not work, what to look at. `GET /preflight`.

**Draft.** A memory not yet verified by a person. Counts for 0.55 of a verified one.

**Dry run.** The runtime's default: cues are reported ("would go to ...") and nothing is sent. Turned off with `--send`.

**Edit key.** The shared operator key (`--edit-key`).

**Fence.** A takeover telling the old primary to stop. A takeover fences for good; a *pause* from a lost witness lease ends when the lease is granted again.

**Golden set.** Memories and questions with known answers, used by `pchi-memory eval`.

**Governance.** Signed, hash-chained receipts of every decision. [governance.md](governance.md)

**Guard.** A condition on the show's context that a transition requires.

**Held.** A press ignored because the scene has not run its minimum. Recorded as `show.held`.

**Lane.** A row of cues in a scene belonging to a department and optionally a device.

**Lease.** The witness's grant of the right to send. [failover.md](failover.md)

**LSO.** Laser safety officer: a role that may arm lasers and try or test-fire lasers only.

**Memory.** Typed, fading notes with who vouched for them, in `pchi-memory`. [memory.md](memory.md)

**Minimum (`min_ms`).** How long a scene must run before an input can move it on.

**OSC.** Open Sound Control, the UDP message format most tools take.

**Primary / standby.** The runtime running the show, and the one that copies its log and takes over.

**Profile.** A file describing how an action becomes a specific tool's OSC. [profiles.md](profiles.md)

**Proposal.** An edit waiting for a person's approval, with a reason and a rehearsal.

**Readiness.** A department's "standing by" or "on hold". Informs, never blocks by itself.

**Receipt.** One signed line in the governance chain.

**Recording.** A run's log with the show, as a file (`pchi-run/1` or `pchi-runlog/1`). [replay.md](replay.md)

**Rehearse.** Play an edited show against a recorded run on a virtual clock; nothing is sent.

**Resume.** `--resume`: carry on the run in `--db`, skipping cues that came due while down.

**Rig.** The set of devices and systems that take part, with hosts, ports and departments.

**Roster.** The file of named people, roles and keys. [roles.md](roles.md)

**Scene (state).** Where the show is. Entering one runs its enter actions and starts its timeline.

**Showchart.** The state machine: scenes, transitions, guards, timelines.

**Standby.** See primary.

**Transition.** A move between scenes on an event, a timer, or a guard.

**Verifying key.** The public key that checks receipt signatures. Write it down somewhere the show machine cannot change.

**Witness.** `pchi-witness`, the third process that grants the lease.
