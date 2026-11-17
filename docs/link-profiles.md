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
- **`confirm`**: a read-back repeated until it matches (`running`) or time runs out (`accepted`, honestly "not seen").
- **`status`**: reads about every `poll_ms` while the runtime is sending. `ready_when` rules say when the device is ready; the first that fails gives the reason shown to people.
- **`stop`**: what STOP MOTION sends.
- **`never`**: commands this profile must never contain (compared in lower case, as a whole command or its first words). **A profile with a step that could send one is refused when it is loaded**, so an edit that adds `power on` cannot get in unnoticed.

## What the runtime guarantees

- Only commands in the file are ever sent, and a cue chooses only a program name that passes the pattern and the allow-list. A cue, an agent and the command bar cannot send a command of their own.
- Each answer is checked; the first no stops the sequence; a timeout drops the connection, so a **late reply is never read as the answer to the next question**.
- Results are `accepted`, `running`, `refused` or `failed`: never "done".
- A device that reads as not ready holds cues and, if armed, is disarmed.
- Without `--send` nothing connects at all.

## What it does not do

It cannot know what a command does. A profile is only as safe as what a competent person decided it may send: review it with whoever is responsible for the machine, and run `pchi-gearcheck` on the real thing. It is not a safety system, and a PCHI stop is not an emergency stop. Only one line per reply is read, and there is no TLS (these interfaces have none): use a trusted, separate network for robots.
