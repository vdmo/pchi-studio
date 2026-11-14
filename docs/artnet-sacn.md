# Art-Net and sACN

DMX over the network: PCHI can drive fixtures, dimmers and media servers directly, without a lighting console. Add a device of type
**Art-Net node** or **sACN (E1.31) output** in the Rig tab.

| Setting | Meaning |
|---|---|
| **host** | the node's address. Art-Net: required (a unicast address, or a broadcast address). sACN: leave blank to send to the universe's multicast address (`239.255.<high>.<low>`), or give a unicast address |
| **port** | defaults to 6454 (Art-Net) and 5568 (sACN) |
| **universe** | Art-Net 0 to 32767 (usually starts at 0); sACN 1 to 63999 |

One device is one universe of 512 channels.

## Writing a cue

A lighting cue's value is written in channels:

```text
1=255                    channel 1 at full
1-3=50%  10=0            channels 1 to 3 at half, channel 10 off
1-8=100% over 2          channels 1 to 8 fade up to full over 2 seconds
blackout over 3          every channel fades to 0 over 3 seconds
```

Levels are 0 to 255, a percentage, `full` or `out`. Terms are separated by spaces, commas or semicolons. Levels you do not mention stay
where they are, so each cue changes only what it names. A fade is linear, up to 600 seconds, and starts from wherever the levels are
at that moment (a second fade begins from the first one's current level).

Give a device a `patch` and cues can name fixtures instead (`fixtures.md`). The Doors check reads every DMX cue the same way: a value written for a console ("blue", "12") is a failure it reports before the
show. **Try a cue** and **Rehearse** show which universe gets which channels.

## Why it keeps sending

DMX receivers hold a level only while they keep being told. Art-Net nodes go dark when frames stop, and sACN receivers time out after a
couple of seconds. So the runtime keeps sending the whole universe twice a second when nothing changes, and 25 times a second during a
fade. A cue changes the levels and sends at once. If the runtime stops, the lights go dark: that is DMX, not a fault.

## What it does not do

- **It has not been run against a real Art-Net node, sACN receiver or fixture.** The packets are laid out as the standards say and
  are checked against that layout and over loopback UDP, nothing more. `pchi-gearcheck` asks for levels and for your word that the
  fixture reacted. Tick a line in `foh-proof.md` only when it has.
- One sender per universe: no merging with a console or other source, no priority handling beyond a fixed sACN priority of 100.
- Fixtures: you can write channels, or patch fixture types and write `wash1=blue` (see `fixtures.md`). Only generic types are built in.
- No feedback: nothing is read back from the node (no ArtPoll discovery, no sACN source discovery).
- A DMX device takes lighting cues only. Visual, laser and audio cues are refused.
- **Safety:** DMX gives no way to confirm a fixture is safe to light. Strobes and moving heads need the same care they always did.
