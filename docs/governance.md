# Governance: who may do what, and a record you can verify

PCHI lets agents (a narrating assistant, PrimeSwarm, your own script) work on a live show. This page says what an
agent may do, what only a person may do, and how every decision is written down so it can be checked afterwards.

## The two kinds of actor

- **Person**: someone holding the operator key (`--edit-key`). With no key set, anyone who is not declared an agent.
- **Agent**: any client that declares itself with the header `X-Pchi-Actor: agent:<name>` (on a WebSocket, where
  browsers cannot set headers, use `?actor=agent:<name>`).

A person may also name themselves, `X-Pchi-Actor: person:Alex`, so the record says who. That is a label, not a login.

## What an agent may and may not do

| Action | Agent | Person |
|---|---|---|
| Read state, report, doors check | yes | yes |
| Preview an edit (`POST /show?dry=1`) | yes | yes (key) |
| **Send an edit** (`POST /show`) | becomes a **proposal** (HTTP 202); nothing changes | applies at once (key) |
| Approve or reject a proposal | **no** | yes (key) |
| Undo a send | no | yes (key) |
| Arm a laser | **no** | yes (key) |
| Try a cue, test-fire a device | no | yes (key) |
| Press GO / send an input / reset (HTTP or WebSocket) | **no** | yes |
| Disarm a laser, STOP LASERS | yes (the safe direction) | yes |
| Report a department READY | yes | yes |

An agent holding the operator key is still an agent: declaring `agent:` only ever removes powers.

## Proposals

`POST /show` from an agent is checked exactly like a real send (a show that would break the running scene is refused)
and then stored. Studio's Live tab shows it under **Waiting for you** with what would change, and a person presses
**Approve and apply** or **Reject**. Over HTTP: `GET /proposals`, `POST /proposals/<id>/approve`, `POST /proposals/<id>/reject`.

If the show changes in the meantime (any edit, by anyone) every older pending proposal becomes **superseded** and can no
longer be approved, because it was made against a show that is no longer running. The agent proposes again.

## Receipts

Start the runtime with `--governance <folder>` (or `PCHI_GOVERNANCE`). Every decision is then written as a receipt, in
PrimeSwarm's own format: hash over a canonical form, Ed25519 signature, chained to the one before. Each receipt names
the actor and records a hash of the state it was decided against (scene, show definition, which lasers are armed).

What is recorded: edits (sent, approved, refused), proposals, rejections, undo, arming and disarming, STOP LASERS,
test-fires and tried cues, every input that moved the show, resets, and **every refusal**, for example an agent trying
to arm a laser or a request with a missing or wrong key. Not recorded: the cues the clock fires by itself (the run
report has those), and inputs that did not change anything.

`GET /governance/export` returns the receipts. Check them anywhere, offline, with PrimeSwarm's verifier:

```bash
curl -s http://127.0.0.1:3100/governance/export > export.json
pip install pynacl
python scripts/verify_governance_chain.py export.json
```

It confirms the chain is unbroken, every hash is right and every signature is valid. Changing, removing or reordering a
receipt fails it. The signing key is kept in the same folder (`governance_key.hex`): protect that folder like the show
file, and keep a copy of the verifying key (printed at startup) somewhere the runtime's machine cannot change.

## What this does not do

- **The actor is a declaration, not a login.** It holds back honest agents and gives the record its names. An agent that
  is handed the operator key and lies about who it is can still act as a person. Keep the key away from agents.
  Per-person identity and roles are the next step.
- Receipts prove what the runtime decided and in what order. They cannot prove that the runtime itself was not replaced
  or that the machine's clock was right.
- A runtime started without `--governance` still holds agents back (the policy always applies) but writes nothing down.
- A person who is not logged in anywhere can still press GO when no operator key is set: set `--edit-key` for any show
  where that matters.
