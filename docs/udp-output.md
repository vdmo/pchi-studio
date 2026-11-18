# UDP text output

A cue becomes one datagram of text sent to one device, from a profile. Many show controllers, media servers and relay boxes take commands this way. Cues can only fill in a value; the commands, the port and the line ending are in the profile, and the destination is the device's `host` and `port` in the rig.

## A profile

Put a file in your profiles folder (`--profiles DIR`) with `"transport": "udp"`:

```json
{
  "id": "stage-box", "name": "Stage relay box", "transport": "udp", "dept": "lighting", "port": 7000,
  "kinds": ["lighting.cue", "lighting.blackout"],
  "hint": "a cue number, 1 to 999",
  "notes": "Commands as the maker's manual lists them.",
  "actions": {
    "lighting.cue": {"value": "([1-9][0-9]{0,2})", "send": "GO {1}", "end": "lf", "basis": "vendor", "source": "manual p. 12"},
    "lighting.blackout": {"send": "BLACKOUT", "basis": "vendor", "source": "manual p. 12"}
  }
}
```

- `dept` is `visual`, `lighting` or `audio`. Laser and motion are refused: UDP gives no acknowledgement and no way to stop.
- `value` is a pattern the whole cue value must match; `{1}` to `{9}` are its bracketed parts. A value cannot contain a control character, so it cannot end the text early or add a second command.
- `end` is `none` (default), `cr`, `lf` or `crlf`. `send` is 1 to 200 printable ASCII characters. Use doubled braces for a literal brace.
- `basis` is `vendor`, `community` or `memory`, with a `source`. Nothing may call itself proven. The Doors check warns about any profile that rests on less than the vendor's own document.
- Add a device of this profile in Studio with the profile's id, and set its host (and port, if the profile has none).

There is no built-in UDP profile: the commands are the device's own, so each one is yours to write from the device's manual.

## What it will and will not do

- **Unicast only.** A broadcast, multicast or unspecified address is refused, so one cue cannot reach every machine on a network. A subnet-directed broadcast (for example 192.168.1.255) looks like a normal address and is not caught: do not put one in the rig.
- **`sent` is the best outcome.** The datagram was handed to the network. UDP gives no acknowledgement, so nothing says it arrived or that the device acted on it. The Doors check says so for every UDP device.
- **Text only.** No binary bytes, no checksums, no replies read.
- **Names are resolved when the cue is sent**, and the result is checked then, so a name that resolves to a multicast address is refused too.
- **No authentication or encryption.** Anyone on the network can send the same datagram; use it on a show network.

## What was tested

- Profile checks: bad dept, port, characters, placeholders without a pattern, unknown basis and line ending, kinds with no action.
- The text a cue makes, and the values that are refused (out of range, a control character, wrong cue kind, a value where none is allowed).
- The destination taken from the rig only, with odd hosts refused.
- Broadcast, multicast and unspecified addresses refused, before a socket is opened.
- A real datagram sent over loopback arrives with exactly the profile's bytes; a refused cue sends nothing.
- The Doors check and the "what would this send" text.

## What was not

- No real device was ever sent to. There is no vendor profile to test against, so nothing here shows any device's commands are right.
- No reply is read, so a device that is off, on another address or ignoring the text looks the same as a working one.
- Cue kinds for these devices are not in the Cue sheet menus; type the kind in, as for the other profile-driven outputs.
