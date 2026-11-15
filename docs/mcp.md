# PCHI for AI agents (MCP)

`pchi-mcp` is an MCP server (the Model Context Protocol, over standard input and output). An AI agent that speaks MCP can **look at a running
show, rehearse an edit against what already happened, and propose it** for a person to approve. It cannot run the show.

```bash
pchi-mcp --runtime 127.0.0.1:3100 --name nova
```

Add it to an MCP client as a command, for example in a client's MCP settings: command `pchi-mcp`, arguments `--runtime 127.0.0.1:3100 --name nova`.
It talks to a runtime that is already running (`pchi-studio-api show.pchi ...`). Give it `--token` if the runtime has one, and `--key` (or
`PCHI_AGENT_KEY`) if the runtime has a roster with a named agent for it (`roles.md`).

## What the agent can do

| Tool | Does |
|---|---|
| `pchi_describe` | **start here.** The runtime describes itself to you: who it takes you to be, every action with whether it is allowed *for you* and what it becomes (an agent's edit is a proposal), where the show is, which ways out of the current scene would fire right now, the rig, what needs attention, and the rules to follow. See below |
| `pchi_state` | the scene, how long it has run, the context (including audience votes), timecode, edits waiting for a person |
| `pchi_show` | the running show's definition as JSON: the base to edit a copy of |
| `pchi_doors_check` | the whole show against the rig and each tool: what will not work, what to look at |
| `pchi_rehearse` | an edited show played against the run so far: the path, cues that would no longer fire, scenes longer or shorter, what each cue would send. Changes nothing |
| `pchi_propose` | an edited show, **proposed**: it waits for a person, with the agent's reason and the rehearsal |
| `pchi_propose_cue_change` | the common small edit without writing the whole show: move a cue and/or change its value. `dry_run` only reports; otherwise it proposes |
| `pchi_proposals` | every proposal and where it stands |
| `pchi_report` | cues and timing, failed deliveries, edits, and who did what |
| `pchi_stop_lasers` | the hard stop: disarm every laser and send each tool's safe message. Allowed because it only makes things safer |

An agent has to say **why** (`said`) with every proposal. The reason is kept with the proposal and in the signed record, and the person approving sees
it next to what would change and how it would have played (`narration.md`).

## What the agent cannot do

It cannot press GO, arm a laser, try or test-fire a device, approve or reject a proposal, undo a send, open or close a poll, or change the show directly.
Those tools are not offered. **That is a convenience, not the guarantee**: the runtime enforces it. Every request is sent as `agent:<name>`, and the
runtime refuses an agent those things whatever it asks and whatever key it holds, and writes the refusal down (`governance.md`).

So the worst an agent can do through this server is propose edits a person will read, and stop the lasers.

## How a session goes

1. The agent calls `pchi_state` and `pchi_show` to see where the show is and what it contains.
2. It tries an idea with `pchi_propose_cue_change` and `dry_run: true`: *"move the lighting cue in INTRO to 40 seconds"* answers *"l1 would NOT fire: INTRO is left
   at 3.0 s before it is due at 40.0 s."*
3. If the idea survives the rehearsal, it proposes it with a reason. The Live tab shows it under **Waiting for you**.
4. A person approves or rejects. If the show changed in the meantime, the proposal is superseded and the agent proposes again.

## What this does not do

- It does not make an agent good at running a show. It makes an agent's mistakes **cheap**: a bad proposal is read and rejected.
- Only the tools above exist. There is no streaming of the run, no subscription to changes (the agent asks again), and no resources or prompts.
- It speaks plain HTTP to the runtime, so run it on the same machine or a trusted network.
- Tested with a hand-written client of the protocol (initialise, list, call, errors, notifications) and a real runtime. It has not been run inside a particular
  agent product: if yours negotiates a different protocol version, or wants something this server does not answer, that will show up on first use.

## Describing itself: `pchi_describe` and `GET /describe`

An agent should not need a manual. `GET /describe` (the tool `pchi_describe`) returns one JSON answer, `pchi-describe/1`, written for the caller:

| Field | Says |
|---|---|
| `you` | who the runtime takes you to be (name, role, whether you are an agent, your departments) |
| `you_may` | every action with its route and an **outcome**: `allowed`, `allowed_as_proposal` (an agent's edit or memory note), or `refused`. It asks the same permission check the routes ask, for you, so it cannot drift from what is enforced |
| `runtime` | whether this runtime is the active one, whether it is sending or in a dry run |
| `show`, `now` | the scenes, the events the show answers to, the current scene, how long it has run, the context, and **each way out of this scene with whether it would fire right now** (a guard that does not hold says so; a scene still held by its minimum says for how long) |
| `rig`, `readiness` | the devices (armed, link) and each department's last word |
| `waiting_for_a_person` | proposals not yet decided |
| `attention` | what to look at first: a department on hold, a lost link, an armed laser, a dry run, a runtime that is not the active one |
| `rules` | the house rules as sentences: rehearse before you propose, say why, a refusal is final, you cannot press GO or arm, STOP LASERS is yours, do not guess when memory does not know |
| `how_to` | the edit loop and where the docs are |

It tells you your own powers and no one else's, and it changes nothing: the routes still enforce. The agent benchmark ([bench.md](bench.md)) is how you measure whether an agent *follows* it.
