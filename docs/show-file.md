# The show file

A show is one JSON file. Studio saves it as `<name>.pchi`; the runtime and every tool read it. A **bare showchart** (what the examples in `crates/pchi-showchart/examples/*.pchi.json` are) holds only the state machine; a **full `.pchi`** also holds the rig.

## Full `.pchi`

```json
{
  "version": "1.0",
  "project": "Spring Gala",
  "name": "main-show",
  "nodes": [ /* the rig: devices and systems */ ],
  "edges": [ /* how they connect */ ],
  "timeline": [],
  "assets": [],
  "showchart": { /* the showchart, below */ }
}
```

`timeline` and `assets` are kept for compatibility and are empty; cues live inside the showchart's scenes.

### Rig nodes

```json
{
  "id": "qlab-1",
  "type": "qlab",
  "label": "Playback",
  "position": { "x": 120, "y": 80 },
  "dept": "audio",
  "params": { "host": "192.168.1.50", "port": 53000 }
}
```

- `type` is one of: control (`cue`, `condition`, `timer`, `schedule`, `trigger`), scene (`scene`, `camera`, `lighting`, `environment`), renderer (`vork`, `unreal`, `touchdesigner`, `notch`), distribution (`ndi`, `osc`, `webrtc`, `rtsp`), venue (`ghost-arcade`, `projector`, `led-wall`, `audio-zone`, `laser`, `lighting-desk`, `audio-console`), tool profiles (`resolume`, `grandma3`, `eos`, `x32`, `beyond`, `qlab`, `magicq`, `artnet`, `sacn`, `obs`), and intelligence (`director-agent`, `vision-agent`, `audio-agent`).
- `dept` is `visual`, `lighting`, `laser` or `audio`. Left out, it is inferred from the type.
- `params` carries `host`, `port`, `listen` (feedback port) and, for DMX, `universe`.
- An edge says what a link does: `activate`, `stream`, `trigger` or `control`.

## The showchart

```json
{
  "id": "timelines-demo",
  "name": "Timelines Demo",
  "version": "0.1.0",
  "initial": "PRESHOW",
  "context": { "energy": 0.3 },
  "guards": { "energy_high": "energy > 0.7" },
  "cue_seq": 12,
  "states": [ ... ],
  "transitions": [ ... ]
}
```

| Field | Meaning |
|---|---|
| `initial` | the scene the show starts in (required, must exist) |
| `context` | starting values guards read |
| `guards` | named expressions, for example `"energy_high": "energy > 0.7"` |
| `cue_seq` | highest cue number ever handed out, so a deleted number is not reused |
| `states` | the scenes |
| `transitions` | the arrows between them |

### Scenes (`states`)

```json
{
  "id": "INTRO",
  "label": "Introduction",
  "min_ms": 8000,
  "enter_actions": [ { "kind": "scene.activate", "payload": { "scene": "intro" } } ],
  "exit_actions": [],
  "timeline": {
    "duration_ms": 8000,
    "tc_start": "00:01:00:00",
    "lanes": [ ... ]
  }
}
```

- `min_ms`: a scene that must run this long before an input can move it on ([scene-minimum.md](scene-minimum.md)). A timer shorter than it is a validation error.
- `enter_actions`, `exit_actions`: run when entering or leaving.
- `timeline.duration_ms`: the ruler's length in the editor. Not enforced.
- `timeline.tc_start`: lock the scene's clock to timecode `HH:MM:SS:FF` (`;` before frames for drop-frame) ([timecode-and-midi.md](timecode-and-midi.md)).

### Lanes and cues

```json
{ "id": "lighting", "name": "Lighting", "dept": "lighting", "device": "grandma-1",
  "cues": [
    { "id": "l1", "at_ms": 2000, "length_ms": 1000, "num": 12, "label": "warm up",
      "action": { "kind": "lighting.look", "payload": { "look": "warm" } } }
  ] }
```

- `dept` routes a cue to the department; `device` (optional) picks one device when several serve it.
- `at_ms` is the offset from entering the scene. `length_ms` is display only.
- `num` is the cue number a stage manager calls (12, or 12.5).
- `action.kind` decides the OSC address: `lighting.look` becomes `/pchi/lighting/look`, or the tool's own address through a profile ([profiles.md](profiles.md)).

### Transitions

```json
{ "source": "INTRO", "target": "DROP", "event": "DROP" }
{ "source": "DROP", "target": "FINALE", "after_ms": 30000 }
{ "source": "INTRO", "target": "DROP", "event": "GO", "guard": "vote.next.winner == hype" }
```

A transition needs an event or `after_ms` (or both spellings on separate transitions). A timecode trigger uses the event `tc:HH:MM:SS:FF`. Two unguarded transitions on the same event from one scene are an error: the second could never fire.

## Validation

`pchi validate show.pchi.json` (a bare showchart) and the runtime (on either kind of file) check: unknown initial scene, duplicate scene ids, unknown transition source or target, ambiguous transitions, missing triggers, invalid guards, a timer shorter than the scene minimum, a bad timecode, and unreachable scenes.

## Recordings are a different file

A run recording (`pchi-run/1` one-file JSON, or `pchi-runlog/1` lines written by `--record`) holds the show *as it ran*, the inputs and everything that happened. See [replay.md](replay.md). It holds no keys, tokens or passwords but does hold names and rig addresses: treat it like the show file.

## Related files

| File | Used by | Read |
|---|---|---|
| roster JSON | `--roster` | [roles.md](roles.md) |
| inputs JSON | `--inputs` | [osc-input.md](osc-input.md), [timecode-and-midi.md](timecode-and-midi.md) |
| profiles folder | `--profiles` | [profiles.md](profiles.md) |
| fixtures folder | `--fixtures` | [fixtures.md](fixtures.md) |
| governance folder | `--governance` | [governance.md](governance.md) |
| golden set JSON | `pchi-memory eval` | [memory.md](memory.md) |

The roster, the sessions file and the governance folder hold keys or key hashes. Keep them out of version control and out of the show folder you share.
