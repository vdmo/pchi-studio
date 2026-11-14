# FOH proof: what is proven, what is not, and how to prove the rest

The aim: an operator at front of house can trust PCHI on the night. That means three things, in this order:
it **runs on time**, it **survives trouble**, and it **tells the truth** about every tool it talks to.

## 1. What has been measured here (no real equipment)

Run it yourself: `cargo test -p pchi-studio-api --lib soak -- --ignored --nocapture`

A show of 800 cues, four real UDP devices on loopback (a Resolume, grandMA3, BEYOND and X32 profile each),
600 cues spread over 20 s and a **burst of 200 cues at one instant**:

| Measure | Result (debug build; a release build is tighter) |
|---|---|
| Cues fired | 800 of 800, each exactly once |
| Fire time against plan | median 6 ms, 99th percentile 33 ms, worst 34 ms |
| Packets that arrived | 800 of 800 (loopback) |
| Packet arrival against plan | median 7 ms, worst 54 ms (in the burst) |
| Failed or held deliveries | 0 |

What this shows: the runtime's own timing and fan-out are not the weak point. What it does **not** show:
network behaviour on a real venue LAN (loss, Wi-Fi, switches), the tools' own reaction time, or a multi-hour run.

## 2. Survives trouble

| Trouble | What happens | Proven by |
|---|---|---|
| Studio (the browser) closes or crashes | Nothing: the runtime owns the clock and keeps firing cues | by design; the runtime has no dependency on Studio |
| The runtime is restarted mid-show | `pchi-studio-api show.pchi --db show.db --resume` carries on in the same scene with its context; cues still ahead fire; cues that came due while it was down are **skipped and listed**, never fired late; lasers start disarmed; departments report READY again | tests `a_restarted_runtime_resumes...`, `cues_that_came_due_while...` |
| The file given on resume no longer matches the recorded run | Refused with the reason (a transition changed), so a wrong show is never resumed by accident | test `resume_refuses...` |
| A device is unreachable or a value cannot be sent | Reported as a failed delivery; the show goes on | tests `a_device_that_cannot_be_reached...` |
| A tool stops answering | Link goes to "no reply"; GO warns if the next scene needs it | tests in `links` |
| A laser | Held until armed, always disarmed after a restart, STOP LASERS needs no key | tests `lasers_get_nothing_until_armed...` |

Not covered: the machine running the runtime losing power with no `--db` (use `--db` for any real show),
two runtimes started by mistake, and a database on a failing disk.

## 3. Tells the truth

- **Doors check** (Live tab, or `GET /preflight`): checks every cue and every scene action against the device and
  the tool's own profile, each device's address, link, READY and (for lasers) arming, and says "ok", "look at it" or
  "will not work". A cue whose value a tool cannot use is a failure *before* the show, not during it.
- **Try a cue** (Live tab, or `POST /devices/<id>/try`): send one cue to one device without any show, and see the
  exact OSC address and values sent. In a dry run it shows what *would* be sent.
- **Run report**: planned against actual time for every cue, failed sends, edits made mid-show.
- Every address in the profiles is marked *confirmed* or *not confirmed* in `connecting-systems.md`.

## 4. What only real equipment can prove: the checklist

Do these once per tool, on the real thing or its software version, with the PA muted / the laser safe.
Write the result next to each line; an unticked line is an unproven claim.

For each tool:

1. Put its real IP and port in the Rig tab. Set its own OSC input to the same port.
2. **Dry run first**: start `pchi-studio-api show.pchi`, open Live > Doors check. No red lines.
3. Start with `--send`. **Try a cue** with a value you can see or hear working. Does the tool react as expected?
   Note the exact address shown, and whether it matches the tool's own OSC monitor.
4. If the tool sends feedback: set `listen_port` and check "Heard from devices" shows its messages.
5. Unplug its network cable for 20 s: does the Doors check / link status notice, and recover when you plug it back?
6. Restart the runtime with `--resume` mid-scene: does the tool end up in the right state?

Per tool, what to look at:

| Tool | Try a cue with | Address to confirm | Status |
|---|---|---|---|
| Resolume Arena | `3` (column) and `2/5` (clip) | `/composition/columns/3/connect`, `/composition/layers/2/clips/5/connect` | not yet tried |
| grandMA3 | `402` (executor) | `/cmd` "Go+ Exec 402" with *Receive Command* on | not yet tried |
| ETC Eos | `2`, `3/9` | `/eos/cue/2/fire`, `/eos/cue/3/9/fire` | not yet tried |
| Behringer X32 / M32 | a scene number (**mute the PA first**) | `/-action/goscene` (**not confirmed**), and `/info` probe | not yet tried |
| QLab | a cue number | `/cue/<n>/start` (re-check) | not yet tried |
| ChamSys MagicQ | `3` | `/pb/3/go` | not yet tried |
| BEYOND | a cue name (**laser safe, arm first**) | `/beyond/general/StartCue`, and STOP LASERS: `BlackOut`, `DisableLaserOutput` | not yet tried |

## 5. The honest summary

PCHI's own part (timing, routing, recovery, reporting, laser handling) is measured or tested. Its claims about
each tool's behaviour are exactly as strong as the checklist above, and today **none of the seven has been run
against the real tool**. Do not rely on a profile for a real show until its line has been ticked.
