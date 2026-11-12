# PCHI: positioning and roadmap

Working document. It records where PCHI is going and, just as important, which claims the code supports today.

## Positioning

> **Shared reality for live experiences.** PCHI is the shared runtime that lets artists, operators, software and agents see the same show and agree on what happens next.

The lead feature is not the protocol. It is **Shared Show State**: one screen (Mission Control) that answers *where are we, who's ready, what's at risk, what's next* for every department. That is PCHI's equivalent of what made ShowKontrol useful: one obvious thing people adopt it for. The protocol, rule engine and signed log are what make that screen trustworthy; they are not the pitch.

Brand traits to keep: confidence-first, builder-friendly, plain words (closer to Ableton/Figma than to enterprise middleware). Avoid "orchestration platform" language.

## Claims discipline

PCHI's strongest asset is that it states its limits (`docs/LIMITATIONS.md`). Marketing must not outrun that.

| We can say | We must not say (yet) |
|---|---|
| Every department sees the same current state and readiness | "Everyone is always in sync" — readiness is only what tools report |
| Confidence is computed from readiness, liveness and rule denials, and shows its working | A bare "97%" — and never a number when nothing has reported |
| Rules are enforced once and every denial is signed and verifiable | "Safe" or "certified" — rules are only as good as their author (`LIMITATIONS.md` §2) |
| Works with tools via bridges (Resolume, TiXL, cables.gl, Strobe) | Works with Vork, Ghost Arcade, grandMA… — not built |

## What exists

- Conductor with Only-Lang rules and a signed, hash-chained governance log.
- **Shared show state** (`show_state` messages → `GET /state`): plan, current/next state, department readiness, liveness, confidence.
- **Mission Control** page (`/mission-control`): commander, technical and wall modes.
- **Readiness-gated transitions**: Only-Lang rules can refuse a state change (`show.entering`, `readiness.<dept>`, `show.blockers`). A refusal is signed, shown in Mission Control, and sent to bridges as `rejected` so a tool that moved on its own can be put back.
- Bridges: Resolume, TiXL, cables.gl, TouchDesigner, Ableton, and **Strobe** (first one that drives *and* follows show state, and handles refusals).
- `demos/strobe-show`: end-to-end, with Strobe simulated.

## Next, in order

1. **Run against a real Strobe** and fix whatever the simulation hid. Highest-value, lowest-effort.
2. **Name the target in denials** so a bridge can pull back the offending control (today denials are visible, not enforced).
3. **Teach the other bridges `rejected`**, so a refusal can pull any tool back, not just Strobe.
4. **Push, not poll**: Mission Control polls `/state` once a second; stream changes over `/ws`.
5. **Ghost Arcade bridge.** It speaks OSC and MIDI; its address scheme needs reading from its source first (AGPL: keep the bridge a separate process). Its Ableton/clip-launch model maps naturally onto states.
6. **Previews** in Mission Control (thumbnail per department). Valuable for trust; needs a frame source, e.g. Spout/NDI from Strobe or Ghost Arcade.

## Lessons borrowed from Vork

[Vork](https://github.com/voyager-so/vork) is a headless 3D engine designed to be driven by agents: a tiny API (~2,500 tokens), JSON scenes, a built-in `help` an agent can read. If PCHI wants agents as first-class participants:

- Keep the message vocabulary small and self-describing. `show_state` has three verbs (`plan`, `enter`, `readiness`) for that reason.
- Give the Conductor a machine-readable description of itself (schema + current state + available transitions) so an agent can act without a manual.
- Agents should act through the same governed path as everyone else, so a denied action is denied for an agent too.

A Vork *adapter* (render a state's scene on enter) is plausible but needs a GPU host and is out of scope until the above exists.
