# Roles: who may do what, per person

One shared operator key means everyone who knows it can do everything, and the record cannot say who did what. A **roster**
gives each person their own key and a role. The runtime then knows who is pressing GO, arming a laser or sending an edit,
refuses what their role does not allow, and writes their name in the run report and the signed receipts.

## Set it up

Make a roster file, a JSON list. The safe way is to let PCHI make each entry:

```bash
pchi-studio-api --new-roster-entry Alex:operator
```

It prints a fresh random key **once** (give it to Alex) and a roster entry that holds only a **salted PBKDF2 hash** of it: the roster file cannot be used to read the key back. Paste the entry into the file. An entry may also have an end date. Or write a key in clear, as below (it works, and the runtime warns about it):

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
A roster with keys in clear holds the keys themselves: protect it like the show file, and move to hashed entries.

## Safer secrets

- **Hashed keys.** (A hashed key is checked with 200000 rounds of PBKDF2, which takes a second or two in a debug build: the first sign-in after a restart feels slow there, and is quick in a release build.) `--new-roster-entry NAME:ROLE` makes an entry with `salt`, `key_hash` and `iterations` (PBKDF2-HMAC-SHA256, 200000 by default) instead of `key`. A key proved once is remembered in memory (never on disk) so it is not re-hashed on every request.
- **End dates.** `"expires": "2026-12-31"` (the end of that day) or `"2026-12-31T18:00"`, UTC. After it the key is refused and says it expired; the startup report lists whose keys have an end.
- **Sessions.** Studio's **Sign in** sends the key once to `POST /signin` and gets a token that lasts `--session-hours` (default 8). This tab keeps the token, not the key; **Sign out** ends it at once on the runtime. Ten wrong keys in a minute make sign-in wait. Machines (a button box, an agent) can still send their key with each request.
- **The receipt-signing key** is the one thing that makes the record trustworthy. `--governance-key <file>` keeps it outside the show folder; on Linux and macOS it is made owner-only. Keep a copy of the **verifying key** (printed at startup) somewhere the runtime's machine cannot change, so a forged log can be told from the real one.

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

- **A key is still a shared secret.** Anyone who has Alex's key, or his session token while it lives, is Alex. Give each person their own, use end dates for visitors, remove people by restarting with them taken out. There is no single sign-on and no second factor.
- **Sessions are kept in memory:** restarting the runtime signs everyone out.
- **Not covered:** the key travels to the runtime in each sign-in and (for machines) in each request, in clear on a plain HTTP network: use a trusted network or a tunnel, as for the rest of the runtime.
- **It identifies, it does not authenticate the network.** Use `--token` and a trusted network when the runtime is shared.
- Per-department limits (a lighting person who may only touch lighting) are not there yet: today's roles are by action.
