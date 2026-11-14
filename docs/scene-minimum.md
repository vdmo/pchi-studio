# Scene minimum: a scene that must run before it can be moved on

Learned from ossia score's time model, which separates "does this happen?" from "when do we move on?". A PCHI scene can now
say **how long it must run**. Its longest stay is a timer you already have ("after 30 seconds go to DROP").

## What it does

- A scene with `min_ms` **holds** any input that would move it on too early: a person's GO, a button, a Stream Deck, an
  agent-free script. The press is ignored, written into the run (`show.held`) and the receipts, and reported
  ("held: INTRO must run 4.8 s more before it can be moved on"). Pressing again after the minimum goes through.
- **A press with nothing to do here is not "held"**: only a press that would really have changed the scene is.
- **The show's own timers are not inputs.** A timer shorter than the minimum would be cut short, so that is an error
  ("state INTRO has a timer after 2000 ms but must run at least 5000 ms"), in the show file, the command bar and the runtime.
- **It is ignored, not queued.** Queueing a press to fire later could surprise the operator mid-show. The Live tab counts down
  on the GO button instead ("held for 3.2 s: INTRO must run 8 s first") and the operator presses again.

## Use it

- **Say it:** "hold intro for at least 8 seconds", "intro must run at least 8 seconds", "set intro minimum to 8 seconds",
  "remove the minimum from intro".
- **In the file:** `"min_ms": 8000` on a state.
- **Sending it to a running show** lists it in the diff: `~ scene INTRO: minimum none → 8.0s`.
- **Rehearse** applies it to the recorded presses: a press that came too early is reported as "would be held", and the show may
  then never reach the next scene in the rehearsal.

## What it does not do

- It does not stop the show's own timer, a reset, or a resume: those are not inputs.
- It is not a maximum. The maximum is the timer transition. ossia's third option, a trigger that is only accepted between the
  two, is exactly "a minimum plus a timer".
- It is per scene, not per transition: a scene's minimum holds every way out of it.
