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
| **LTC** | an `ltc` input in the inputs file (below): the timecode a playback machine puts on an audio track, read from a pipe, a file, or an audio device |

A timecode source must be allowed to press: it acts as the role `button` (see `osc-input.md`, `roles.md`). The record names it.

## LTC input

LTC is an 80-bit biphase-mark signal on an audio track, once per video frame. PCHI decodes it from audio; it does not care about the signal's polarity or level, follows a tape a few percent fast or slow, and ignores hiss between frames.

```json
{"ltc": [{"name": "Playback", "stdin": true, "sample_rate": 48000, "format": "s16le", "channels": 2, "channel": 1, "fps": 25}]}
```

- **Source (give exactly one):** `stdin` (a pipe, for example `arecord -f S16_LE -r 48000 -c 2 | pchi-studio-api show.pchi --inputs inputs.json`, or `ffmpeg ... -f s16le -`), `file` (a raw audio file or a named pipe; read at the speed it plays, or as fast as possible with `"pace": false`), or `device` (part of an audio input device's name; a build with `--features audio`).
- `format` is `s16le` (default), `f32le` or `u8`; `channel` says which channel carries the LTC (0 is the first).
- `fps` is the rate to assume where the signal cannot say: 29.97 and 30 differ by 0.1%, so only your word settles it (add `"drop": true` for drop-frame, which the signal does say). Default 25.
- The position it reports is the time at the end of the frame it just read, the same moment MTC reports. After a gap, the first frame is spent finding the bit rate again.
- `pchi-studio-api --audio-devices` lists what a `--features audio` build can see. Like MIDI, building for audio devices needs the system audio library (`libasound2-dev` on Linux).

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

## MIDI output

A rig device of type **MIDI output** sends a short MIDI message when a cue reaches it, for a lighting desk that takes program changes, a sampler, a playback machine or a controller's lights. In the Rig tab add **MIDI output** and put part of the MIDI port's name in **host** (any case; blank means the first port). There is no port number. `pchi-studio-api --midi-ports` lists the input and output ports the machine has (`in:` and `out:`). It serves the audio department unless you set another one on the device.

The cue's value says what to send, in a closed set of words:

| Value | Sends |
|---|---|
| `note 60` or `note 60 100 ch 2` | note on (velocity 100 unless given), then note off 100 ms later |
| `pc 12 ch 3` | program change |
| `cc 7 64 ch 1` | controller 7 set to 64 |
| `panic` | controller 123 (all notes off) on all 16 channels |
| `msc go 5`, `msc load 12.5 list 2 dev 3 fmt sound`, `msc all-off` | one MIDI Show Control message (below) |

- Numbers are sent as written, 0 to 127. A device that counts programs from 1 will show one more than you wrote. Channels are 1 to 16 (1 if not said). Velocity is 1 to 127, because velocity 0 means note off.
- **Nothing else can be sent:** no system-exclusive, no clock, no start or stop, no free bytes. A cue, an agent or the command bar cannot put its own bytes on the wire. Anything not in the table is refused, and the Doors check shows the refusal before the show.
- **MIDI has no acknowledgement.** "sent" means the message was handed to the port, not that the device acted on it. The Doors check says so.
- It needs a runtime built with `--features midi` (the same system library as MIDI input). Without it the Doors check marks the device as failing, and a cue says so instead of pretending.
- Not built yet: free system-exclusive, MIDI clock and timecode output.

### MIDI Show Control

MIDI Show Control (MSC) is the standard message a lighting or sound desk takes to GO, STOP or load a cue. **Out:** a MIDI output cue value that starts with `msc`:

```
msc <go|stop|resume|load|all-off|restore|reset> [cue] [list N] [dev N] [fmt lighting|moving-lights|sound|video]
```

It sends `F0 7F <device> 02 <format> <command> [cue [00 list]] F7`, the cue and list as ASCII digits and dots. Defaults: device 0, format lighting. `load` needs a cue; `all-off`, `restore` and `reset` take none; `go`, `stop` and `resume` may name one (without one, the desk acts on its own current cue).

- **Closed set:** only those seven commands and four formats. Pyrotechnics, machinery, rigging, lasers, timed go, set, fire and go-off are not built. Device is 0 to 111: a group or all-call message is refused, because it could move every desk at once. A cue cannot put other bytes on the wire.
- **No reply.** MSC has none, so "sent" means handed to the port.
- **Basis: memory.** The layout and codes are from memory of the MSC specification; no copy of the document was to hand. Nothing here has been seen to move a real desk. Check one against your desk's manual before relying on it: some desks count device ids from 1, and some need a cue list.

**In:** an input binding with `msc` presses an event, like a note:

```json
{"midi": [{"name": "Desk", "port": "loopMIDI", "bindings": [
  {"msc": "go", "cue": "5", "event": "NEXT"},
  {"msc": "stop", "device": 3, "event": "HOLD"}
]}]}
```

`cue`, `list`, `device` (default 0; an all-call message also matches) and `format` narrow what it takes; leave one out to take any. It can only press an event: not set a context value, not stop lasers or motion. It needs a place on the roster, like any press, and a scene's minimum holds it with a receipt. A sysex that is not plain MSC is ignored as before.

## What is tested, and what is not

- **Tested:** timecode maths at every rate (drop-frame included), the MTC decoder (every rate, lost messages, full frames), the
  jump rules, cues, `tc:` transitions and the minimum bypass, the OSC forms of timecode, MIDI parsing and bindings, and MTC
  driving a locked scene. OSC timecode was also played into a real runtime over real UDP at 25 fps: the cues fired at the right
  timecodes and the scene changed on its `tc:` transition.
- **MIDI Show Control:** the exact bytes of every command and format, the refusals (groups, all-call, pyro and other formats, bad cue numbers), reading back what was built, and which binding a heard message matches. Not run against a real desk or a real MSC sender.
- **MIDI output:** the words, the exact bytes (every channel, the limits, what is refused, and that no accepted value can make anything but a note, controller or program change), the descriptions and the Doors check are tested. The code that writes to a port compiles with `--features midi` but could not be run: the machine it was built on had no MIDI sequencer, so it has never sent a message to a port, real or virtual.
- **Compiled but not run:** the MIDI port code (`--features midi`) and the audio device code (`--features audio`) compile against the real `midir` and `cpal` libraries on Linux, but have
  not been run against a MIDI device, a sound card or a playback machine. Treat them as unproven until you have sent them real MTC or LTC. The LTC **decoder** is tested with synthetic LTC (every rate, level, polarity, speed, noise, chunking, a start in the middle) and a file driving a locked scene.
- **Not covered:** LTC on a real sound card and a real playback machine (a real signal may be dirtier than the synthetic ones), timecode in `rehearse` (a rehearsal replays presses, not timecode), and timecode restored after
  `--resume` (it is heard again from the source).
