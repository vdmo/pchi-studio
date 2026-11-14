# Timecode and MIDI

Two things a working FOH crew expects: cues locked to the playback's **timecode**, and **MIDI** (a foot pedal, a keyboard, a
playback machine sending MIDI Time Code).

## Timecode

A scene's timeline can be **locked to timecode**. Its cues then fire when the timecode reaches a position, not on the
runtime's own clock, so pausing, scrubbing and re-running the playback keep the show with it.

- **Lock a scene:** say "lock intro to timecode 00:01:00:00" (`"tc_start": "00:01:00:00"` on the scene's timeline). That is the
  timecode at which the scene's own clock reads zero: a cue at 2 s fires at 00:01:02:00. Say "unlock intro" to go back to the
  runtime's clock.
- **Change scene on a timecode:** say "from intro to drop at timecode 00:01:05:00" (a transition on the event
  `tc:00:01:05:00`). The scene change is the show's own clock, like a timer: a scene minimum does not hold it.
- **While the timecode is not running, locked cues wait.** The Live tab shows `Timecode · 00:01:03:13 · 25 fps · running · from mtc`
  and the Doors check warns when a show uses timecode and none is running.
- **Frame rates:** 23.976, 24, 25, 29.97 (non-drop and drop-frame) and 30. Write drop-frame with a `;` before the frames
  (`01:00:00;02`); frame 00 and 01 of a non-tenth minute do not exist in drop-frame and are refused.

### What happens when the timecode jumps

Timecode moves by about 40 ms a frame. A step of more than half a second, or any step backwards, is a **locate**, not a play:

- **Forward:** nothing fires. The cues jumped over are not fired late (a light that fires 40 s late is worse than one that
  does not fire). The Live tab and the run record how many were skipped.
- **Backward:** cues fire again as the position passes them, so rehearsing a section again works.
- **In the first message heard,** nothing fires: there is nothing to compare with. Enter the scene before the timecode
  reaches its start, which is how it works in practice.

### Where timecode comes from

| Source | How |
|---|---|
| **MIDI Time Code** | a MIDI input with `{"timecode": "mtc"}` (below). Eight quarter-frame messages make a position, taken as two frames after the first; a lost message is not guessed, and the next full sequence recovers. A full-frame message (a locate) is read exactly |
| **OSC** | an OSC input with `{"address": "/tc", "timecode": true, "fps": 25}`. The message may be a string `"01:02:03:12"`, four numbers (hours, minutes, seconds, frames, and optionally the frame rate), or one number of seconds |
| **LTC** | **not supported.** LTC is an audio signal and needs an audio input and a decoder. Convert it to MTC or OSC first (many playback tools and audio interfaces can) |

A timecode source must be allowed to press: it acts as the role `button` (see `osc-input.md`, `roles.md`). The record names it.

## MIDI input

In the inputs file (`--inputs`), next to OSC inputs:

```json
{"midi": [{
  "name": "Pedal",
  "port": "nanoKONTROL",
  "bindings": [
    {"note": 60, "channel": 1, "event": "NEXT"},
    {"pc": 3,   "event": "DROP"},
    {"cc": 7,   "context": "volume", "scale": [0, 1]},
    {"cc": 64,  "event": "NEXT"},
    {"note": 62, "action": "laser.stop"},
    {"timecode": "mtc"}
  ]
}]}
```

- `port` is part of the port's name; without it, the first port. `pchi-studio-api --midi-ports` lists what is there.
- A **note** or **program change** presses an event; a note-off, or a note-on with velocity 0, is a release and is ignored.
- A **controller** (`cc`) sets a context value, scaled from 0 to 127 to `scale`; used as an `event` it counts as pressed from 64 up
  (a sustain pedal).
- **`laser.stop`** is the hard stop, never held. Nothing else can be bound: not arming, editing or trying a cue.
- A scene's **minimum** holds a MIDI press too, with a receipt, as for any button.

### Building with MIDI

Reading MIDI ports needs the system's MIDI library, so it is an optional feature:

```bash
cargo build --release -p pchi-studio-api --features midi
```

On Linux that needs `libasound2-dev`. Without the feature, a `midi` input is refused at startup with that message and everything
else works.

## What is tested, and what is not

- **Tested:** timecode maths at every rate (drop-frame included), the MTC decoder (every rate, lost messages, full frames), the
  jump rules, cues, `tc:` transitions and the minimum bypass, the OSC forms of timecode, MIDI parsing and bindings, and MTC
  driving a locked scene. OSC timecode was also played into a real runtime over real UDP at 25 fps: the cues fired at the right
  timecodes and the scene changed on its `tc:` transition.
- **Compiled but not run:** the MIDI port code (`--features midi`) compiles against the real `midir` library on Linux, but has
  not been run against a MIDI device or a playback machine. Treat it as unproven until you have sent it real MTC.
- **Not covered:** LTC, timecode in `rehearse` (a rehearsal replays presses, not timecode), and timecode restored after
  `--resume` (it is heard again from the source).
