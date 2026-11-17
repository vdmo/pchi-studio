# PCHI Studio documentation

PCHI Studio is a show-control system for live events: you describe a show as **scenes** with **timed cues**, connect the **rig** (lighting desks, lasers, audio consoles, visuals tools), and a Rust **runtime** runs it, sends the cues, records everything, and keeps people and agents inside the roles you give them.

This is the map. Start with the first section that fits you.

## Read in this order

| If you are... | Read |
|---|---|
| New, want to know what this is | [overview.md](overview.md) |
| Asking how it differs from QLab, Medialon and the rest | [positioning.md](positioning.md) |
| Installing and running it for the first time | [getting-started.md](getting-started.md) |
| Building a show in the Studio editor | [studio-guide.md](studio-guide.md), [command-bar.md](command-bar.md) |
| Running a show on the night | [show-day-handbook.md](show-day-handbook.md) |
| Looking up a flag, route, tool or file format | [runtime-reference.md](runtime-reference.md), [show-file.md](show-file.md), [tools-reference.md](tools-reference.md) |
| Developing or measuring agents | [mcp.md](mcp.md), [bench.md](bench.md), [twin.md](twin.md) |
| Locking it down for a shared network | [security.md](security.md) |
| Stuck | [troubleshooting.md](troubleshooting.md), [glossary.md](glossary.md) |
| Deciding what to trust | [LIMITATIONS.md](LIMITATIONS.md), [foh-proof.md](foh-proof.md) |

## Every document, by subject

**Building a show**
- [library.md](library.md): save and plan shows on the server, share them with a team
- [side-panel.md](side-panel.md): the Checks, Code and AI panel in Studio (problems found as you edit, the show as text, an AI that can only propose)
- [command-bar.md](command-bar.md): build and edit by typing or speaking
- [scene-minimum.md](scene-minimum.md): a scene that must run before it can be moved on
- [fixtures.md](fixtures.md): DMX fixture files, so a cue can say `wash1=blue`
- [timecode-and-midi.md](timecode-and-midi.md): lock a timeline to LTC or MIDI timecode
- [import.md](import.md): bring a cue list in from CSV or QLab 4

**Connecting the rig**
- [connecting-systems.md](connecting-systems.md): how departments and devices take part
- [remote-setup.md](remote-setup.md): one venue computer, reached from a distance over Tailscale, with a roster and a startup script
- [profiles.md](profiles.md): how an action becomes the OSC a tool understands (Resolume, grandMA3, Eos, X32, BEYOND, QLab, MagicQ)
- [artnet-sacn.md](artnet-sacn.md): DMX over the network
- [osc-input.md](osc-input.md): Stream Deck, tablets and button boxes that press GO
- [capture.md](capture.md): capture what a tool sends
- [industry-tools.md](industry-tools.md): where PCHI sits next to other tools
- [ur.md](ur.md): Universal Robots: run named programs, read state, with refusals and faults handled honestly (only run against a pretend UR so far)
- [kuka.md](kuka.md): KUKA: what is built (XML framing, an example profile, the contract a receiver must meet), what is not, and what I need to go further
- [ursim-check.md](ursim-check.md): the procedure for checking the UR driver against URSim and then a real robot, starting read-only
- [link-profiles.md](link-profiles.md): describe a device driven by lines of text over TCP in a file, with a list of commands it must never send
- [motion.md](motion.md): the motion department: robots and moving rigs, armed like lasers, with their own stop (step 1 of the robotics plan; no real robot driver yet)
- [robotics-kuka-ur.md](robotics-kuka-ur.md): the design for KUKA and Universal Robots (not built)
- [robotics-plan.md](robotics-plan.md): a plan (not built) for controlling robots and motion rigs: what exists, the safety rule, and the order to build in

**Before the doors**
- [rehearse.md](rehearse.md): try an edit against what already happened
- [safety-drill.md](safety-drill.md): `pchi-drill`, the show-day safety drill
- [gear-check.md](gear-check.md): prove each tool, one at a time
- [real-gear-day.md](real-gear-day.md): a whole day at the venue, with `pchi-venueday`
- [foh-proof.md](foh-proof.md): what has and has not been proven on real gear

**While it runs**
- [roles.md](roles.md): who may do what, per person
- [failover.md](failover.md): a standby, a witness, and following a takeover
- [virtual-events.md](virtual-events.md): audience votes
- [narration.md](narration.md): what a proposed edit says in words
- [bench.md](bench.md): the agent benchmark
- [twin.md](twin.md): the rig twin, stand-in devices that can fail
- [obs-plugin.md](obs-plugin.md): an OBS script that shows PCHI's state in OBS and reports whether the scenes the show needs exist
- [../tools/tixl/README.md](../tools/tixl/README.md): TiXL as a rig device (OSC profile, demo show, a stand-in listener); not yet run against TiXL

**After the show**
- [replay.md](replay.md): recordings and replay
- [report.md](report.md): the post-show report
- [governance.md](governance.md): signed, hash-chained receipts, and how to verify them

**Agents**
- [mcp.md](mcp.md): `pchi-mcp`, an MCP server agents can look and propose through
- [memory.md](memory.md): agent memory that fades and says "I don't know"

**Securing it**
- [roles.md](roles.md), [tls.md](tls.md), [governance.md](governance.md), [security.md](security.md)

**Project**
- [STRATEGY.md](STRATEGY.md), [LIMITATIONS.md](LIMITATIONS.md), `../PRICING.md`

## About the protocol and the website

The repository also holds the original PCHI protocol (`pchi-schema/`, `primeswarm-pchi/`, the `tools/` bridges) and a presentation site, [index.html](index.html) and [docs.html](docs.html). This manual is about **PCHI Studio**: the editor, the runtime and their tools.

## A note on claims

Each document says what it does *and what it does not*. Where something has only been tested with simulated gear, hand-written fixtures or loopback networks, the document says so. Nothing here should be read as "proven on your rig" until you have run [gear-check.md](gear-check.md) and [real-gear-day.md](real-gear-day.md) on it.
