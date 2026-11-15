# Security model

How the secrets, roles and record fit together, and what they do not protect against. The detail lives in [roles.md](roles.md), [tls.md](tls.md), [governance.md](governance.md) and [failover.md](failover.md); this page is the whole picture.

## Principles

- **The runtime decides.** Studio, agents and every other client only *ask*. Roles are enforced in the runtime and refusals are written down.
- **The safe direction is never locked.** Anyone, signed in or not, may disarm a laser, press STOP LASERS, rehearse and read.
- **Agents can only make things safer or propose.** An agent's edit becomes a proposal; it cannot press GO, arm a laser, approve, undo or test-fire. Declaring `agent:` only ever removes powers, even if the agent holds the operator key.
- **Lasers start disarmed**, after every start, resume and takeover.
- **Dry run by default.** Nothing is sent without `--send`.
- **Memory is outside the show's path.** No cue, timer, takeover or laser touches it.

## The secrets, and what each one opens

| Secret | Flag | Opens | Must differ from |
|---|---|---|---|
| Feed token | `--token` | read and talk to the runtime at all | all others |
| Operator/edit key | `--edit-key` | send an edit, undo, test-fire (single shared key) | |
| Roster keys | `--roster` | one person each, with a role; hashed with PBKDF2-HMAC-SHA256 (200000 rounds) | |
| Audience token | `--audience-token` | `POST /vote` only | `--token`, `--edit-key` |
| Failover key | `--failover-key` | a standby reading the log, fencing a primary, a witness lease | every other |
| Agent key | `pchi-mcp --key` | a roster entry of role `agent` | |
| Governance signing key | `--governance[-key]` | signs receipts. The one thing that makes the record trustworthy | |

## Layers

1. **Network.** Bind to loopback unless other machines must reach it; with a non-local `--bind` and no `--token` the runtime warns. Put HTTPS (Caddy, nginx) in front on a shared network and run the runtime on loopback, then pass `--behind-tls`. Tokens and keys otherwise cross the network in clear.
2. **Who you are.** A roster of named people and roles; Studio's **Sign in** exchanges a key for a session token that lasts `--session-hours` (default 8). Ten wrong keys a minute make sign-in wait. `--sessions FILE` keeps only token hashes, so a restart does not sign everyone out.
3. **What you may do.** Roles: operator, editor, lso, guest, agent, button; an entry can be limited to departments.
4. **What was decided.** `--governance DIR` writes a signed, hash-chained receipt for every decision, including refusals. `scripts/verify_governance_chain.py` verifies the export without trusting PCHI's code. Keep the **verifying key** somewhere the show machine cannot change it.
5. **Inputs.** OSC, MIDI and LTC inputs press only a few things (GO, a context value, STOP LASERS). They never arm or edit. Listening beyond the machine needs an allow list of sender addresses.
6. **Failover.** The failover key lets a standby read the log and stop the primary. A witness lease prevents two senders.

## Files that hold secrets

Protect these like the show file and keep them out of version control: the roster (hashed or, worse, clear keys), the sessions file, the governance folder (it holds the signing key unless `--governance-key` is used), and any environment holding tokens. Recordings and reports hold **no keys or tokens** but do hold names, reasons typed, and rig addresses.

## What it does not protect against

- A person with a valid operator key doing something harmful: it is recorded, not prevented.
- A machine whose operating system is compromised: it can read the signing key and the log.
- Anyone on the network when HTTPS is not used: tokens and keys are visible in clear.
- The receipts' verifying key being replaced if you did not write down the original elsewhere.
- Ballot stuffing in audience polls: the gateway must give each person one id.
- A single witness failing: a primary keeps its lease while it can reach the witness, but a standby cannot take over without it (unless a person overrides with `POST /takeover?witness=ignore`, which is noted in the receipt).

See [LIMITATIONS.md](LIMITATIONS.md) for the protocol layer's limits (Only-Lang is a documented subset, and so on).
