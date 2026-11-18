# Runtime reference: `pchi-studio-api`

The runtime is one process: it loads a show, runs it, serves Studio and other clients, routes cues, records the run and enforces roles.

```bash
pchi-studio-api <show-file> [options]
```

`<show-file>` is a Studio `.pchi` (showchart and rig) or a bare showchart (`.pchi.json`). With a bare showchart there is no rig, so cues are not routed to devices. It is optional only for the modes that do not run a show (`--midi-ports`, `--audio-devices`, `--new-roster-entry`, `--replay`, `--export-run`).

Every option with an environment variable can be set that way. A flag wins.

## Options

### Serving

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--bind ADDR` | | `127.0.0.1:3100` | Where to listen. A non-local address without `--token` prints a warning |
| `--token SECRET` | `PCHI_TOKEN` | none | Required on every request (`?token=` or `Authorization: Bearer`) |
| `--allow-origin ORIGIN` | `PCHI_ALLOW_ORIGIN` | none | Let a Studio served from this origin talk to the runtime from the browser. Repeat or comma-separate. `*` is refused |
| `--behind-tls` | | off | Say HTTPS is provided in front (reverse proxy); only silences the clear-text warning |
| `--send` | | off | Really send cues (UDP/OSC to each device's host and port). Without it deliveries read `dry-run` |

### Recording and resume

| Flag | Env | Meaning |
|---|---|---|
| `--db FILE` | | Record to this SQLite file. Default: in memory, lost on exit |
| `--resume` | | Carry on the run in `--db`: the scene, context and cues still ahead. Cues that came due while down are skipped and listed; lasers start disarmed; departments report READY again. Needs `--db` |
| `--record FILE` | `PCHI_RECORD` | Also write every record as one line, flushed at once, synced each second ([replay.md](replay.md)) |
| `--export-run FILE` | | Write the latest run in `--db` as a one-file recording and exit |
| `--replay FILE` | | Print a recording as a timeline and check it replays. Needs no show file |
| `--rehearse FILE` | | Play this edited show against the run in `--db` (or `--recording`) and print what would differ. No gear |
| `--recording FILE` | | With `--rehearse`: use this recording instead of `--db` |

### People, keys, governance

| Flag | Env | Meaning |
|---|---|---|
| `--edit-key KEY` | `PCHI_EDIT_KEY` | Operator key (`X-Pchi-Edit-Key` or `?edit_key=`) to send an edit, undo a send, or test-fire |
| `--roster FILE` | `PCHI_ROSTER` | Named people and roles ([roles.md](roles.md)) |
| `--new-roster-entry NAME:ROLE[:DEPTS]` | | Make a roster entry with a fresh random key, print the key once and the entry, exit. Roles: operator, editor, lso, guest, agent, button |
| `--session-hours N` | | Sign-in lifetime, 1 to 168. Default 8 |
| `--sessions FILE` | `PCHI_SESSIONS` | Keep sessions across restarts (token hashes only) |
| `--library DIR` | `PCHI_LIBRARY` | Keep a library of saved and planned shows on the server: see [library.md](library.md) |
| `--governance DIR` | `PCHI_GOVERNANCE` | Signed, hash-chained receipt for every decision ([governance.md](governance.md)) |
| `--governance-key FILE` | `PCHI_GOVERNANCE_KEY` | Where the receipt-signing key lives (default: in the `--governance` folder) |
| `--audience-token TOKEN` | `PCHI_AUDIENCE_TOKEN` | Opens `POST /vote` and nothing else for an audience gateway. Must differ from `--token` |

### Inputs and rig data

| Flag | Env | Meaning |
|---|---|---|
| `--inputs FILE` | `PCHI_INPUTS` | OSC, MIDI and LTC inputs ([osc-input.md](osc-input.md), [timecode-and-midi.md](timecode-and-midi.md)) |
| `--profiles DIR` | `PCHI_PROFILES` | Extra tool profiles (`*.json`). A built-in's id is replaced ([profiles.md](profiles.md)) |
| `--fixtures DIR` | `PCHI_FIXTURES` | Extra DMX fixture files ([fixtures.md](fixtures.md)) |
| `--midi-ports` | | List MIDI input and output ports and exit (build with `--features midi`) |
| `--audio-devices` | | List audio inputs and exit (build with `--features audio`) |

### Failover

| Flag | Env | Meaning |
|---|---|---|
| `--failover-key KEY` | `PCHI_FAILOVER_KEY` | Secret a standby proves itself with. Must differ from every other token and key |
| `--standby-of HOST:PORT` | | Run as standby of that primary. Needs `--failover-key` |
| `--takeover auto\|manual` | | Standby: take over alone (default) or only on `POST /takeover` |
| `--takeover-after N` | | Standby: seconds of silence before takeover, 2 to 600. Default 6 |
| `--stall-grace N` | | Standby: extra seconds for a primary that accepts connections but does not answer. Default as long again as `--takeover-after`; 0 treats it as dead |
| `--standby-name NAME` | | The name the primary's record shows. Default `standby` |
| `--primary-token TOKEN` | `PCHI_PRIMARY_TOKEN` | Standby: the primary's `--token` if different |
| `--witness HOST:PORT` | `PCHI_WITNESS` | Use a witness: send only while holding its lease. Needs `--failover-key` |
| `--witness-ttl N` | | Lease length in seconds, 2 to 120. Default 6 |
| `--advertise HOST:PORT` | `PCHI_ADVERTISE` | The address others reach this runtime at, told to the witness. `GET /primary` reports it |

See [failover.md](failover.md).

### Memory

| Flag | Env | Meaning |
|---|---|---|
| `--memory FILE` | `PCHI_MEMORY` | Agent memory ([memory.md](memory.md)). Never in the show's path |

## HTTP and WebSocket API

Every route needs the token if `--token` is set, except where noted. Anything that changes the show or arms a laser also needs the role or key described in [roles.md](roles.md) and [governance.md](governance.md).

### Reading the show

| Route | Method | Returns |
|---|---|---|
| `/state` | GET | scene, time in scene, context, timecode, proposals waiting |
| `/events` | GET | recent events from the log |
| `/ready` | GET | readiness per department and device |
| `/report` | GET | cue timing, failed deliveries, edits, who did what |
| `/run-report` | GET | the hand-over page as HTML ([report.md](report.md)) |
| `/preflight` | GET | the Doors check: the show against the rig |
| `/fixtures`, `/profiles`, `/captures`, `/inputs` | GET | what is loaded |
| `/recording` | GET | the latest run as a one-file recording |
| `/replay` | GET, POST | the laid-out view with the replay check; POST a recording to view it |
| `/role` | GET | `active`, `standby`, `fenced` or `paused` ([failover.md](failover.md)) |
| `/primary` | GET | where the active runtime is |
| `/whoami` | GET | who the runtime thinks you are, and what you may do |
| `/describe` | GET | the runtime describing itself to the caller: their powers, the show now, the rig, what needs attention, the rules ([mcp.md](mcp.md)) |
| `/ws` | WebSocket | the live feed; also accepts `ready` and input messages |

### Driving the show

| Route | Method | Does |
|---|---|---|
| `/input` | POST | `{"event":"NEXT"}`, a context change, a reset. Held if the scene has not run its minimum |
| `/ready/:dept` | POST | `{"ready":true,"note":"..."}` |
| `/devices/:id/arm` | POST | arm or disarm a laser (operator; disarm is always allowed) |
| `/laser/stop` | POST | STOP LASERS: disarm all and send each tool's safe message |
| `/motion/stop` | POST | STOP MOTION: disarm every motion device and send each tool's stop message. A software request, **not an emergency stop** ([motion.md](motion.md)) |
| `/devices/:id/try`, `/devices/:id/test` | POST | try a cue or test-fire a device |

### Editing the running show

| Route | Method | Does |
|---|---|---|
| `/show` | POST | send an edited show. `?dry=1` previews. An agent's send becomes a proposal (HTTP 202) |
| `/show/undo` | POST | put the show back as before the last send |
| `/rehearse` | POST | play an edited show against the run; read-only |
| `/proposals`, `/proposals/:id/:verb` | GET, POST | list, approve or reject |

### Governance, audience, sign-in

| Route | Method | Does |
|---|---|---|
| `/governance/export` | GET | signed receipts, for offline verification |
| `/signin`, `/signout` | POST | exchange a key for a session token |
| `/vote` | POST | audience vote (needs `--audience-token`) |
| `/polls`, `/polls/:id/:verb` | GET, POST | list, open and close polls |

### Memory

`GET /memory?q=`, `GET /memory/list`, `POST /memory`, `POST /memory/derive`, `POST /memory/:id/verify|supersede|settle|archive`. See [memory.md](memory.md).

### Failover internals

`/replicate` (the standby reads the log), `/fence` (the standby tells a primary it has taken over), `/takeover` (on a standby). These need `--failover-key`.

## Exit and error behaviour

- A failed or unreachable send is reported and never stops or delays the show.
- A show file that fails validation is refused at startup, with the reason.
- Studio can only edit what the runtime lets its role edit; refusals are written to the record.

## Related

[tools-reference.md](tools-reference.md) for the other binaries; [show-file.md](show-file.md) for the file format; [security.md](security.md) for how the keys, tokens and roles fit together.
