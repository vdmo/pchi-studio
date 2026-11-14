# Capture a cue from a live tool

Learned from ossia score's Snapshot: instead of typing a cue number you already fired on the console, **let the console tell
PCHI**. While you run the real tool (rehearsal, a cue-to-cue), PCHI listens to what the tool sends back and offers each cue it
fired as a cue for the show, at the moment it was heard.

## Use it

1. Give the tool's device a **Listen port** in the Rig tab and point the tool's OSC output at this machine on that port
   (for Eos: its OSC *UDP TX* setting). Start the runtime with `--send` (it only listens while really sending).
2. Run the show, and fire cues on the console.
3. The Live tab fills a box, **Fired on the tools**: *Eos fired lighting 3/9 · INTRO at 0:12.0*. **Add to show** puts a
   lighting cue with that value into the scene it was heard in, at that time (kept to 50 ms), in the lighting lane (made if there
   is none). It is one step in Studio's history: Ctrl+Z takes it back. The × dismisses one.
4. Tidy the timing in the Timeline as usual (there is a quantise there).

The runtime remembers the newest 30 (`GET /captures`); a tool that repeats itself within 0.4 s counts once.

## What a tool's messages mean is data

A profile (`profiles.md`) can carry **capture rules**: which message from the tool means "I fired this cue", and the cue that
fires the same thing.

```json
"capture": [
  {"listen": "/eos/out/event/cue/(\\d+)/(\\d+(?:\\.\\d+)?)/fire", "kind": "lighting.look", "value": "{1}/{2}",
   "basis": "community", "source": "an ETC community post: Eos sends this when a cue is taken"}
]
```

- `listen` is a pattern that must fit the **whole** address; `{1}`, `{2}` are the bracketed parts, as in a profile's `send`.
- A captured cue must be one the profile can **send back to the tool**: its value goes through the profile's own rules, so a
  capture can never be a cue that fails on the night. A message that fits no rule (a stop, a status) is ignored.
- Like every rule it says what it rests on. The Live tab shows "community source" next to a capture whose rule is not the vendor's.

**Built in: Eos only**, and its basis is `community`: Eos sends `/eos/out/event/cue/<list>/<cue>/fire` when a cue is taken, per
forum posts, not an ETC page. The other tools' feedback is not described here, because I could not confirm what they send; the
**Heard from devices** list on the Live tab shows the addresses a tool really sends, and you can write a capture rule for it in
your own profile file.

## What it does not do

- It captures **which cue was fired**, not the levels a console is showing. It is not a snapshot of fader values.
- It adds to the show open in Studio: the scene must exist there (otherwise it says so). Open the show the runtime is running.
- The timing is when PCHI *heard* it: network and tool delay (usually a few milliseconds) are in it.
- Only tools with a listen port and a capture rule. Nothing is captured from a tool that stays silent.
