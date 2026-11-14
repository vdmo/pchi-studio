# A standby runtime: seconds instead of minutes

The runtime is one process on one machine. If it dies mid-show, a person can restart it with `--resume` and the show carries on, but that takes
minutes. A **standby** runs on a second machine, copies the first one's log as it is written, and takes over when the first goes quiet.

```bash
# the primary (the usual runtime, plus the failover key)
pchi-studio-api show.pchi --db primary.db --send --failover-key "<a long secret>" --governance gov-a --roster people.json

# the standby, on another machine: the same show file, its own database and receipts, and the same failover key
pchi-studio-api show.pchi --db standby.db --send --failover-key "<the same secret>" --standby-of 10.0.0.5:3100 \
    --governance gov-b --roster people.json --inputs inputs.json
```

Give the standby the same flags as the primary (`--roster`, `--inputs`, `--audience-token`, `--edit-key`, `--governance`): it starts them when it takes over,
not before. Its own `--db` and `--governance` folders must be its own.

## What the standby does

| | |
|---|---|
| **While it waits** | Copies the primary's log (every input, context change, cue, live edit and decision) once a second. Sends no cue, streams no lighting, opens no OSC, MIDI or LTC input. Every request but `GET /role` gets "standby". Studio's Live tab shows a blue **Standby** note |
| **Taking over** | Builds the run from its copy, the same as restarting with `--resume`: the scene and context the show was in, the live edits made since the show started, the cues still ahead armed. A cue that came due while nobody was running is **skipped and listed**, not fired late. Lasers start **disarmed** and departments are asked to report READY again |
| **Carried over** | The scene, the context, live edits, **open polls with their votes** (and closed polls' results), **edit proposals** (waiting ones stay waiting; ones the show has moved past are marked superseded) and **what a send can still undo**. All of it is rebuilt from the log, the same way for a standby and for `--resume` |
| **Not carried over** | Signed-in sessions (people sign in again), the receipt chain (the standby's receipts are a second chain: export both), and the last half second of votes (they are written to the log in batches every half second) |
| **Who may use it** | `--failover-key` is the only thing that lets a standby read the log and stop the primary. It must differ from `--token`, `--edit-key` and `--audience-token`. Without `--failover-key` the primary refuses `/replicate` and `/fence` |

## When it takes over

- **`--takeover auto`** (the default): after `--takeover-after` seconds (default 6, 2 to 600) of the primary not answering, having copied at least one run.
- **`--takeover manual`**: only on `POST /takeover` with the header `X-Pchi-Failover-Key`. This is also how to hand over **on purpose**: the standby copies the last of the log, takes over and
  stops the (healthy) primary at once.

A standby that was *refused* by the primary (wrong key, a log that is not the one it followed) does **not** take over: that is a mistake to fix, not a death to cover.
`GET /role` on the standby says which.

## Stopping the old primary (fencing)

Once the standby is running it keeps telling the old primary, every few seconds, which takeover it is on. A primary that hears a newer one **stops**:
it sends nothing, streams no lighting, disarms its lasers, ignores inputs and timers, and Studio shows a red banner. So a primary that was only slow, or that a person restarted
with `--resume`, does not fight the standby for the lights. (In the check we ran, a restarted primary was stopped within about a second.)

**The one case it cannot cover:** if the network between the two is cut while both can still reach the tools, the standby cannot hear the primary or tell it anything, and both
will send until the network is mended. Use `--takeover manual` if that is how your network can fail, and decide with your eyes.

## Pointing things at the right runtime

Everything that talked to the primary must talk to the standby after a takeover: Studio, department pages, a button box, the audience gateway.

- **Studio follows by itself.** On the Live tab, **Backup runtime** takes the standby's address (for example `10.0.0.6:3100`). Studio then watches both runtimes' `GET /role` every two seconds and moves to the one that is running the show: it stays where it is while that runtime is `active`, moves when it is not and the other is, and when both claim to be active (a takeover that has not yet stopped the old primary) the later takeover wins. A banner says when it moves. A session belongs to one runtime, so after a move you sign in again (a typed operator key still works on both).
  Studio reaches the backup directly from the browser, so start the backup with `--allow-origin http://<the address Studio is served from>` (several, comma-separated, are fine; `*` is refused). Only listed origins are answered; every request still needs the token and key it always needed.
- **Anything else** (a button box, the audience gateway) does not move by itself. Put both behind one name you can repoint (DNS, a reverse proxy, a load balancer's health check on `GET /role`: `"role": "active"` is the one to send to), or tell people the second address.

## What it does not do

- It does not make the show survive a loss of the **tools** (the lights, the laser), only of the runtime.
- Two standbys following one primary would both take over. Run one.
- The copy is as old as the last poll (one second): an input pressed in that second on a primary that then dies is lost. The run report on the standby shows where the log ends.
- It is checked on loopback only, with real processes (below), not on two machines in a venue.

## What has been checked, and what it showed

`cargo test -p pchi-studio-api --test failover_procs` starts the real binary twice, a fake lighting desk that both send to, and a link between them that can be cut. Measured on one machine with `--takeover-after 2`:

| | |
|---|---|
| Primary killed outright | standby active after about **1.8 s**; the desk heard it after about **2.0 s**, and then heard only the standby |
| Link cut, primary still alive | standby took over after about **1.8 s**; for as long as the link stayed cut the desk heard **two senders** (the case above, shown rather than argued); when the link came back the primary was stopped within about **0.5 s** and the desk heard only the standby |

Times are with a 2-second silence limit; the default is 6 s, so expect about 6 s plus the poll. The test does not use a real network, a real desk or real lasers.

**A bug this found.** Running a pair by hand showed a standby taking over from a primary that was alive: the first sign-in on a runtime checks a hashed key with 200,000 rounds, and doing that on a thread the runtime needs made the primary go silent for about 2 seconds in a debug build (a test now holds it to under 0.7 s). The check now runs where it cannot stall the others. If your `--takeover-after` is shorter than a sign-in can take, a standby can still be fooled by anything that stalls the runtime for that long: keep it at 6 s or more on a loaded machine.
