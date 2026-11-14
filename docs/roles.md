# Roles: who may do what, per person

One shared operator key means everyone who knows it can do everything, and the record cannot say who did what. A **roster**
gives each person their own key and a role. The runtime then knows who is pressing GO, arming a laser or sending an edit,
refuses what their role does not allow, and writes their name in the run report and the signed receipts.

## Set it up

Make a roster file, a JSON list. Give each person a different key of at least 8 characters:

```json
{"people": [
  {"name": "Alex",  "role": "operator", "key": "..."},
  {"name": "Eddie", "role": "editor",   "key": "..."},
  {"name": "Sam",   "role": "lso",      "key": "..."},
  {"name": "Gus",   "role": "guest",    "key": "..."},
  {"name": "Nova",  "role": "agent",    "key": "..."}
]}
```

Start the runtime with `--roster people.json` (or `PCHI_ROSTER`). Each person opens Studio's Live tab and presses **Sign in**
(their key is kept in that browser tab only). The Live tab then says who you are and what you may do.
The roster is read at startup: to change it, restart the runtime (`--resume` carries the show on).
The file holds the keys, so protect it like the show file.

## The roles

| Role | May | May not |
|---|---|---|
| **operator** | everything: GO, send edits, approve or reject proposals, undo, arm lasers, try cues, test-fire | |
| **editor** | send edits, approve or reject proposals, undo, try cues, test-fire | GO, arm lasers |
| **lso** (laser safety officer) | arm lasers; try and test-fire **lasers only** | GO, edits, anything on other devices |
| **guest** | look: state, run report, doors check, rehearse | everything else |
| **agent** | a named agent: its edits become proposals; rehearse | everything else |

Everyone, signed in or not, may **disarm a laser, press STOP LASERS, rehearse an edit and read**. The safe direction is never
locked behind a role.

## How it fits with the rest

- **Identity comes from the key, not from what a client says.** A label on the request (`X-Pchi-Actor: person:Alex`) cannot
  make someone Alex. A client that declares `agent:` stays an agent even if it holds an operator's key.
- **GO needs a role once there is a roster.** A button box or department page that presses GO needs its own roster entry (an
  operator with a key of its own) or it is refused. With only the old single `--edit-key`, GO stays open as before.
- **The old operator key still works** next to a roster, as an operator named "operator". Keep it for emergencies or drop it.
- **Receipts and the run report name the person**: `Sam (lso): armed b1`, `Gus (guest): refused: guest may not arm a laser`.
  The run report's "Who did what" lists every decision in order. See `governance.md` for the signed receipts.
- **A named agent** (role `agent`) proposes under its own name (`agent:Nova`); a person approves it. An unnamed agent that just
  sends `X-Pchi-Actor: agent:<name>` works as before.

## What this does not do

- **Keys are shared secrets.** Anyone who has Alex's key is Alex. Give each person their own, remove people by restarting
  with them taken out, and do not reuse keys. There are no passwords, expiry or single sign-on yet.
- **It identifies, it does not authenticate the network.** Use `--token` and a trusted network when the runtime is shared.
- Per-department limits (a lighting person who may only touch lighting) are not there yet: today's roles are by action.
