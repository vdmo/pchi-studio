# The post-show report

One page, built from a recording, for the person who was not there: the client, the venue, the next shift, you in a month.

```bash
pchi-run-report run.jsonl --out run-report.html                       # a --record file or a one-file recording
pchi-run-report --db show.db --chain governance.json --memory memory.db --out report.html  # from the database, receipts checked, memory read
```

Or, from the running runtime, `GET /run-report` (the same page, with the receipts checked), or Studio's **Hand-over page** button on the Live tab. The page is one HTML file with no scripts and no network use: mail it, print it, keep it with the invoice.

## What is on it

- **A verdict**, in a box at the top: either "Nothing to report" or the list of what needs a look (cues that did not reach a device, a cue more than a quarter of a second off, a pause, a takeover or a restart, a recording that does not replay, a signed record that does not verify).
- **Where the show went**: a bar of the scenes by how long each ran, and a table of each scene, when it began, how long it ran, and what left it. Inputs that did nothing, and presses held because a scene had not run its minimum, are counted.
- **Memory from this run**: how many drafts the run left waiting for a person to verify, how many are verified, how many more could be derived, and the drafts themselves (with any that are in conflict). Only if the runtime has `--memory` or the command is given `--memory FILE`; it is only read.
- **Cue timing**: how many cues fired, the median and 95th-percentile lateness, the worst, and the eight furthest from plan. Lateness is how long after its planned time the runtime fired the cue, not how long the tool took.
- **Did the cues reach the devices?** Counts by outcome, and every failed or unrouted one with the reason. A run that was not started with `--send` says its cues were *dry-run*.
- **Interruptions**: restarts, a takeover, a pause because the witness's lease was lost (see `docs/failover.md`), each with the time.
- **Live edits** and **who decided what**: each edit, and each decision with who made it and whether it was allowed.
- **The signed record**: whether the receipts link and verify. It trusts the key in the export; to be sure of *who* signed, compare it with the key you wrote down when the show was set up. `scripts/verify_governance_chain.py` does the same check without trusting this program.
- **Does the recording hold up?** The run is played back through the show and has to go where the recording says.

## What it does not show

- Whether anything looked or sounded right. "Sent" means the message left the runtime; whether the tool did the right thing is what the gear check and the people in the room are for.
- Anything the runtime was not told: cues run by hand on a desk, inputs that never reached it.
- It holds the names and reasons people and agents typed, and the rig's addresses. It holds no keys, tokens or passwords. Treat it like the show file.
