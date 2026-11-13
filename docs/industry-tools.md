# Industry tools PCHI should talk to

A working list of the tools most live shows already use, per department, with how each is controlled.
**How sure I am** is marked: *checked* means I confirmed it against the vendor's documentation or
several independent sources in this session; *from memory* means I believe it but have not checked it
and it must be verified against the vendor's current docs before anything is built.
Ports and addresses change between versions: always confirm against the version in use.

## Cross-cutting protocols

| Protocol | What it is | Used for |
|---|---|---|
| OSC (UDP/TCP) | Open Sound Control: address + typed arguments | Almost everything below. PCHI already sends generic OSC. |
| MIDI / MIDI Show Control (MSC) | Notes, CCs, SysEx cues over USB/DIN/network | Lighting desks, playback software, controllers |
| Art-Net / sACN (E1.31) | DMX lighting data over Ethernet | Fixtures and nodes directly; some lasers and media servers |
| NDI | Video over IP | Visual distribution (already a rig node type) |
| Timecode (LTC / MTC / Ableton Link) | Shared clock | Syncing to a DJ or playback source |

## Show / cue playback

| Tool | Where it is used | Control | Notes |
|---|---|---|---|
| QLab (Figure 53) | Theatre, corporate, installs (macOS) | OSC, UDP and TCP, port 53000, documented dictionary; *checked* | Often runs audio/video cues. PCHI would trigger it, not replace it. |
| Ableton Live | Live electronic music, DJ-style sets | MIDI, OSC via add-ons, Ableton Link; *from memory* | Usually the source PCHI follows, not a target. |
| Show Cue Systems, Medialon, Stage Research | Theatre / themed entertainment | Own protocols, MSC; *from memory* | Skip until asked for. |

## Visuals / VJ / media servers

| Tool | Where it is used | Control | Notes |
|---|---|---|---|
| Resolume Arena / Avenue | The most common VJ software for clubs and festivals | OSC, default UDP port 7000, set in Preferences > OSC; addresses like `/composition/layers/1/clips/1/connect`; easiest to learn an address with OSC Shortcut mode; *checked (secondary sources)* | Official OSC page not retrievable here; verify addresses against the manual. |
| TouchDesigner | Generative visuals, installations | OSC and WebSocket in/out; *from memory* | Very flexible: no fixed addresses, you define them. |
| disguise, Pixera, Millumin, Notch, Unreal | Large-scale and architectural media | Each has its own API (OSC, REST or JSON-RPC); *from memory* | Needs per-product work. |
| Strobe | The VJ engine PCHI already has an adapter for | OSC; adapter exists in `pchi-strobe-adapter/` | Addresses still not verified against a real Strobe. |

## Lighting

| Tool | Where it is used | Control | Notes |
|---|---|---|---|
| grandMA3 / grandMA2 (MA Lighting) | Festivals, touring, large concerts | OSC 1.1 in and out; messages like `/gma3/Page1/Fader201,i,100`; the whole command line via `/cmd` with a string; MA3 manual has an OSC page per version; *checked* | Setting names differ between software versions. grandMA3 onPC is a software version of the console. |
| ETC Eos family (Eos, Ion, Gio) | Theatre, broadcast, venues | OSC over UDP (console receives on one port, transmits on another; ETC suggests 8000 and 8001) or TCP on 3032; set in Setup > System > Show Control > OSC; *checked* | Very well documented; ETC publishes the full OSC command list. |
| ChamSys MagicQ | Touring, festivals | OSC, MIDI, DMX; *from memory* | Free PC version. |
| Avolites Titan, Hog (High End) | Touring, concerts | Own remote protocols, MIDI; *from memory* | Skip until asked for. |
| QLC+ | Free/open source, small venues | WebSocket API, OSC; *from memory* | Useful as a free test target for lighting. |

## Lasers

| Tool | Where it is used | Control | Notes |
|---|---|---|---|
| Pangolin BEYOND | The common professional laser show software | Acts as an OSC server; addresses like `/beyond/master/brightness`; full list on Pangolin's wiki; one third-party project uses port 12000 by default (not confirmed by the vendor); *checked (partly)* | OSC goes IN to BEYOND; frame data does not come back out. |
| Laserworld Showeditor / ShowNET, others | Laser projectors and their own software | Often DMX/Art-Net triggers or own network protocols; *from memory* | Many lasers can simply be triggered over DMX. |
| ILDA | The hardware/data standard (DB25, ILDA files) | Not a network control protocol | Out of scope for PCHI. |

Lasers are the one department where a wrong message can hurt someone: any laser support needs an
arm/disarm and a hard stop before it is used with real hardware.

## Audio

| Tool | Where it is used | Control | Notes |
|---|---|---|---|
| Behringer / Midas X32 and M32 | The most widespread digital mixer in small and mid-size venues | OSC over UDP, port 10023 (X-Air series: 10024); the mixer replies to the port the request came from; the protocol is documented unofficially (Patrick-Gilles Maillot) and subscriptions must be renewed with `/xremote`; *checked (secondary sources)* | The official spec is thin: verify on a real desk or an emulator. |
| Behringer Wing, Allen & Heath SQ / dLive, Yamaha CL / QL / Rivage, DiGiCo | Mid to large venues, touring | OSC, MIDI or TCP depending on the brand; *from memory* | One adapter per family. |
| Q-SYS | Installed sound, themed venues | JSON-RPC (QRC) over TCP; *from memory* | |
| Reaper | Playback / recording | OSC; *from memory* | |

## Picking a few

I would build these four profiles first. They cover all four departments, each has public OSC
documentation, and each has a free or software version to try without buying hardware (I believe this
for all four but have not confirmed every demo's limits: check before relying on it).

1. **Resolume Arena** for visuals. The most common VJ tool; port and address style are known.
2. **grandMA3** for lighting, with **ETC Eos** as the same kind of OSC profile for theatre venues.
   Both are OSC, so one lighting adapter design covers both.
3. **Behringer X32 / M32** for audio. The biggest install base; one adapter also covers the X-Air.
4. **Pangolin BEYOND** for lasers, together with the arm/disarm and hard-stop safety first.

Not picked, and why: QLab and Ableton are more often the thing PCHI sits beside than a target;
disguise, Pixera and the other large media servers need per-product work for a narrower audience;
Strobe stays as it is until it can be checked against the real thing.

## Status

Built (first version): the five profiles in the picks above, plus laser arm/disarm and a hard stop. See
"Tool profiles" and "Laser safety" in `connecting-systems.md` for what each sends and which addresses are
confirmed. **None has been tried against the real tools yet.**

## What "building a profile" means

Today PCHI sends every action as a generic `/pchi/<kind>` OSC message. A **device profile** translates
a PCHI action into the tool's own message, for example `lighting.look blue` into a grandMA3 command or
an Eos cue fire, using the host and port in the Rig tab. Each profile is a small, tested module that
produces the exact bytes for given actions, and is then confirmed against the real tool or its
offline/software version.
