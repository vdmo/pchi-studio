# Command-line tools

Every binary in the workspace, what it is for, and where it is explained. All accept `--help`.

| Tool | Purpose | When |
|---|---|---|
| [`pchi-studio-api`](runtime-reference.md) | the runtime | always |
| [`pchi`](#pchi) | check, simulate and run a showchart | while building |
| [`pchi-import`](#pchi-import) | cue list or QLab 4 workspace to a show | starting from existing material |
| [`pchi-drill`](#pchi-drill) | the safety drill | before doors |
| [`pchi-gearcheck`](#pchi-gearcheck) | prove each tool, one at a time | on the real rig |
| [`pchi-netcheck`](#pchi-netcheck) | measure the network and rehearse a takeover | before doors, with a standby |
| [`pchi-venueday`](#pchi-venueday) | run all of the above into one folder | the day at the venue |
| [`pchi-twin`](#pchi-twin) | stand-in devices for the rig, that can be made to fail | testing without gear |
| [`pchi-bench`](#pchi-bench) | give an agent jobs and grade what it did | developing and comparing agents |
| [`pchi-witness`](#pchi-witness), [`pchi-follow`](#pchi-follow) | failover helpers | with a standby |
| [`pchi-run-report`](#pchi-run-report) | the hand-over page | after the show |
| [`pchi-memory`](#pchi-memory) | agent memory from the command line | after the show |
| [`pchi-mcp`](#pchi-mcp) | MCP server for agents | when agents are involved |
| `scripts/verify_governance_chain.py` | verify signed receipts without trusting PCHI's code | audit |

## pchi

The showchart tools. Offline: nothing is sent unless `--emit` is given. They read a **bare showchart** (`.pchi.json`), not a full `.pchi` with a rig.

```bash
pchi validate show.pchi.json              # structural problems
pchi inspect  show.pchi.json              # scenes, actions, event alphabet, dead ends
pchi simulate show.pchi.json              # every path through the chart; emits nothing
pchi run show.pchi.json --events START,PORTAL_OPEN,END --set energy=0.3 --db run.db
pchi run show.pchi.json --emit 127.0.0.1:8890       # also send each action as a PCHI event (JSON over UDP)
pchi replay run.db                        # replay the latest run from the log alone and check it reproduces
pchi events run.db                        # every record in a log
```

With no `--events`, `run` follows the first transition out of each state until none is left. `--set KEY=VALUE` presets context (JSON or plain text).

## pchi-import

```bash
pchi-import cues.csv --out show.pchi --name "Spring Gala"
pchi-import workspace.qlab4 --map Audio=audio.snapshot
```

CSV, TSV or QLab 4 plist in; a `.pchi` show out. Columns `time` and `action` are required; `value`, `label`, `number`, `lane`, `length`, `scene` are optional. Cue types with no known action are kept on a `notes` lane and send nothing. QLab 5, `.noisette` and `.score` are refused in words. Tested on hand-written fixtures only. See [import.md](import.md).

## pchi-drill

```bash
pchi-drill my-show.pchi --interactive --sign ./receipts --report drill.md
```

Builds a copy of the show in memory, sends nothing, provokes trouble (disarmed lasers, wrong keys, an unreachable device, a restart, an early GO, timecode jumps) and prints GO or NO GO. Exit code 1 on NO GO. `--profiles` and `--fixtures` take the same folders as the runtime. See [safety-drill.md](safety-drill.md).

## pchi-gearcheck

```bash
pchi-gearcheck my-show.pchi --only qlab,foh --report gear.md --update-doc docs/foh-proof.md
```

**Sends real messages**, one device at a time, only after you say yes. Asks for `LASER SAFE` or `MUTED` where it matters, shows the exact message, asks whether the tool reacted, and (for tools that answer) listens and times a cable pull. See [gear-check.md](gear-check.md).

## pchi-netcheck

```bash
pchi-netcheck --primary 10.0.0.5:3100 --standby 10.0.0.6:3100 --witness 10.0.0.7:3300 \
  --failover-key "$PCHI_FAILOVER_KEY" --report net.md --samples 10 [--no-cut]
```

Reach, delay and loss, clock skew, roles, whether the standby follows, primary lag, a rehearsed takeover, whether the witness refuses a third party; then an interactive cut test that times a takeover and the old primary stopping. Exit non-zero on a failure. See [failover.md](failover.md).

## pchi-venueday

```bash
pchi-venueday my-show.pchi --runtime 10.0.0.5:3100 \
  --primary 10.0.0.5:3100 --standby 10.0.0.6:3100 --witness 10.0.0.7:3300 \
  --recording rehearsal.jsonl --skip failover --out docs/gear-results/2026-11-20
```

Runs the preflight, the drill, the gear check, the network check and the report in order, into one folder with an `index.md` (results, what was **not** tried, tool versions). `--skip` takes `preflight,drill,gear,failover,report`. See [real-gear-day.md](real-gear-day.md).

## pchi-twin

```bash
pchi-twin my-show.pchi --runtime 127.0.0.1:3100 --control 127.0.0.1:3400
```

Stands up one loopback device per UDP device in the rig, writes `my-show.twin.pchi` pointed at them, reports each department READY, and serves a control API (`GET /devices`, `GET /seen/<id>`, `POST /fault`, `POST /ready`, `POST /clear`). A device can be `down` or `lag:<ms>`. See [twin.md](twin.md).

## pchi-bench

```bash
pchi-bench --agent careful                         # a reference agent: checks the benchmark itself
pchi-bench --agent-cmd ./my-agent.sh --report bench.md --json bench.json
pchi-bench --list | --print-show | --only move-cue,stop-lasers | --timeout 120 | --pass-mark 0.8
pchi-bench --agent-cmd bench/agents/claude.sh --repeat 5 --evidence runs/a   # a real agent: repeats, cost, sealed evidence
pchi-bench --verify runs/a/index.json
```

Runs each scenario against a real runtime and a twin, gives the agent its task, and grades the outcome and the conduct from the runtime and a trace of every request. Exit 1 if any scenario fails. See [bench.md](bench.md).

## pchi-witness

```bash
pchi-witness --bind 0.0.0.0:3300 --failover-key "$PCHI_FAILOVER_KEY"
```

A third process that grants a **lease**: only the holder sends. A single witness is a single point of failure for *taking over* (not for the running primary, which keeps its lease while it can reach the witness). See [failover.md](failover.md).

## pchi-follow

```bash
pchi-follow --listen 0.0.0.0:3100 --runtime 10.0.0.5:3100,10.0.0.6:3100 --status 127.0.0.1:3101
```

A TCP forwarder at one fixed address that polls each runtime's `/role` and forwards to the active one, closing carried connections when it moves. Lets desks and department pages keep one address across a takeover. It is one process, and does not cover the audience gateway. See [failover.md](failover.md).

## pchi-run-report

```bash
pchi-run-report run.jsonl --out report.html
pchi-run-report --db show.db --chain governance.json --memory memory.db --out report.html
```

One self-contained HTML page (no scripts, no network). See [report.md](report.md).

## pchi-memory

```bash
pchi-memory --db memory.db derive run.jsonl
pchi-memory --db memory.db recall "why was the haze cue late"
pchi-memory --db memory.db list --status draft
pchi-memory --db memory.db verify <id>
pchi-memory --db memory.db eval golden.json --fail-under-recall5 0.8 --max-wrong 0
```

Subcommands: `derive`, `remember`, `recall`, `list`, `verify`, `supersede`, `settle`, `pin`, `forget` (needs `--yes`), `sweep`, `audit`, `export`, `stats`, `golden-init`, `golden-check`, `eval`. `--agent` acts as an agent (drafts and recall only). See [memory.md](memory.md).

## pchi-mcp

```bash
pchi-mcp --runtime 127.0.0.1:3100 --name nova [--token T] [--key K]
```

MCP over stdin and stdout, so an agent can look at the show, rehearse and propose edits, ask and draft memory, and stop lasers. It cannot press GO, arm, approve, or edit directly. See [mcp.md](mcp.md).

## Not binaries, but tools

- `tools/obs-pchi/pchi_obs.py`: an OBS script (Tools > Scripts) for the **runtime**: shows PCHI's state, reports READY or the missing OBS scenes, optional GO hotkey. See [obs-plugin.md](obs-plugin.md).
- `tools/resolume-pchi-bridge`, `touchdesigner-pchi-chop`, `ableton-pchi-max`, `strobe-pchi-bridge`, `tixl-pchi-bridge`, `cables-pchi-relay`, `pchi-show`: bridges for the **PCHI protocol** (the PrimeSwarm conductor), documented in their own folders and in the root `README.md`.
- `demos/`: `multi-tool-live-show` and `strobe-show`, scripted end-to-end demos of that protocol layer.
