# Narrated creativity: say it, see its effect, approve it

You say or type what you want. Studio turns it into an edit. Before it touches the running show, the people deciding can see
your words, what would change, and how it would have played against the run so far. The record keeps all three.

## The flow

1. **Say or type it** in the Show or Timeline command bar (the microphone works too): "move the laser cue to 40 seconds in the
   drop". The edit happens in your Studio show, as always, and the line is remembered.
2. **Send it:** Live tab > **Send show to runtime…**. The dialog shows what will change and a box, "What you asked for",
   already filled with the lines you used since the last send. Edit it if you like.
3. **Choose how:**
   - **Send now** applies it to the running show (if your role may edit).
   - **Propose for approval** stores it instead. Nothing changes yet.
4. **Someone approves.** The Live tab shows it under **Waiting for you**: who proposed it, their words in quotes, what would
   change, and **Against the run so far**: the same path or not, cues that would no longer fire, scenes that would run
   longer or shorter (the Rehearse result, worked out when it was proposed). They press **Approve and apply** or **Reject**.
5. **The record** (`--governance` receipts and the run report's "Who did what") holds the words next to the decision:
   `Eddie (editor): proposed edit #1 (2 change(s)) — said: "bring the lights in much later"; waiting for a person`, then
   `Alex (operator): approved proposal #1 from Eddie (editor) and applied it … — said: "bring the lights in much later"`.

## Agents

An agent sends the same thing over HTTP: `POST /show?said=<its reason>` with the show file. For an agent it is always a
proposal. Its reason is kept, the rehearsal is attached, and a person approves. An agent can also check an idea first with
`POST /rehearse`, which changes nothing.

## Rules worth knowing

- Anyone allowed to edit may ask for approval instead of applying (`?propose=1`). Someone who may not edit (a guest) cannot.
- The words are kept as typed: control characters are dropped and the line is cut at 300 characters.
- If the show changes while a proposal waits, the proposal is **superseded** and cannot be approved: propose again.
- The rehearsal on the card is as of the moment it was proposed. The run keeps going, so open **Rehearse** again for a fresh one.

## What it does not do

- It does not understand what you meant any better than the command bar does. If the bar says it cannot do something, that
  is the end of it: it never guesses at an edit.
- Nothing stops you proposing and then approving your own proposal. The flow gives a second pair of eyes; it does not force one.
- The words are what was typed or recognised by the browser's speech recognition. A mis-heard line is recorded as heard.
