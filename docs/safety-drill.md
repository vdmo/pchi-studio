# The show-day safety drill

Before the doors, run the drill:

```bash
pchi-drill my-show.pchi --interactive --sign ./receipts
```

It builds a copy of **this show** in memory, **sends nothing**, provokes trouble, and checks what PCHI does. It ends with the questions
only people can answer. The result is a report, a GO or NO GO, and (with `--sign`) a signed receipt tied to the exact show and rig by a hash.
The exit code is 1 on a NO GO, so a script can stop on it.

## What PCHI is made to do

| Drill | Provokes | Holds if |
|---|---|---|
| **Doors check** | the whole show against the rig | nothing will fail |
| **Lasers stay held** | cues to a disarmed laser, arming, STOP LASERS | held until armed, passed while armed, held again after STOP LASERS |
| **Keys, roles and agents** | no key, an agent holding the key, a guest, the laser safety officer | each may do only what its role allows |
| **Agents propose** | an agent sends an edit | it waits as a proposal, and changes the show only when a person approves |
| **A press that comes too early** | GO straight after entering a scene with a minimum | it is held |
| **A device goes away** | a device whose address cannot be reached | its delivery is reported failed and the show carries on |
| **A restart in the middle** | the runtime stops and is started again with `--resume` | it comes back in the same scene, and lasers are disarmed |
| **Undo a send** | an edit sent to the running show, then undo | the show is exactly as before |
| **Timecode stops and jumps** | timecode that plays, stops, then locates a minute ahead | the stop is noticed, the Doors check warns, the jump fires nothing |

A drill that does not apply to the show (no laser, no minimum, no timecode) is **skipped, and says why**: it is not a pass.

## What only people can say

The questions depend on what the show contains: a laser means a laser safety officer and a live STOP LASERS test; audio means the PA muted;
timecode means seeing it run on the Live tab; DMX means seeing the right fixtures light. With `--interactive` each is asked (y or n);
without it they are listed as **not asked**. **A single "no" makes it a NO GO.**

## What it tells you, and what it does not

It tests **PCHI's own behaviour, on this show's devices and scenes**. It does **not** test the venue's network, the tools, the fixtures, the
people, or a real laser. A GO from the drill means "PCHI will do what it promises with this show"; it is not "the show will go well".
Run `pchi-gearcheck` for the tools and fixtures, and answer the human questions honestly.

The stock example show **fails** the Doors check: its cue values are placeholders (`"intro"` is not a Resolume column). That is the drill doing
its job.

## The record

`drill-report-<date>.md` lists what held and what failed, with the show-and-rig hash. With `--sign <folder>` a receipt `drill.run` goes into
the governance log of that folder (use the runtime's `--governance` folder to keep it in one chain), `ALLOW` for a GO and `DENY` for a NO GO.
Verify the chain offline with `scripts/verify_governance_chain.py`. The hash shows which show and rig were drilled: change a cue or a device
and it differs.
