# OSC input: a Stream Deck, tablet or button box presses things

Until now a show could only be run from Studio or over HTTP. A crew's first question is "can I put GO on a Stream Deck?".
An **input** listens for OSC on a UDP port and turns messages into the few things a button may do.

```bash
pchi-studio-api my-show.pchi --inputs inputs.json
```

```json
{"inputs": [{
  "name": "Stream Deck",
  "listen": "0.0.0.0:9000",
  "allow": ["192.168.1.50"],
  "bindings": [
    {"address": "/pchi/go",     "event": "NEXT"},
    {"address": "/pchi/drop",   "event": "DROP"},
    {"address": "/pchi/energy", "context": "energy"},
    {"address": "/pchi/stop",   "action": "laser.stop"}
  ]
}]}
```

## What a binding can do

| Binding | Does |
|---|---|
| `"event": "NEXT"` | sends that event to the show, like pressing GO. A message whose first argument is `0` or `false` (the button being released) is ignored; add `"press": false` to act on every message |
| `"context": "energy"` | sets that context value to the message's first argument (a fader, a sensor). Only changes are applied |
| `"action": "laser.stop"` | the hard stop: disarms every laser and sends BEYOND's blackout. The safe direction, so it is never held or refused |

Nothing else can be bound. An input **cannot arm a laser, edit the show, undo, try a cue, or send anything but these**. It acts
as the role `button`, which may only press.

## Safety, because a UDP port has no login

- **Listening beyond this machine needs an allow list** of sender addresses. Without one the runtime refuses to start: anyone
  on the network could otherwise press GO. Datagrams from any other address are dropped and counted.
- **At most 100 messages a second** are looked at per input. The rest are dropped and counted, so a faulty controller cannot
  flood the show.
- **A scene's minimum applies to buttons too.** A press that comes too early is held, with a receipt (see `scene-minimum.md`).
- **With a roster** (`roles.md`) an input is the role `button` and may press. Without one, an input behaves like any client of a
  show with no sign-in.
- **The record names the input**: `Stream Deck (button): /pchi/go sent 'NEXT': moved the show to DROP`. Presses that move the
  show, refusals and STOP LASERS are receipts. Context changes and releases are not (they can be fast); they are in the run.

## Seeing it

The Live tab shows each input: `OSC in · Stream Deck :9000 · 4 bindings · last /pchi/go 4 s ago · 12 pressed`, plus counts of
what was ignored (other senders, over the rate cap, refused). `GET /inputs` returns the same.

## What it does not do yet

- **MIDI.** A MIDI input needs a system MIDI library, so it goes with the timecode work (MIDI, LTC, Art-Net, sACN).
- **Wildcards and arguments in addresses** (`/scene/*`, the scene number in the address). Each address is bound exactly.
- **Changing inputs while running.** They are read at startup.
- **Authentication.** An allow list limits who may press, but anything that can spoof a sender address on your network can still
  press. Use a trusted network, as for OSC generally.
- Stream Deck software has no built-in OSC output: use its OSC plugin or a bridge such as Chataigne or Companion.
