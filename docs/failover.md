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

## Slow, stuck and gone are different

A primary that is merely busy must not be replaced: two runtimes sending at once is worse than a pause. So the standby reads *how* the primary is quiet:

| What the standby sees | It means | The standby |
|---|---|---|
| nothing is listening, or the connection is dropped | the process is gone | takes over after `--takeover-after` |
| the network does not carry the request | unknown | takes over after `--takeover-after` |
| it accepts the connection and does not answer | alive and stuck | waits `--stall-grace` longer (default: as long again) |
| it answers, but its own event loop runs behind | slow | does **not** take over |

The primary measures its own event loop ten times a second (how late a timer fires) and reports it with every answer. `GET /role` on either runtime shows `lag_ms` (the last three seconds) and `lag_worst_ms` (the last minute); the standby's `/role` shows `primary_health` (`ok`, `slow`, `stalled`, `gone`, `unreachable`) and Studio's Standby note says it in words. The limit is not zero: a primary stuck for good is still replaced, after the extra wait.

## A witness: so a cut network cannot give you two senders

Without a witness the standby has to guess whether a silent primary is dead or only out of reach, and when it guesses wrong both runtimes send until the link is mended (shown below). A **witness** removes the guess. It is a third process, `pchi-witness`, on a machine of its own, that grants one thing: the right to send, as a lease.

```bash
pchi-witness --bind 0.0.0.0:3300 --failover-key "$KEY"                                   # the third machine
pchi-studio-api show.pchi --db a.db --failover-key "$KEY" --witness witness:3300 --advertise primary:3100
pchi-studio-api show.pchi --db b.db --failover-key "$KEY" --witness witness:3300 --advertise standby:3100 --standby-of primary:3100
```

- The runtime running the show asks the witness for the lease every quarter of its length (`--witness-ttl`, default 6 s, 2 to 120). While the witness says yes it sends.
- If the witness says **no** (someone else holds the lease), or cannot be reached for the whole lease, the runtime **pauses**: it sends nothing, streams no lighting, disarms its lasers, ignores inputs, and says so (`GET /role` shows `"role": "paused"` and why; Studio shows a banner). It goes on by itself when it holds the lease again: cues that came due meanwhile are skipped, not fired late, and lasers stay disarmed.
- A standby may take over only after the witness **grants it the lease**, and the witness grants it only once the primary's lease has run out. So a standby that merely cannot reach a living primary is told no (its `/role` says so in `witness.note`), and a primary cut off from the witness stops itself *before* the standby is allowed to start. The primary counts its lease from when it *asked*, and treats it as 90 % as long as the witness does, so the two do not overlap by the length of one answer.
- After a takeover the standby holds the lease and keeps asking. The old primary, if it comes back, is refused the lease (it starts paused), and is fenced as before.

**Measured** (`cargo test -p pchi-studio-api --test witness_procs`, real processes on one machine, lease 3 s, `--takeover-after 2`):

| | |
|---|---|
| Standby cut off from a **living** primary that still reaches the witness | the standby did **not** take over in 9 s of trying; the desk heard only the primary |
| Primary cut off from both the standby and the witness | primary **paused** after about **2.6 s**; standby active after about **3.1 s**; the desk heard the last primary datagram about **0.6 s before** the first standby one, never both |
| Witness unreachable for a moment | the primary paused, sent nothing, and went on by itself when the witness came back; the standby did not take over |
| Primary killed | standby active after about **3.1 s**, holding the lease; the witness reported the show on the standby |

**What you give up, chosen on purpose.** With the witness down, nothing can take over, and the runtime that holds the lease pauses when it runs out: the show stops rather than doubles. If the primary is dead and you know the witness is down, `POST /takeover?witness=ignore` (with the failover key) takes over without a lease; the takeover receipt says it was on a person's word, and that runtime then runs unleased. Put the witness on a machine that fails separately from both runtimes, and on a network path both can reach. A witness restarted loses who held the lease: the first to ask gets it, and a standby only asks at the moment it would take over, so the primary asks first.

The witness is plain HTTP and the failover key crosses the network with every claim: keep it on a network you control, or put TLS in front of it (`docs/tls.md`).

## Stopping the old primary (fencing)

Once the standby is running it keeps telling the old primary, every few seconds, which takeover it is on. A primary that hears a newer one **stops**:
it sends nothing, streams no lighting, disarms its lasers, ignores inputs and timers, and Studio shows a red banner. So a primary that was only slow, or that a person restarted
with `--resume`, does not fight the standby for the lights. (In the check we ran, a restarted primary was stopped within about a second.)

**The one case it cannot cover:** if the network between the two is cut while both can still reach the tools, the standby cannot hear the primary or tell it anything, and both
will send until the network is mended. Use `--takeover manual` if that is how your network can fail, and decide with your eyes.

## One signed record across a takeover

The standby copies the primary's signed receipts as well as its log. If **both runtimes sign with the same key** (copy the primary's `--governance-key` file to the standby by a channel you trust), the standby starts its own governance log from the primary's receipts, and everything after the takeover continues the same hash chain. One verifying key then checks the whole show:

```bash
curl -s http://standby:3100/governance/export > chain.json
python scripts/verify_governance_chain.py chain.json     # every receipt: chain, hash and signature OK
```

(Checked: three receipts from the primary and two from the standby verified as one chain.)

A copy of the signing key on a second machine is a trade: whoever can read it on either machine can forge the record. If you do not want that, give the standby its own key. Its chain is then separate, and the takeover receipt says so and names the primary's last receipt and verifying key, so a reader can join the two by hand: *"…not adopted: this runtime signs with a different key … It follows the primary's receipt <hash> (primary verifying key <key>)."* The same note appears if the standby's governance log already holds receipts. The standby keeps the primary's receipts in `<governance log>.primary` meanwhile, so a restarted standby does not lose them.

## Rehearse a takeover

`GET /takeover-check` on the standby (with its `--token` if it has one) rebuilds the show a takeover would run from the standby's copy, **changing nothing**: sending is off, no input is opened, nothing is fenced, and the rehearsal show is stopped before it answers. It then compares what it rebuilt with the primary's own account of the show and answers:

- `ready` and a one-line verdict, or what would not work (the scene, context, definition, undo list, waiting proposals or poll differ; the primary refused the standby; nothing copied yet).
- what a takeover now would carry (scene, context, polls, proposals, undo depth), cues it would skip because they are already due, and how the signed record would continue.
- what is not carried in any case.

In Studio, with a backup address set, **Backup runtime → Rehearse a takeover** shows the same. It takes a few tens of milliseconds, so it can be done before every show and again after any change. Votes in the last half second are written to the log in batches, so a poll's count can differ by those; the check says so as a note, not a failure.

## Pointing things at the right runtime

Everything that talked to the primary must talk to the standby after a takeover: Studio, department pages, a button box, the audience gateway.

- **Studio follows by itself.** On the Live tab, **Backup runtime** takes the standby's address (for example `10.0.0.6:3100`). Studio then watches both runtimes' `GET /role` every two seconds and moves to the one that is running the show: it stays where it is while that runtime is `active`, moves when it is not and the other is, and when both claim to be active (a takeover that has not yet stopped the old primary) the later takeover wins. A banner says when it moves. A session belongs to one runtime, so after a move you sign in again (a typed operator key still works on both).
  Studio reaches the backup directly from the browser, so start the backup with `--allow-origin http://<the address Studio is served from>` (several, comma-separated, are fine; `*` is refused). Only listed origins are answered; every request still needs the token and key it always needed.
- **A fixed address: `pchi-follow`.** A button box or gateway has one address in its settings. `pchi-follow --listen 0.0.0.0:3100 --runtime primary:3100,standby:3100` listens on that address, asks each runtime's `GET /role` every half second, and passes connections (HTTP and WebSocket) to the one that says `active`. When the show moves it closes the connections it carried so they reconnect to the new one. Two active at once (a cut link, before the fence): the later takeover. None active: connections are refused until one is. `--status 127.0.0.1:3199` serves what it sees. Run it where the button boxes can reach it (if it is on a runtime's machine, give it a different port). It is one process: a second copy elsewhere is the way to not depend on it. Measured on loopback: the fixed address led to the standby about **3.4 s** after the primary was killed.
- **Ask where the show is.** Every runtime answers `GET /primary`: `{"primary": "host:port", "self": true|false, "role": …}`. The running one says `self: true` and its `--advertise` address; a standby names the primary it follows; a runtime that was fenced names the one that fenced it. With a witness, `GET /primary` on the witness answers from the lease (null while nobody holds it).
- **Anything else** that cannot ask: put both behind one name you can repoint (DNS, a reverse proxy, a load balancer's health check on `GET /role`: `"role": "active"` is the one to send to), or tell people the second address.

## A failover day: prove it on the real network

Everything above was shown on one computer. A network adds delay, loss, clocks that disagree, firewalls, and a switch that is slow after a cable moves. `pchi-netcheck` runs the checks over the real network between the real machines and writes down what it measured:

```bash
pchi-netcheck --primary primary:3100 --standby standby:3100 --witness witness:3300 --failover-key "$KEY"
```

First the checks that change nothing: answer time and loss to each runtime (best, median, worst), how far each machine's clock is from the one running the check, that the roles are as expected, that the standby is following and hears the primary, that the primary's event loop keeps up, a rehearsed takeover, and that the witness answers and **refuses a second sender** while the primary holds the lease. Then, only if those pass and you type `CUT`, the cut test: you cut the primary off (pull the cable, switch off its port, stop the program; cutting only its link to the standby is the interesting one) and it times the takeover; you restore it and type `DONE`, and it times how long the old primary takes to stop, and notes whether both said active in between. It writes `failover-report-<date>.md` and exits non-zero if anything failed. It sends nothing to any tool; whether the standby can reach the *tools* is `pchi-gearcheck`, run on the standby's machine. Do the cut test only when nothing relies on the primary.

The same program ran against real processes on loopback (cut test included, in `witness_procs`): the takeover was measured 3.3 s after `CUT` with a 3 s lease, and the restored old primary was held paused by the witness. Loopback shows the logic; the delay, loss and clock figures only mean something on the real network.

## What it does not do

- It does not make the show survive a loss of the **tools** (the lights, the laser), only of the runtime.
- Two standbys following one primary would both take over. Run one.
- The copy is as old as the last poll (one second): an input pressed in that second on a primary that then dies is lost. The run report on the standby shows where the log ends.
- It is checked on loopback only, with real processes (below), not on two machines in a venue. `pchi-netcheck` (above) is how you check it on yours.

## What has been checked, and what it showed

`cargo test -p pchi-studio-api --test failover_procs` starts the real binary twice, a fake lighting desk that both send to, and a link between them that can be cut. Measured on one machine with `--takeover-after 2`:

| | |
|---|---|
| Primary killed outright | standby active after about **1.8 s**; the desk heard it after about **2.0 s**, and then heard only the standby |
| Link cut, primary still alive | standby took over after about **1.8 s**; for as long as the link stayed cut the desk heard **two senders** (the case above, shown rather than argued); when the link came back the primary was stopped within about **0.5 s** and the desk heard only the standby |

Times are with a 2-second silence limit; the default is 6 s, so expect about 6 s plus the poll. The test does not use a real network, a real desk or real lasers.

**A bug this found.** Running a pair by hand showed a standby taking over from a primary that was alive: the first sign-in on a runtime checks a hashed key with 200,000 rounds, and doing that on a thread the runtime needs made the primary go silent for about 2 seconds in a debug build (a test now holds it to under 0.7 s). The check now runs where it cannot stall the others. If your `--takeover-after` is shorter than a sign-in can take, a standby can still be fooled by anything that stalls the runtime for that long: keep it at 6 s or more on a loaded machine.
