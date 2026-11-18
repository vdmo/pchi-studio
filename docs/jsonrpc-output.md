# JSON-RPC output: one request over TCP from a closed list

A rig device can answer a cue with **one JSON-RPC 2.0 request** over TCP, for gear that is controlled that way. The first one built in is **Q-SYS** (its QRC control protocol). The requests come from a profile file (`"transport": "jsonrpc-tcp"`), the same idea as [HTTP output](http-output.md): the profile fixes the method and the shape of the parameters; a cue supplies only a **value**.

## What a cue can and cannot do

- The value must match the action's pattern. It is then placed into the parameters as **data**: the request is built as JSON, not by joining text, so a value such as `x","method":"Evil` stays one string inside one field and cannot add a field, change the method or send a second message.
- A cue cannot choose the host, the method or the parameters' shape.
- A profile for the laser or motion departments is refused: a request and an answer give no stop and no readiness.

## What it does not do

- **No login.** A Q-SYS Core that requires a password is not reachable yet, and a profile cannot carry a password.
- One connection per cue: connect, send, read one answer, close. No subscriptions, no status polling, no readiness.
- No TLS.
- **`accepted` means the device answered with a `result`.** An `error` answer is `refused`, with the device's own message. Neither says the sound changed.

## Built in: Q-SYS (`qsys`)

Rig tab: "Q-SYS Core" (or `add qsys`), port 1710, department audio.

| Cue kind | Value | Sends |
|---|---|---|
| `audio.control` | `Control Name=number`, e.g. `Main Gain=-10` | `Control.Set` with `Name` and `Value` (a number) |
| `audio.snapshot` | `Snapshot Name/bank`, e.g. `Show Open/1` | `Snapshot.Load` with `Name` and `Bank` (a number) |

Each message ends with a zero byte.

**Basis: memory.** The method names, port and terminator are as remembered from Q-SYS's documentation, with no copy read and no Core tried. Confirm them against your Designer version.

## Writing your own

A file in the folder given to `--profiles`, like the HTTP one, with `terminator` (`nul`, `lf` or `crlf`) and, per cue kind, a `method` and `params`:

```json
"actions": {
  "audio.control": {
    "value": "([A-Za-z0-9_. -]{1,60})=(-?\d{1,6}(?:\.\d{1,4})?)",
    "method": "Control.Set",
    "params": {"Name": "{1}", "Value": "{2:number}"},
    "basis": "vendor",
    "source": "the vendor's document and page"
  }
}
```

In `params`, a string that is exactly `{k}` becomes part *k* of the value (`{0}` is all of it); exactly `{k:number}` becomes that part as a number. Any other string with a brace in it is refused when the profile loads. `basis` is `vendor`, `community` or `memory` with a `source`, never `proven`. Then add a device of type **OSC** (or any) with the parameter `profile` set to your id and its `host`.

## What is tested

The profile rules, the exact JSON built for each cue, that a hostile value stays inside its string, number conversion, how replies are judged (result, error, not JSON), and a **stand-in device** that receives exactly one zero-terminated request and answers with a result, an error, nonsense or nothing. **Not tested: a real Q-SYS Core.** The cue kinds are not yet in the Cue sheet's menus.
