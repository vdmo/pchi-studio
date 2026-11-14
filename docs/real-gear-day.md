# A real-gear day

Everything PCHI says about the real world is, today, unproven: no tool profile, no MIDI port, no Art-Net or sACN output, no fixture type
has run against the real thing. `pchi-gearcheck` and `pchi-drill` exist to change that. This is a plan for a half-day that turns as many
claims as possible into proven ones, and records which.

The honest first rule: **do not tick what you did not see.** An unticked line is an unproven claim. That is allowed. A ticked line
that nobody saw is the thing this day is meant to remove.

## Before the day (about an hour)

1. **Decide the goal.** A tool you will really use on a show first. Proving three tools well beats touching seven.
2. **Find out what you can run without hardware.** Many tools have free or trial software. I have *not* verified which of them accept OSC
   without a console or licence, so the first thing to do is fill in this table, a few minutes per tool:

   | Tool | Free or trial software? | Accepts OSC in that mode? | Where I will run it |
   |---|---|---|---|
   | Eos | Eos family offline / Nomad (reported free to run offline) | ? | |
   | grandMA3 | MA3 onPC (reported free) | ? | |
   | MagicQ | MagicQ PC (reported free) | ? | |
   | QLab | macOS only (check which features are free) | ? | |
   | Resolume | Arena demo mode (reported to run with a watermark) | ? | |
   | X32 / M32 | X32-Edit connects to a desk; whether it can stand in for one is not known | ? | |
   | BEYOND | needs a licence; check for a trial | ? | |
   | Art-Net / sACN | a free viewer on the same network (for example sACNView) shows the frames | n/a | |
   | MIDI / MTC | a virtual MIDI port pair (for example loopMIDI on Windows) plus anything that sends MTC | n/a | |

   Anything with "?" stays unproven until a real desk or install says otherwise. Write the answers in `docs/foh-proof.md`.
3. **One machine, one network.** Run the runtime and each tool on the same laptop (loopback) to prove the *messages*; a second
   machine or a real desk on a real switch proves the *network*. They are different claims: record which one you tested.
4. **Build what you need:** `cargo build --release -p pchi-studio-api` (add `--features midi` for MIDI; on Linux that needs `libasound2-dev`).
   Keep the tools' versions: write them down.
5. **Print or open** `foh-proof.md` section 4 and `gear-check.md`.

## The day, in order

Each block has a time box. Stop a block when it is stuck: an honest "could not try" is a result.

| Time | What | How | Record |
|---|---|---|---|
| 15 min | **Start from a clean show** | a small `.pchi` with one device per tool you will test, harmless cues | the show file's name |
| 20 min per tool | **Messages** | `pchi-gearcheck show.pchi --only <id>` with the tool's own OSC monitor open next to it. Pick a harmless cue; compare the address the check shows with the tool's monitor; did it react? | the report, a screenshot of the monitor |
| 10 min per tool | **Feedback** | set the listen port, fire a cue *in the tool*, see "Heard from devices" and (Eos) "Fired on the tools" | what was heard |
| 10 min | **Cable pull** (X32, or any tool that answers a probe) | `pchi-gearcheck` asks you to pull and restore the cable | seconds to notice and to recover |
| 20 min | **DMX** | a sACN or Art-Net viewer, then a fixture if you have one: `1=255`, a patched `wash1=blue`, a fade | the viewer's frame, the fixture's reaction |
| 20 min | **MIDI and timecode** | a virtual port pair; a note presses GO; MTC drives a locked scene; stop the MTC and watch the Live tab | what happened at each step |
| 15 min | **The drill** | `pchi-drill show.pchi --interactive --sign ./receipts` | the report and the receipt |
| 15 min | **Write it down** | see below | the commit |

**Safety on the day.** Lasers: only with a laser safety officer, the output blocked at the laser software, and the beam path clear. Audio:
the PA muted before any scene recall. DMX: nothing strobing or moving that you have not planned for.

## After the day

1. `pchi-gearcheck ... --update-doc docs/foh-proof.md` ticks only what you confirmed. Check the file before committing.
2. For each rule you saw work, add a `proven` note to it in the profile file (`profiles.md`), for example
   `"proven": "2026-11-20, Eos 3.2 offline on the same laptop, Alex"`. The Doors check then stops warning about that rule. Do the same
   for a fixture type's file notes.
3. For each rule that did **not** work, fix the profile (the tool's own monitor shows what it expected) and run the check again, or
   leave it unticked with a note about what failed. A wrong profile found today is a show saved.
4. Keep the reports: put them in `docs/gear-results/<date>/` with the tool versions.
5. Say, in the commit message, what was and was not tried. A claim is proven only for the version and set-up you tried.

## What the day will not prove

- A different version of the tool, or a different console surface, may behave differently.
- Loopback proves the message, not the venue's network, switches or Wi-Fi. A show-network rehearsal is its own thing.
- A short test is not a long run: nothing here shows hours of uptime.
- It says nothing about the people. The drill's human questions are for them.
