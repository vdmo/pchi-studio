# Show-day handbook

A checklist-shaped path through a whole production: build, prove, run, recover, review. Each step points to the document that explains it. Steps you skip are claims you have not tested.

## Phase 1: build (days before)

1. **Draft the show.** Studio's command bar (`chain preshow, intro, drop, finale`), a template, or `pchi-import cues.csv` / a QLab 4 workspace. [command-bar.md](command-bar.md), [import.md](import.md)
2. **Describe the rig.** One device per system, with host and port, on the Rig tab. Pick each lane's device. [studio-guide.md](studio-guide.md)
3. **Place the cues.** Timeline tab, with cue numbers for the stage manager's cue sheet.
4. **Add the safeguards.** A minimum on scenes that must not be skipped ([scene-minimum.md](scene-minimum.md)); timecode locks where a playback drives the show ([timecode-and-midi.md](timecode-and-midi.md)).
5. **Validate offline.** The Doors check on the Live tab (it reads the full `.pchi`, rig included). The `pchi validate` and `pchi simulate` commands read a *bare showchart* (`.pchi.json`), not a full `.pchi`: use them on the `showchart` part if you extract it.
6. **Save** `show.pchi` and keep it under version control. Do **not** commit the roster, sessions file or governance folder.

## Phase 2: prepare the machines (a week to a day before)

1. **People.** Make a roster entry per person with `--new-roster-entry NAME:ROLE`, give each their key once, and put the hashed entries in `people.json`. Roles: operator calls the show; editor changes it; lso arms lasers; guest looks; agent proposes; button is a machine input. [roles.md](roles.md)
2. **Network.** Bind to the show network, set `--token`, and put HTTPS in front if the network is shared. [security.md](security.md), [tls.md](tls.md)
3. **Receipts.** Start with `--governance <folder>`. Write the printed **verifying key** down somewhere the show machine cannot change. [governance.md](governance.md)
4. **Inputs.** Stream Deck or button box: `--inputs inputs.json` with an allow list of sender addresses. [osc-input.md](osc-input.md)
5. **A standby and a witness** if a runtime dying mid-show is unacceptable: three machines, one failover key. [failover.md](failover.md)
6. **Recording.** Always `--db show.db --record run.jsonl`. The file survives the loss of the database.

## Phase 3: prove it at the venue (the day, before doors)

Run `pchi-venueday` to do these in order into one folder, or run each yourself.

| Step | Tool | Proves |
|---|---|---|
| Doors check | Live tab, or `GET /preflight` | the show fits the rig and each tool's profile |
| Safety drill | `pchi-drill show.pchi --interactive --sign ./receipts` | lasers stay held, roles hold, a device going away does not stop the show, `--resume` works |
| Gear check | `pchi-gearcheck show.pchi` | each tool really reacts to what PCHI sends (your eyes, one device at a time, with `LASER SAFE` and `MUTED` guards) |
| Network check | `pchi-netcheck --primary ... --standby ...` | delay, loss, clock skew, a rehearsed takeover |
| Rehearsal run | a full run with `--record` | the timeline, and the material for the report |
| Rehearse an edit | Live tab, **Rehearse** | an edit still plays out the same against what already happened |

[safety-drill.md](safety-drill.md), [gear-check.md](gear-check.md), [real-gear-day.md](real-gear-day.md), [rehearse.md](rehearse.md)

Do not tick what you did not see. [foh-proof.md](foh-proof.md) is the honest list.

## Phase 4: run

Start the runtime for real:

```bash
pchi-studio-api show.pchi --db show.db --record run.jsonl --send \
  --bind 0.0.0.0:3100 --token "$TOKEN" --roster people.json --sessions sessions.json \
  --governance ./receipts --inputs inputs.json --memory memory.db
```

1. **Open Studio, sign in, check the role banner.** It must say who you are.
2. **Lasers start disarmed.** Only an operator or the lso arms them, per device, after the beam path is clear.
3. **Departments report READY.** The ready board shows ready, on hold or no link. GO asks twice if a department the *next* scene needs is on hold.
4. **Call the show.** GO on Space (or the button box). A press before a scene's minimum is held, with a countdown.
5. **STOP LASERS** is always available to everyone, signed in or not.
6. **Edits mid-show.** Rehearse first. A person with the right role sends it (the diff is shown first); an agent's edit becomes a proposal under **Waiting for you**. **Undo last send** puts it back.

### If something goes wrong

| Symptom | Do | Read |
|---|---|---|
| The runtime process died | restart with `--resume`; or let the standby take over | [failover.md](failover.md) |
| The primary is unreachable but alive | with a witness it pauses by itself and the standby takes over; without one, decide by hand with `POST /takeover` | [failover.md](failover.md) |
| A device does not react | the Live tab shows its last delivery (sent, dry-run, failed). `sent` means the packet left; check the device's own monitor | [connecting-systems.md](connecting-systems.md) |
| A press did nothing | it may be *held* (scene minimum) or have nothing to do in this scene; check the event console | [scene-minimum.md](scene-minimum.md) |
| Timecode stopped | locked cues wait; the Live tab and Doors check say so. A jump forward fires nothing | [timecode-and-midi.md](timecode-and-midi.md) |
| An agent proposes something odd | reject it. It cannot press GO or arm anything | [mcp.md](mcp.md) |

After a restart, cues that came due while down are skipped and listed (never fired late), lasers are disarmed, and departments must report READY again.

## Phase 5: after the show

```bash
pchi-studio-api --db show.db --export-run run.json                 # a recording to keep
pchi-run-report --db show.db --chain governance.json --memory memory.db --out report.html
pchi-memory --db memory.db derive run.jsonl                        # drafts from what happened
python3 scripts/verify_governance_chain.py governance.json          # verify receipts without PCHI's code
```

1. **Hand over** `report.html` (or Studio's **Hand-over page**). It says whether anything needs a look: cues that did not reach a device, a cue more than a quarter second off, a pause or takeover, a recording that does not replay, a receipt that does not verify. [report.md](report.md)
2. **Replay** the run in Studio to see what happened. [replay.md](replay.md)
3. **Review memory:** verify what is worth keeping, archive the rest, settle disagreements. [memory.md](memory.md)
4. **Keep** the recording, the report and the receipts together with the invoice, and `docs/gear-results/<date>/` with the show file.

## A one-page checklist

- [ ] Show validated; Doors check clean
- [ ] Roster made; keys handed out; hashed entries only
- [ ] `--governance` on; verifying key written down elsewhere
- [ ] `--token` set; HTTPS in front if the network is shared
- [ ] Drill run, GO, receipt kept
- [ ] Gear check run on each real tool; what was *not* tried is written down
- [ ] Standby, witness and `pchi-netcheck` if failover matters; gear check also run from the standby machine
- [ ] `--record` and `--db` on
- [ ] Lasers: lso present, beam path clear, armed per device
- [ ] Rehearsal run done and reported
- [ ] After: report, verify receipts, memory review
