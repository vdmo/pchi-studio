# Stage-show examples

Three shows at different scales, for learning the editor and for trying the show library. **Everything in them is invented**: song titles, timings, cue values, addresses and ports. They show the *scale and shape* of a big pop show, not any real tour's setlist or settings.

| File | Scale | What it shows |
|---|---|---|
| [stadium-pop-tour.pchi](stadium-pop-tour.pchi) | Stadium, 80,000 | 22 scenes, 200 cues: doors, support changeover, countdown, intro film, three acts, a B-stage acoustic set with stage travel, an encore, house-up. Six devices: LED walls, lighting, lasers, FOH desk, playback, stage automation. |
| [arena-pop-show.pchi](arena-pop-show.pchi) | Arena, 15,000 | The same shape, condensed: 14 scenes, 108 cues, four devices (no lasers, no automation). |
| [stadium-preshow-doors.pchi](stadium-preshow-doors.pchi) | Stadium, a day by the clock | Gates open (90 min), support act (45), changeover (30), countdown, intro film, hand-over. Scenes move on by timer; the operator can skip ahead with NEXT. |

## How a running order is built

- **One scene per song or beat**, in running order on the Show tab (one row per act). Operators advance with **NEXT**; the countdown and intro film advance by timer, because their length is fixed.
- **A minimum time on each song** (45 s): a stray GO in the first 45 seconds is held, so a song cannot be skipped by accident ([scene-minimum.md](../../docs/scene-minimum.md)).
- **One lane per system** in each scene (LED walls, lighting, FOH desk, playback, lasers, stage), each pointing at its device. Cues carry readable labels (`Chorus 1`, `Final chorus`).
- **Values are what each tool's profile accepts:** a Resolume column (`12`) or layer/clip (`2/3`), a grandMA3 executor number, a BEYOND cue name, a QLab cue number, an X32 scene number, a motion program from the device's allowed list.

## Safety, in the files and around them

- **Lasers and the stage automation are held until a person arms them** in the Live tab, and a PCHI stop is **not an emergency stop**. A real moving stage, and real lasers over a crowd, need their own safety systems, interlocks and sign-off ([safety-drill.md](../../docs/safety-drill.md), [motion.md](../../docs/motion.md)).
- The stage programs (`LIFT_UP`, `BSTAGE_OUT`, ...) are placeholders for whatever the stage-automation system's own operator would expose. PCHI sends only a program name from the device's list.
- There is no show-stop or hold scene in these examples: how a show is stopped, and resumed, is a decision for the production.

## Using them

- **In the library:** `node tools/examples/import-examples.mjs http://127.0.0.1:3101` adds all three to a runtime started with `--library DIR` (once each: running it again leaves them alone). They appear under **Library** with status, venue and notes.
- **From a file:** Studio's **Open** takes the `.pchi` directly.
- **Change the rig first.** The addresses (`10.20.x.x`) are placeholders: set yours in the Rig tab before pointing a show at real gear, and run the doors check and [the gear check](../../docs/gear-check.md).

## Checking them

The editor's checks find no problems in any of them (the `examples.test.mjs` test keeps it that way), and the runtime's own doors check in dry-run mode reports no failures: every cue value is accepted by its tool's profile. What that does **not** prove: that any real tool accepts these messages (several profile rules rest on community sources or memory, which the doors check says), or that a show this size runs well on a real network.

## Rebuilding them

[build-examples.mjs](build-examples.mjs) generates the three files, so the rig, songs and timings can be changed in one place:

```bash
cd pchi-studio-react && npm run test:sdk     # builds the SDK bundle the script uses
cd .. && node tools/examples/build-examples.mjs
```
