# The side panel: Checks, Code and AI

The **Checks** button in Studio's top bar opens a panel on the right with three tabs. The number on the button is how many problems the show has right now (red if any will not work, amber if all are only worth a look).

## Checks

Problems found **while you edit, without the runtime**, each with a place you can click to go to:

- a scene or transition that does not hold together (unknown scene, no initial scene, unreachable scene);
- a lane pointing at a device that was removed, a department with no device, or several devices and none chosen;
- a **motion cue for a program that is not on the robot's `programs` list**, a robot with no list at all, or a motion cue with no program;
- a cue after the end of its scene (it will never fire), two cues with the same number, a cue with no value;
- a device no cue uses.

On the **Cue sheet** a cue with a problem carries a ⛔ (will not work) or ⚠ (look at it). Nothing here blocks anything. The runtime's own doors check (Live tab) goes further: it knows what is actually reachable and what each tool said. These checks are the part that needs only the file.

## Code

The show as text, read only: the selected scene, or the whole show. Copy puts it on the clipboard. Device **passwords are hidden in this view** (a screen gets shared), so a copy from here is not a backup: Save writes the real file.

## AI

Describe a change; the AI returns a **proposal**: what it says it did, a list of exactly what differs from your show, and any problems it would add. You press **Apply to the draft** or Discard.

What keeps it safe next to a robot:

- **It can only propose.** Apply edits your draft and nothing else. It cannot change the rig, arm a device, or send anything to the runtime; sending is the usual separate step, with its own diff and roles.
- **It never sees where your devices are.** What leaves your browser: scenes, cues, and for each device its name, type, department and allowed program **names**. Not addresses, ports or passwords. (The panel says so before you press Send.)
- **A proposal that adds a problem the show did not already have cannot be applied** (for example a program the arm does not allow, or a device that does not exist). A problem the show already had does not block it.
- The canvas layout and the cue-number counter are kept from your show, so cue numbers are never reused.
- Applying is one undo step: **Undo this change** appears next to it, as long as you have not edited since.

How it works: your browser calls the Anthropic API directly with **your own key**, which is kept in this tab (session storage) and never saved to the show, the runtime or the server. You choose the model (the default is `claude-sonnet-5-5`). It needs an internet connection from the computer running Studio, which a venue network may not allow.

## What is not proven

- The AI request has been tested only against a stand-in for the API (so the panel, the proposal list, Apply and Undo are exercised, not a real model's replies). A real model may return something the panel refuses; it then says why and changes nothing.
- A model can misread an instruction in a way that passes every check, for example putting a cue at the wrong time. Read the list of changes before you apply it, and the checks do not replace a rehearsal.
- The checks look at the show and the rig as they are in the editor, not at the running system.
