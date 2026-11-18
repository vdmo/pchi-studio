# Where PCHI Studio sits

A one-page answer to "what is this, and how is it different from what I already use?" It says what is different, what is not, and what has not been shown yet. Read it with [LIMITATIONS.md](LIMITATIONS.md) and [foh-proof.md](foh-proof.md).

## In one sentence

PCHI Studio is a **show-logic and safety layer above the tools a crew already uses**. The show is a state chart of scenes with timed cues; a separate runtime sends the cues to your gear; people and agents act only inside roles you give them; everything is recorded.

It does **not** replace QLab, grandMA, Resolume, BEYOND or an X32. It tells them what to do and when, and keeps a record of what happened.

## What already exists

None of the parts below is new. Do not claim they are.

| Kind of tool | Examples | What it does well |
|---|---|---|
| Cue-list players | [QLab](https://www.wikipedia.org/wiki/QLab), [Show Cue System](https://showcuesystems.com/scs11help/topics/scs-overview.htm), Linux Show Player, Ontime | A linear list of cues, mostly audio and video playback, with outputs to other tools |
| Multi-department show control | [Medialon](https://medialon.com), [7thSense](https://7thsense.one/medialon-7thsense-show-control) | Lighting, audio, video and effects together, with timelines, triggers and conditional logic. This is the closest overlap |
| Laser software | [Pangolin](https://pangolin.com/QS/), [Showcontroller](https://www.laserworld.com/en/shownet-compatible/showcontroller) | A timeline and live engine for one department |
| Attractions control | [Theme-park systems](https://www.inparkmagazine.com/beyond-av-control/) | Lighting, animatronics, fountains and lasers together |
| Open-source show control | [Chataigne](https://github.com/benkuper/Chataigne-docs/blob/master/getting-started-1/getting-started.md) | OSC, MIDI, DMX, serial, UDP, TCP, HTTP and PJLink. It has a **State Machine** for real-time rules and a **Time Machine** (timelines), in separate modules. Free, GPL-3.0 |
| Button and control platforms | Bitfocus Companion and its newer product [Buttons](https://www.broadcastbeat.com/articles/bitfocus-improves-availability-security-and-user-management-in-new-version-of-buttons) | Buttons now advertises high availability and user permissions |
| State-chart editors | Stately/XState (from memory) | A state-chart editor for software, not for shows |

Medialon already offers conditional logic across departments, timelines with expressions in cue properties, monitoring and failover. Chataigne already has both a state machine and a timeline. PCHI is not the first to coordinate several departments, and not the first to pair state logic with timelines. See [the comparison below](#compared-with-medialon-and-chataigne).

## What is different

1. **A state chart is the backbone, with timelines inside each scene.** Scenes, transitions, guards and timers hold the structure of the show. A cue list is linear; here you can branch, hold, move on early and loop.
2. **One model, four views.** Show, Timeline, Cue sheet and Live edit and display the same data, with one undo history ([studio-guide.md](studio-guide.md)).
3. **Departments are first-class.** Lasers and stage automation are held until a person arms them. A ready board shows each department. GO asks for a second press if a department the next scene needs is on hold ([motion.md](motion.md), [connecting-systems.md](connecting-systems.md)).
4. **The editor and the runtime are separate.** Studio never talks to your gear; the runtime does. A change is shown as a diff, then sent, undone, or proposed for someone's approval ([roles.md](roles.md)).
5. **Rehearse before you send.** An edited show can be played against what has already happened, with nothing sent ([rehearse.md](rehearse.md)).
6. **A record you can verify.** Decisions are written to hash-chained receipts, and a run can be replayed and reported ([governance.md](governance.md), [replay.md](replay.md), [report.md](report.md)). A hash receipt shows the record was not changed; it is not a signature of who did it.
7. **AI that can only propose.** The panel and agents suggest edits, shown as a diff. A person applies them, and a proposal that adds new problems cannot be applied ([side-panel.md](side-panel.md), [mcp.md](mcp.md)).
8. **A shared library** of versioned shows, with plans, venues and statuses ([library.md](library.md)).

The claim to defend is the **combination**: a state chart of the show, department safety, a separate runtime with roles, and a verifiable record. It is not any one feature.

## Compared with Medialon and Chataigne

Built from vendor and project pages found by web search, not from running either product. Vendor claims are marked as claims. Where a row says "not found", I did not find it; that is not the same as "it does not have it".

| | PCHI Studio | [Medialon Manager](https://7thsense.one/medialon-7thsense-show-control) | [Chataigne](https://github.com/benkuper/Chataigne-docs/blob/master/getting-started-1/getting-started.md) |
|---|---|---|---|
| Cost and licence | See `PRICING.md` | Commercial | Free, GPL-3.0 |
| Timeline | Timelines inside each scene; editable from the Show tab | Timeline with drag-and-drop; sync accuracy claimed at 1/100 s | A Time Machine of sequences |
| State logic | A state chart of scenes: events, timers, guards, minimum times | Triggers and conditional logic; expressions in cue properties (V7) | A State Machine of rules, kept separate from the timeline |
| Protocols | OSC with 11 tool dialects, Art-Net, sACN, OBS, MIDI out (notes, program change, controllers) and MIDI Show Control out and in, MIDI timecode in, LTC in, PJLink, HTTP from profiles (plain http), JSON-RPC over TCP (Q-SYS, no login), MQTT publish (no built-in profile, no login), serial lines (RS-232; needs a feature build), Modbus TCP, UR robots. **None tried on a real device** | DMX, MIDI, TCP, UDP, OSC, Art-Net, sACN, RS-232, GPIO, SMPTE: **much broader** | OSC, MIDI, DMX, serial, UDP, TCP, HTTP, PJLink: **much broader** |
| Departments with arm and hold | Yes: lasers and motion are held until armed; a ready board; GO asks twice | Monitoring and alerts claimed; arming and holds not found | Not found |
| Editor separate from the thing that sends | Yes: Studio edits, a runtime sends, changes go as a diff, can be proposed for approval, and can be undone | Not found | Not found; it is one application |
| Roles and who-may-do-what | Roster with operator, editor, guest and per-department limits | Not found in what I read | Not found |
| Record of what happened | Hash-chained receipts, replay, run report | Log files and performance logging claimed | Not found |
| Failover | Standby and witness; tested only in simulation | Claimed for Manager 4: backup takes over a failed system | Not found |
| AI | Proposal-only panel and agent edits for approval | Not found | Not found |
| Maturity | Never on real gear, nobody outside the project has run a show | Established commercial product, in V7 | Established, used by artists and in community modules |

### What this changes

- **The "state chart plus timelines" claim is weaker than I first said.** Chataigne has a state machine and a timeline, and Medialon has conditional logic with timelines. What I could not find in either is *the show itself as a graph of scenes* with timelines nested inside each scene and one model behind Show, Timeline, Cue sheet and Live. That is a difference of design, not of capability, and a user may not care.
- **Roles and audit are the stronger difference,** but only against what I could find. Bitfocus Buttons now advertises permissions and high availability, and Medialon claims logging, so check each before saying PCHI alone has them.
- **Protocol reach is where PCHI loses.** Both competitors speak more protocols than PCHI. The gap has narrowed: [protocol-roadmap.md](protocol-roadmap.md) lists what is built, and PCHI now has MIDI out, MIDI Show Control, PJLink, HTTP output and more OSC dialects. Still not built: GPIO, serial with binary framing, SMPTE output, TLS for HTTP, and any password or auth for devices. Every one of the built protocols is tested against bytes or a stand-in, not a real tool, which is a weaker claim than "works with".
- **Maturity is where PCHI loses most.** Both are used on real shows. PCHI has not been.

## What it is not

- **Not a media server.** No audio or video playback, no pixel mapping, no previsualisation. It triggers tools that do those.
- **Not an emergency stop.** A PCHI stop is a software stop. Real lasers over a crowd and real moving stages need their own interlocks, safety systems and sign-off ([safety-drill.md](safety-drill.md)).
- **Not multi-user editing.** One person edits a show at a time, with no merge ([library.md](library.md)).

## What has not been shown

- **No real gear yet.** Every tool profile has been checked against its own tests and, at most, simulated devices. Several rules rest on community sources or memory ([industry-tools.md](industry-tools.md), [foh-proof.md](foh-proof.md)).
- **The example shows are invented**, with placeholder addresses and made-up stage programs ([../tools/examples/README.md](../tools/examples/README.md)).
- **The AI panel has run only against a stubbed network,** never a real model. Whether its proposals are good is untested.
- **Robots:** the UR driver has run only against a pretend robot, KUKA is a contract and an example profile, and nothing has run on a real robot ([ur.md](ur.md), [kuka.md](kuka.md)).
- **Scale and duration are untested:** large shows, long runs, and a real show network under load.
- **Nobody outside the project has operated it** on a real show.

## What would make the claim stronger

1. Run [gear-check.md](gear-check.md) and [real-gear-day.md](real-gear-day.md) on one real rig, and record the result in [foh-proof.md](foh-proof.md).
2. Have a stage manager who did not build it call a show from the Cue sheet and Live tab, and write down where they got stuck.
3. Try the AI panel on a real model, and count how many proposals were usable.
4. Compare it side by side with Medialon and Chataigne on the same small show, and write down honestly what each does better.

## How to describe it

- **Say:** "a state-chart show controller with department safety, human-approved changes and a verifiable record, sitting above your existing tools."
- **Do not say:** "like nothing else", "replaces your show control", "safe for lasers or robots", or "proven". Each of those needs evidence that does not exist yet.
