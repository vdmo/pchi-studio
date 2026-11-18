# Link profiles: devices driven by a line of text over TCP

A **link profile** is a JSON file that says how a device that speaks a text command language over TCP is driven: what each cue sends, what answer means yes, how to read the device's state, and what must never be sent. The built-in one is [Universal Robots](ur.md) (`crates/pchi-studio-api/profiles/link/ur.json`). A crew can add its own with `--profiles <folder>`: a file whose `"transport"` is `"tcp-text"` is a link profile, anything else in the folder is an OSC profile ([profiles.md](profiles.md)).

The point of a file and not code: a robot's command language belongs to the robot, and a site may have its own (a KUKA controller running a receiver the integrator wrote has a command format only that site knows). A file can be read, reviewed and signed off by whoever is responsible for the machine. **The runtime sends only commands that appear in it.**

## The format

```json
{
  "id": "acme-arm", "name": "Acme Arm", "transport": "tcp-text", "dept": "motion", "port": 7000,
  "kinds": ["motion.run", "motion.stop"],
  "value": "[A-Z_]{1,32}",
  "needs_programs": true,
  "never": ["power on", "brake release"],
  "actions": {
    "motion.run": {
      "steps": [
        {"send": "STATUS", "expect": "READY", "fail": "the arm says {reply}"},
        {"send": "RUN {0}", "expect": "OK .*", "fail": "the arm would not run {0} ({reply})"}
      ],
      "confirm": {"send": "STATE", "expect": "RUNNING", "within_ms": 3000, "every_ms": 250}
    }
  },
  "status": {
    "poll_ms": 1000,
    "reads": [{"var": "state", "send": "STATE", "expect": "(\\w+)"}],
    "ready_when": [{"var": "state", "one_of": ["IDLE", "RUNNING"], "why": "its state is {value}"}]
  },
  "stop": [{"send": "HALT", "expect": "OK"}]
}
```

- **`value`** is the pattern the cue's value (the program name) must match completely. It is the only part of a command a cue can influence, and it goes into `{0}`. Use a strict pattern: no spaces, no punctuation you do not need.
- **`needs_programs`**: the value must also be on the device's own `programs` list (a rig node parameter). The default for a robot is that nothing may be run.
- **`steps`** run in order, each: `send` (a template; `{0}` is the only placeholder, and a command is one line), `expect` (a pattern the **whole reply** must match; if not, the request is `refused` with `fail`, where `{reply}` is what the device said, and **nothing further is sent**), `store` (keep the reply, or its first group, as a variable), `skip_if` (skip this step if a stored variable matches).
- **A value of its own for one action:** an action may carry its own `"value"` pattern, used instead of the profile's. Its bracketed parts are `{1}` to `{9}` in the commands (`"value": "(\\d{1,2})/(\\d{1,2})"` and `"send": "CUE {1} {2}"`). A command that uses a part the pattern does not have is refused when the profile is loaded, and what comes out of a value is never read as a placeholder again.
- **`request_end`** and **`reply_until`** (for a device that frames its own messages, such as XML, or ends lines with a carriage return): `request_end` is what is appended to a request (`"\n"` by default, `"\r\n"`, `"\r"`, or `""` for nothing); `reply_until` is the text that ends a reply (`"</Reply>"`, or a single `"\r"`), read however it arrives in pieces, up to 64 KB. Without it a reply is one line.
- **`greeting_refuse`**: `{"match": "^PJLINK 1 ", "why": "..."}`. If what the device says on connect matches, nothing is sent and `why` is the failure shown. It is how a device that asks for a password PCHI cannot give is said no to, instead of being sent commands it will refuse.
- **A status read** may use `"extract": {"state": "<State>(\w+)</State>", ...}` to take several variables out of one reply, instead of `var`.
- **`confirm`**: a read-back repeated until it matches (`running`) or time runs out (`accepted`, honestly "not seen").
- **`status`**: reads about every `poll_ms` while the runtime is sending. `ready_when` rules say when the device is ready; the first that fails gives the reason shown to people.
- **`stop`**: what STOP MOTION sends.
- **`never`**: commands this profile must never contain (compared in lower case, as a whole command or its first words). **A profile with a step that could send one is refused when it is loaded** (for XML commands too: `<Power>` or `<Brake .../>` is caught by a `never` entry `power` or `brake`), so an edit that adds `power on` cannot get in unnoticed.

## Built in: PJLink projectors

`profiles/link/pjlink.json` drives any projector that speaks PJLink class 1 (TCP 4352, lines ending in a carriage return). Add a device of type **Projector (PJLink)** in the Rig tab, with the projector's address.

| Cue kind | Value | Sends |
|---|---|---|
| `visual.power_on` | none | `%1POWR 1`, then reads `%1POWR ?` to see it warming up or on |
| `visual.input` | a two-digit input code, 11 to 59 (31 is digital input 1 on most projectors) | `%1INPT 31` |
| `visual.mute` | `11`/`10` picture mute on/off, `21`/`20` sound, `31`/`30` both | `%1AVMT 31` |

What it will **not** do, on purpose: power a projector off. The profile's `never` list forbids `%1POWR 0`, and the runtime refuses to load a profile that could send it, because switching a lamp off during its cool-down can damage it. It also has no readiness rule, so a projector in standby never blocks the cue that powers it on; its power and error state are read and shown.

Honest limits: a projector with a **PJLink password** is refused, with that reason, and nothing is sent (the password scheme is not built): turn authentication off on the projector. The command text and codes come from a projector maker's manual that was read; the carriage-return framing, the port and the greeting are from memory and search snippets, not from the PJLink specification. It has been run only against a pretend projector. The Studio Rig tab adds the device, but the cue kinds above are not yet offered in the Cue sheet's menus: write them in the show file or send them from the command line.

## A device that takes fixed lines

Many devices (a switcher, a media player, a relay board) take a short fixed line over TCP and answer `OK`. Nothing needs to be built for one: write a link profile that lists exactly the lines that may ever be sent. A starting point:

```json
{
  "id": "relay-board", "name": "Relay board", "transport": "tcp-text", "dept": "lighting", "port": 5000,
  "request_end": "\r\n", "kinds": ["lighting.look"], "value": "[0-9]{1,2}",
  "never": ["format", "reset", "firmware"],
  "hint": "a relay number, 1 to 16",
  "actions": {
    "lighting.look": {
      "value": "([1-9]|1[0-6])",
      "steps": [{"send": "RELAY {1} ON", "expect": "OK", "fail": "the board would not switch relay {1} ({reply})"}]
    }
  }
}
```

Put the file in a folder given to `--profiles`, add a device of type **OSC** (or any) with the parameter `profile` set to `relay-board`, and check it with the Doors check and a probe before relying on it. The same rules apply as for any link profile: only the lines in the file are sent, a cue only fills in the part the pattern allows, and the first answer that is not the expected one stops the sequence.

## Modbus TCP profiles (`"transport": "modbus-tcp"`)

For a PLC in front of a machine. Same format and guarantees; the differences:

- `unit` (default 1) and `port` (502 is the standard). No `request_end` or `reply_until`: Modbus frames its own messages.
- A step's `send` is one of `read_coil A`, `read_input A`, `read_reg A`, `read_ireg A`, `write_coil A 0|1`, `write_reg A V` (addresses count from 0). A read answers `A=value`; a write answers `OK` once the device has echoed it; a device that refuses answers `EXCEPTION <code> <name>` (a `refused`, not a broken link). The only placeholder in a Modbus command is `{n}`, the program's number from the device's list (`NAME=3`); a name with no number is refused.
- **`never_addresses`**: `["coil:30-39", "reg:100"]`. A profile with a step that writes one is refused at load, and each write is checked again as it is sent.
- **`wait_ms`** on a step waits after it succeeds (a PLC needs time to see a pulse). **`cleanup`** on an action runs if a step fails after a changing step (every Modbus write, or a text step marked `"changes": true`) was attempted: put back what the steps changed. If no changing step was attempted, nothing is written.

## What the runtime guarantees

- Only commands in the file are ever sent, and a cue chooses only a program name that passes the pattern and the allow-list. A cue, an agent and the command bar cannot send a command of their own.
- Each answer is checked; the first no stops the sequence; a timeout drops the connection, so a **late reply is never read as the answer to the next question**.
- Results are `accepted`, `running`, `refused` or `failed`: never "done".
- A device that reads as not ready holds cues and, if armed, is disarmed.
- Without `--send` nothing connects at all.

## What it does not do

It cannot know what a command does. A profile is only as safe as what a competent person decided it may send: review it with whoever is responsible for the machine, and run `pchi-gearcheck` on the real thing. It is not a safety system, and a PCHI stop is not an emergency stop. Only one line per reply is read, and there is no TLS (these interfaces have none): use a trusted, separate network for robots.
