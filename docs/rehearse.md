# Rehearse: try an edit against what already happened

Before you send an edit to a running show (or approve an agent's proposal), play it against the run so far. No gear, no
waiting, nothing is sent: the recorded run supplies what people and sensors really did, and the edited show is played
against it on a virtual clock.

## Use it

- **Studio:** Live tab > **Rehearse**. It uses the show open in Studio and the run the runtime has recorded.
- **HTTP:** `POST /rehearse` with the show file as the body. Read-only: no operator key, and agents may use it. That is how an
  agent checks an idea before proposing it.
- **After the night:** `pchi-studio-api show.pchi --db show.db --rehearse edited.pchi` prints the answer and exits. The run
  must have been recorded with `--db`.

## What you get

In plain sentences, per scene:

- whether the edited show takes the **same path** as the real run, and where it stops matching;
- cues that would **not fire** because the scene now ends first ("INTRO is left at 3.0s before it is due at 9.0s");
- cues that **move** or are **new**, and scenes that **last longer or shorter** (a timer you added or changed);
- recorded presses that would **do nothing** in the edited show, and where the real run went on without it;
- anything a tool **cannot be sent**, and what each cue would put on the wire (`/composition/columns/3/connect`).

## What it cannot tell you

- **People might press differently.** Their presses are replayed as they were, at the times they were made. A show that is
  much faster or slower might have been driven differently.
- Nothing about the tools themselves: only that PCHI can build the message.
- A cue due at the very moment its scene ends counts as cut off.
- If the real run was edited while it ran, the cue lists it shows are what was running at the time. The answer says so.
- The scene the run is in right now is judged only up to now: cues later than that are "not yet due", not cut off.
