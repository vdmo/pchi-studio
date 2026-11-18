# Serial output: write one line to a serial port from a closed list

A rig device can answer a cue by **writing one line of text to a serial port** (RS-232, or RS-485 through an adapter), for projectors, video switchers, relay boards and other gear controlled that way. The commands come from a profile file (`"transport": "serial"`), the same idea as [HTTP](http-output.md), [JSON-RPC](jsonrpc-output.md) and [MQTT](mqtt-output.md) output: the profile fixes the line settings and the command; a cue supplies only a **value**.

There is **no built-in serial profile**: the commands are the device's own, so you write one from its manual.

It needs a build with the serial feature:

```bash
cargo build --release -p pchi-studio-api --features serial
```

It needs **no system library** (the port is named, not searched for). Without the feature, a cue says so and the Doors check fails the device before the show. `pchi-studio-api --serial-ports` lists what the machine reports.

## What a cue can and cannot do

- The value must match the action's pattern, and cannot contain a control character, so it cannot end the line early or add a second command.
- A cue cannot choose the port or the line settings. The port is the device's `host`: `COM3`, `/dev/ttyUSB0`, `/dev/ttyACM0`, `/dev/cu.usbserial-…` or a pseudo-terminal `/dev/pts/N`. Any other name (`/etc/passwd`, `/dev/sda`) is refused, so that field cannot point at some other file.
- Laser and motion profiles are refused: a serial line gives no reliable acknowledgement or stop.

## What it does not do

- **Text lines only.** No binary bytes: no STX/ETX framing, no checksums, no raw hex. A device that needs them is not covered.
- **RS-485** is a serial port to this code. Use a USB adapter that switches direction by itself. Nothing here drives a direction pin, and nothing arbitrates a shared bus.
- **No acknowledgement unless the profile asks for one.** With no `expect`, `sent` means the bytes were handed to the port. With an `expect`, the first reply line must match it for `accepted`; a different line is `refused` with the device's words; no line within a second is `failed`.
- Line settings: baud 1200 to 230400, 7 or 8 data bits, parity none, even or odd, 1 or 2 stop bits, no flow control.
- Opening some adapters resets the device on the other end (an Arduino does). The port is opened on the first cue and kept open.

## Writing a profile

```json
{
  "id": "hall-projector",
  "name": "Hall projector",
  "transport": "serial",
  "dept": "visual",
  "baud": 9600,
  "data_bits": 8,
  "parity": "none",
  "stop_bits": 1,
  "kinds": ["visual.power_on", "visual.input"],
  "hint": "an input number, 1 to 9",
  "notes": "From the projector's RS-232 command table, page 40.",
  "actions": {
    "visual.power_on": {
      "send": "PON", "expect": "PON|ACK",
      "basis": "vendor", "source": "projector manual p. 40"
    },
    "visual.input": {
      "value": "([1-9])", "send": "IN{1}", "end": "crlf",
      "basis": "vendor", "source": "projector manual p. 41"
    }
  }
}
```

- `send` is up to 200 printable ASCII characters. `{0}` is the whole value, `{1}` to `{9}` the bracketed parts of its pattern; a literal brace is written twice.
- `end` is the line ending: `cr` (default), `lf`, `crlf` or `none`.
- `basis` is `vendor`, `community` or `memory` with a `source`, never `proven`. The Doors check says when rules rest on less than the vendor's document.
- Add a device of type **OSC** (or any) with the parameter `profile` set to your id and `host` set to the port. The cue kinds are not yet in the Cue sheet's menus.

## What is tested

The profile rules, the port-name rule, that no value can end the line, the exact bytes, the Doors check and dry-run text, and the message a build without the feature gives. With `--features serial` on Linux, a **pseudo-terminal pair** stands in for a cable, so the real port code writes a line that arrives byte for byte, accepts a matching reply, refuses a different one, fails on silence and fails on a port that is not there. **Not tested: any real serial hardware, a USB adapter, an RS-485 bus, or a real device's replies.**
