# Tool profiles are data

How a PCHI cue becomes the OSC a tool understands used to be code. It is now a small JSON file per tool, so a crew can add a tool or
correct a message without touching Rust. The seven built-in profiles (Resolume, grandMA3, ETC Eos, X32, BEYOND, QLab, MagicQ) are
files too: `crates/pchi-studio-api/profiles/*.json`. `GET /profiles` lists everything loaded.

## Use your own

1. Put one or more `*.json` files in a folder (each file is one profile, or a list of them).
2. Start the runtime with `--profiles <folder>` (or `PCHI_PROFILES`). Run `pchi-gearcheck` with the same flag.
3. In Studio's Rig tab, add a device (type **OSC**, or any), set its **host** and **port**, and add a parameter `profile` with
   the profile's id. A device that names a profile also takes its **department** from it, unless you set one.

A profile file with a built-in's `id` **replaces** it. If any file in the folder is wrong, the runtime refuses to start and names
the file and the problem, rather than quietly leaving a tool on its old mapping.

## The format

```json
{
  "id": "acme-desk",
  "name": "Acme Desk",
  "dept": "lighting",
  "kinds": ["lighting.look"],
  "hint": "go N (a playback number)",
  "rules": [
    {"match": "(?i)go (\\d{1,3})", "send": "/acme/go/{1}",
     "args": [{"int": "{1}"}, {"string": "go"}],
     "basis": "vendor", "source": "the Acme manual, page 41"},
    {"match": "(?i)off", "send": "/acme/off", "basis": "community"}
  ],
  "probe": {"address": "/info"},
  "stop": [{"address": "/acme/off"}]
}
```

| Field | Meaning |
|---|---|
| `id`, `name` | `id` is a to z, 0 to 9 and `-`; it is what a device's `profile` names |
| `dept` | `visual`, `lighting`, `laser` or `audio`: which department the tool serves |
| `kinds` | the action kinds it handles (`lighting.look`, `audio.*`). Anything else is "no mapping" |
| `hint` | what to tell the person when a value fits no rule |
| `error` | optional full message; `{value}`, `{hint}` and `{name}` are filled in |
| `rules` | tried in order; the first whose `match` fits the cue's value wins |
| `probe` | a harmless message that makes the tool answer, so the link can be watched (only where the tool really does) |
| `stop` | what STOP LASERS sends to this tool (lasers only) |

A rule:

- **`match`** is a regular expression that must fit the **whole** value (you do not need `^` and `$`). `(?i)` makes it ignore case.
- **`send`** is the OSC address. `{0}` is the whole value, `{1}`, `{2}` the parts in brackets; `{1|lower}` and `{1|upper}` change case.
- **`args`** are `{"int": ...}`, `{"float": ...}` or `{"string": ...}`, with the same `{n}` placeholders.
- **`kinds`** (optional) limits a rule to some of the profile's kinds. **`ignore_value`** (instead of `match`) is for a command that
  takes no value, such as a blackout.
- **`basis`** says what the rule rests on: `vendor` (the vendor's own documentation), `community` (users or third-party manuals) or
  `memory` (not confirmed). **`source`** says where. Be honest here: it is what the Doors check shows the crew.

## What a profile cannot do

The format keeps a bad or careless profile from sending something dangerous:

- Every address that is built must be a plain OSC address (letters, digits and `/_-.`), whatever the cue's value was.
- A string argument may not contain control characters or exceed 200 characters; an `int` must really be a whole number.
- A value must match a rule's pattern **whole**, so a profile only ever sends what its patterns allow. The grandMA3 profile lists
  the playback command words and nothing else, which is why a show file cannot delete or store anything on a console. Write your
  own patterns as tightly as that.
- A profile is a message format. It cannot read files, run anything, or change the show.

## What the Doors check says about it

For every device whose profile has rules that are not `vendor`, the Doors check warns:
*"Behringer X32: 1 of 1 message rule(s) in the Behringer X32 / M32 profile rest on memory rather than the vendor's documentation:
prove them with pchi-gearcheck before relying on them."* A warning does not stop the show. It tells the operator which tools
have not yet been proven against the real thing.

## What this does not do

- It does not make a message correct: a rule is only as good as its `source`. `pchi-gearcheck` is how you find out whether the real tool
  reacts.
- Profiles are loaded at startup, not while running.
- Receiving (feedback from the tool) is not described in a profile yet: only a `probe` for tools that answer one.
- Studio does not edit profile files. A profile is a text file you write, share and keep with the show.
