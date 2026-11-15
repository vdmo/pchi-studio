# Troubleshooting

Organised by what you see. Each answer says where to look next.

## The runtime will not start, or warns

| You see | Why | Fix |
|---|---|---|
| `warning: --send has no effect: <file> has no rig` | the file is a bare showchart (`.pchi.json`) with no devices | open it in Studio, add devices on the Rig tab, **Save** as `.pchi` |
| `warning: listening on ... with no --token` | a non-local `--bind` and nobody to stop other machines sending inputs | add `--token` |
| `warning: ... over plain HTTP: keys, session tokens ... cross the network in clear text` | a roster or edit key plus a non-local bind | put HTTPS in front ([tls.md](tls.md)), then add `--behind-tls`; or bind to `127.0.0.1` |
| `warning: nobody on the roster is an operator and there is no --edit-key` | nobody can run the show or send edits | add an operator entry: `pchi-studio-api --new-roster-entry Alex:operator` |
| `warning: N key(s) ... are written in clear` | the roster holds plain keys | replace each with a hashed entry from `--new-roster-entry` |
| A startup error naming the show | the show failed validation (unknown target, bad guard, a timer shorter than a scene minimum, bad timecode, unreachable scene) | run `pchi validate <file>`; the message names the scene |
| `--resume` refused | it needs `--db` | give `--db show.db` |
| `--standby-of` or `--witness` refused | they need `--failover-key` | add it, the same on every machine |

## Studio cannot reach the runtime

- The dev server proxies `/api` to `127.0.0.1:3100`. If the runtime is elsewhere, edit `pchi-studio-react/vite.config.ts` or start it on that address.
- If Studio is served from a different origin than the runtime, start the runtime with `--allow-origin http://host:port` (an exact origin; `*` is refused).
- With `--token`, Studio reads it from the browser: in the console, `localStorage.setItem('pchi.token', '<secret>')`.
- The Live tab says **Standby** in blue: you are talking to a standby; the active runtime is elsewhere (`GET /primary` says where). `--allow-origin` plus a backup address in Studio lets it follow a takeover.
- The banner says **paused**: the runtime lost its witness lease and sends nothing until it is granted it again ([failover.md](failover.md)).

## Cues do not do anything

1. **Are you in a dry run?** Deliveries read `dry-run` unless the runtime has `--send`.
2. **What does the Live tab say about the device's last cue?** `sent` means the UDP packet left this machine: UDP has no acknowledgement. `failed` carries the reason. "no laser device in the rig" means no device serves that department.
3. **Is the device chosen?** A lane with several devices for its department needs one picked. The Doors check lists it.
4. **Is a laser armed?** Cues to a disarmed laser are held. Arming needs an operator or the lso. STOP LASERS disarms.
5. **Does the tool understand the message?** Compare what the runtime sends (the gear check shows the exact address and values) with the tool's own OSC monitor. A profile rule marked unproven has never been run against the real tool ([foh-proof.md](foh-proof.md)).
6. **Did the scene end first?** Leaving a scene cancels cues that have not fired. Rehearse shows cues that would be cut off.
7. **After a restart:** cues that came due while the runtime was down are skipped and listed, not fired late.

## GO does nothing

- The press may be **held**: the scene has a minimum that has not elapsed. The GO button counts down; press again after.
- It may have **nothing to do** here: no transition out of this scene would fire now.
- A department the *next* scene needs is on hold: GO asks for a second press ("GO ANYWAY").
- Your role may not call the show: editors, lso and guests cannot press GO. The role banner says what you may do.
- A scene locked to timecode waits while the timecode is not running.

## "Not allowed", sign-in and keys

- **A refusal on send:** sending an edit, undo and test-fire need an operator or editor, or the `--edit-key`. Sign in with your own key on the Live tab.
- **Sign-in is slow after a restart:** a hashed key is checked with 200000 PBKDF2 rounds; a debug build takes a second or two. Use a release build.
- **A key was refused as expired:** the roster entry has an `expires` date.
- **Everyone was signed out by a restart:** use `--sessions FILE`.
- **An agent's edit did not apply:** it became a proposal (HTTP 202) under **Waiting for you**; a person approves it. If the show changed meanwhile, it is superseded and the agent proposes again.

## Failover

| You see | Look at |
|---|---|
| The standby does not take over | `GET /role` on the standby: it says whether it was *refused* by the primary (wrong key, or a log that is not the one it followed), whether the primary is `slow` (not replaced) or `stalled` (waiting `--stall-grace`), or whether the witness refused the lease |
| Two runtimes appear to be sending | run without a witness a cut network can do this; with a witness a primary pauses when it cannot prove it holds the lease. Run `pchi-netcheck` |
| After takeover desks cannot find the show | desks need `pchi-follow` at one address, or Studio's backup address; the audience gateway is not covered |
| Memory is empty after takeover | memory is not synced: run `POST /memory/derive` on the standby |

## Timecode

- Locked cues wait: no timecode is arriving. The Live tab shows the source and rate; the Doors check warns.
- Nothing fired after a jump: a forward step over half a second is a **locate**, and the cues jumped over are not fired late.
- MIDI or LTC not available: rebuild with `--features midi` or `--features audio` and install the system audio library (`libasound2-dev` on Linux). `--midi-ports` and `--audio-devices` list what is seen.

## Recordings and reports

- **"does not replay":** the recording was edited, or comes from a show that changed in a way that matters. Reading is never sending; nothing is re-sent.
- **A report says "signed record DOES NOT VERIFY":** compare against `scripts/verify_governance_chain.py`, which does not trust PCHI's code. The report trusts the key inside the export; compare it with the verifying key you wrote down at setup.
- **A last line cut off** in a `--record` file after a crash is dropped; damage anywhere else is an error.
- **Memory section missing from the report:** give `--memory FILE` or start the runtime with `--memory`.

## Memory says "I don't know"

That is a designed answer: nothing remembered, nothing about that, too old or uncertain, or two memories disagree. Read the reason and the closest notes; settle conflicts in the Memory panel. Memory does not understand synonyms ("smoke effect" will not find "haze") because it is keyword and character-trigram search, not a neural embedding ([memory.md](memory.md)).

## Import

QLab 5, `.noisette` and `.score` are refused in words. A QLab cue type with no known action is kept on a `notes` lane and sends nothing; map it with `--map TYPE=KIND`. The importer was tested on hand-written fixtures only: check the result in Studio.

## Building and tests

- `cargo build` fails on a missing system library only with `--features midi` or `audio`.
- Studio: `npm run typecheck` then `npm run test:sdk`.

## Still stuck

Collect: the startup output, `GET /role`, `GET /preflight`, the event console, and the recording (`--export-run`). A recording holds no keys or tokens but does hold names and addresses; read it before sharing it.
