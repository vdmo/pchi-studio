# Gear check: prove each tool, one at a time

`docs/foh-proof.md` says none of the seven tool profiles has been run against the real tool. `pchi-gearcheck` is how you
fix that: a guided walk through your rig, device by device, that records what actually happened.

```bash
pchi-gearcheck my-show.pchi --update-doc docs/foh-proof.md
```

It reads the rig from a Studio `.pchi` file. **It sends real messages**, one device at a time, and only after you say yes.
Use `--only id1,id2` to check some devices, `--report file.md` to name the report.

## What it asks, per device

1. **Safety.** A laser needs you to type `LASER SAFE` (beam off or blocked at the laser software, nobody in the path). An audio
   desk needs `MUTED` (mute the PA first). Without the word, that device is skipped and nothing is sent.
2. **A harmless value.** You choose the cue, scene or column to try. There is no default, so nothing fires by surprise.
3. **The exact message.** It shows the OSC address and values it will send, and asks "Send it now?".
4. **Did the tool react?** yes, no or not sure. Your word: the check cannot see or hear the tool. If no, it asks what you
   saw and what the tool's own OSC monitor showed.
5. **What the tool sends back.** For tools with a listen port (or the X32, which answers a probe), it listens, shows what it
   heard, and asks whether that matches the tool's own OSC monitor.
6. **Cable pull** (tools that answer a probe, today the X32): it asks you to unplug the cable, times how long the link takes
   to notice, then to plug it back and times the recovery.

A laser is armed only inside the check, for the one cue, and disarmed straight after. The runtime itself always starts
disarmed.

## The result

- A markdown **report** with what was sent, what you said, what was heard and the cable timings.
- With `--update-doc`, the **Status** column of the checklist is rewritten **only for devices that were tried**:
  `ticked <date>: sent <message>, reacted (operator's word)`, or `tried <date>, NOT ok: <why>`.
  A skipped device changes nothing, so an unticked line stays an unproven claim.
- A device is **ticked** only if the message could be sent, you said the tool reacted, any feedback matched the tool's
  monitor, and (where tested) the link noticed the cable pull and recovered.

## What it does not prove

- That the tool did the *right* thing, only that you saw it do something you expected.
- Long runs, other versions of the tool, or a busy network. Do it again on the show's own network before the show.
- Tools with no profile (generic OSC devices) are skipped: only the tool's manual can confirm those.
- Only the X32 can be probed for the cable test. For the others, "heard" is all the link can say.

Tested here against a pretend X32 (answers `/info`, can be made to stop answering) and a scripted operator. Not yet against
a real tool: that is the point of running it.
