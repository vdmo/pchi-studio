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
| Glue and state-chart tools | Chataigne, Bitfocus Companion, Stately/XState (from memory, not checked for this page) | Routing between tools; a state-chart editor for software |

Medialon and similar systems already offer conditional logic across departments. PCHI is not the first to coordinate several departments.

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
