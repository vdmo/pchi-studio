# Protocol roadmap: reaching the widest list of tools

Where PCHI Studio stands on protocols, and the order to widen it. [positioning.md](positioning.md) says protocol reach is where Medialon and Chataigne are ahead; this is the plan to close that, honestly.

## The rule for every addition

A protocol only counts as **supported** when all of these are true. Otherwise it is listed as *started* or *not built*.

1. **Exact bytes are tested:** a test decodes what would go on the wire for a given cue.
2. **The basis is written down:** `vendor`, `community` or `memory`, with a source ([profiles.md](profiles.md)). The Doors check shows anything not `vendor` or `proven`.
3. **Nothing says "proven"** until someone has seen the real tool do the right thing ([gear-check.md](gear-check.md)).
4. **It is safe by construction:** a closed set of messages, an allow-list where a wrong message could hurt someone, and a failed send never stops the show.
5. **It is documented,** including what it does *not* do.

Counting a protocol that fails these would make the list longer and the claim worse.

## What exists now (checked against the code)

| Direction | Protocol | State | Where |
|---|---|---|---|
| Out | OSC over UDP, generic `/pchi/...` | built | `output.rs` |
| Out | OSC tool dialects: Resolume, grandMA3, Eos, X32, BEYOND, QLab, MagicQ | built as data files; none run on the real tool | `profiles/*.json` |
| Out | OSC to a robot-motion device (allow-listed programs) | built, simulated only | `profiles/motion.json` |
| Out | Art-Net and sACN (DMX), fades, fixture files | built | `dmx.rs`, `fixtures.rs` |
| Out | OBS (obs-websocket 5) | built, run only against a fake OBS | `obs.rs` |
| Out | Text over TCP from a profile file: Universal Robots Dashboard, a KUKA example | built, pretend robot only | `textlink.rs`, `profiles/link/` |
| Out | Modbus TCP (a PLC route) | built, simulated PLC only | `modbus.rs`, `plcsim.rs` |
| In | HTTP, WebSocket | built | `lib.rs` |
| In | OSC (Stream Deck, tablets, button boxes) | built | `inputs.rs` |
| In | MIDI notes, controllers and MIDI Time Code (needs `--features midi`) | built; the transport is not tested on real MIDI hardware | `midi.rs` |
| In | LTC timecode from an audio input (needs `--features audio`) | built | `ltc.rs` |
| Import | CSV, QLab 4 cue lists | built | `import.rs` |

**Not built:** MIDI *output* (including MIDI Show Control), HTTP/REST output, serial (RS-232/RS-485), GPIO, MQTT, PJLink, JSON-RPC, UDP/TCP raw-text send other than through a TCP link profile, NDI control, SMPTE/LTC *output*, Ableton Link, MIDI Show Control *input*. A search of the code for `serialport`, `reqwest`, `mqtt`, `pjlink` and `visca` finds none of them.

## Order of work, cheapest and most useful first

The profile format is data, so **adding a tool that speaks OSC or lines of text costs a JSON file and its tests, not Rust**. That is where the list grows fastest. New Rust is needed only for new transports.

### Tranche A: more OSC tool dialects (data only)

Candidates, each needing a source check before its basis is set above `memory`: **Reaper**, **Bitfocus Companion** (button press over OSC), **Millumin**, **Ableton Live** (through the AbletonOSC add-on), **TouchDesigner** (user-defined, so a documented pattern rather than a profile), plus the mixers and desks named in [industry-tools.md](industry-tools.md) as "from memory" (Wing, Avolites, Hog).
*Done means:* a profile file, a byte-exact test per rule, honest `basis` and `source`, a line in [profiles.md](profiles.md).

### Tranche B: more text-over-TCP devices (data only)

**PJLink** (projectors, a published standard; with a `never` list so a show file cannot, for example, power a projector off unless the profile allows it), **Dataton WATCHOUT**, **Extron and Kramer** control strings, a generic "send this fixed line" profile. Each is a file for the existing text-link engine.
*Done means:* as above, plus the `never` list reviewed and the reply that means "yes" written from the vendor's document.

### Tranche C: new transports (Rust)

In this order, because each is testable without gear by checking bytes:

1. **MIDI output**, then **MIDI Show Control** out and in. Notes, program change and controller, and MSC cues to lighting and sound desks. It extends `midi.rs`, behind `--features midi`.
2. **HTTP/REST output** for Companion, PTZ cameras and webhooks. A closed list of URL templates in a profile, like the OSC ones; no free-form URLs from a cue.
3. **JSON-RPC over TCP** (the style used by Q-SYS and some media servers).
4. **MQTT publish** (installations, sensors, building systems), with a topic allow-list.
5. **Serial RS-232/RS-485**, behind a feature flag because it needs a system library.
6. **SMPTE/LTC output** and **Ableton Link**, last, since they are clocks, not cues.

*Done means:* encode tests, a stand-in receiver in the style of `ursim.rs` or `plcsim.rs`, a Rig tab entry, `gearcheck` coverage, a doc page that says what was and was not tried.

### Not planned yet

**GPIO** (needs hardware this project does not have), **proprietary binary protocols** (ATEM, disguise, Pixera native), **DALI/KNX/BACnet** (building systems). Each is real work and each waits for someone who owns the gear.

## Today's schedule

Each slice ends with a commit and push, so progress is visible and nothing sits half done. I do not give clock times: the length of a slice depends on what a source check turns up.

| # | Slice | Ends with |
|---|---|---|
| 1 | Source-check and add the first Tranche A dialects (Reaper, Companion, Millumin, AbletonOSC) | **Built, not yet committed.** Ableton, Millumin and Companion rest on documents that were read; REAPER rests on search results quoting its config file. Byte tests pass; nothing tried on a real tool |
| 2 | PJLink and the generic fixed-line profile (Tranche B) | **Built, not yet committed.** PJLink class 1 as a link profile (power on only, input, mute), with four small engine changes it needed: carriage-return line ends, per-action values with `{1}` to `{9}`, a greeting that can refuse a device, and replies ended by a carriage return. The fixed-line device is a documented recipe, not a built-in. No password support; not tried on a real projector |
| 3 | MIDI output and notes/program/controller (Tranche C1) | encode tests, stand-in receiver; commit |
| 4 | MIDI Show Control out and in | tests against the MSC byte layout; commit |
| 5 | HTTP/REST output with URL templates (C2) | tests, a stand-in server; commit |
| 6 | Update this page, [industry-tools.md](industry-tools.md) and [positioning.md](positioning.md) to the new truth | commit |

If a slice finds a source I cannot confirm, the rules go in as `memory` and the Doors check will say so. The schedule slips before the honesty does.

## What this will not get to

Real-gear proof. Until the tools have been tried on a real rig, every protocol above is *built and tested against bytes and stand-ins*, which is a different claim from *works with the tool*. The count of protocols will go up today; [foh-proof.md](foh-proof.md) will not.
