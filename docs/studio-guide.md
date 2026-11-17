# Studio guide

Studio is the browser app for building a show and operating it. It has five tabs, one show model, and one undo history for Show and Timeline.

```
Show | Timeline | Cue sheet | Rig | Live
```

The show's name sits in the top bar. A dot **● unsaved** appears when there are changes since the last Save or Open; they are also kept in the browser, so a refresh does not lose them. **Save** downloads `<name>.pchi`; **Open** reads one. Press **?** at any time for the keyboard list.

## Keyboard shortcuts

| Keys | Where | Does |
|---|---|---|
| `Ctrl K` | everywhere | type or speak a command ([command-bar.md](command-bar.md)) |
| `Ctrl Z`, `Ctrl Shift Z` | everywhere | undo, redo (scenes and cues; rig systems are not in undo, remove them by name) |
| `?` | everywhere | the shortcuts list |
| Double-click | Show | add a scene, or rename one |
| `F2` | Show | rename the selected scene |
| `Delete` | Show | remove the selected scene or link |
| Drag from a scene edge | Show | connect two scenes |
| Double-click a lane | Timeline | add a cue |
| Drag, drag the right edge | Timeline | move, lengthen a cue |
| `Space` | Timeline | play or pause the preview |
| `Space` | Live | GO |
| GO twice | Live | go on although a department is on hold |

## Show tab: scenes and transitions

The graph of the show. Each node is a **scene**; each arrow is a **transition**.

- **Add and connect.** Double-click empty space to add a scene; drag from a scene's edge to another to link them. Or type: `intro then drop then finale`.
- **Transition triggers.** An arrow fires on a named *event* (`NEXT`, `START`, anything an input can send), after a *timer* (`after 30 seconds go to finale`), or when a *guard* on the show's context is true (`energy > 0.7`). A transition can combine an event and a guard.
- **Scene settings.** Label; enter and exit actions; a **minimum time** (`hold intro for at least 8 seconds`, see [scene-minimum.md](scene-minimum.md)); a timecode lock (see [timecode-and-midi.md](timecode-and-midi.md)).
- **Context and guards.** The show's starting context values and named guard expressions. Audience votes also land in the context ([virtual-events.md](virtual-events.md)).
- **Validation.** Problems are shown as you work: an unknown target, two unguarded transitions on one event (the second can never fire), a timer shorter than a minimum, an unreachable scene.

The **start screen** on an empty show offers templates (Festival set, Theatre show, Installation) and a text brief that *drafts* commands for you to read before pressing **Build it**. Nothing is built from a brief until you do.

### The timeline strip under the canvas

Select a scene on the Show tab and its timeline shows under the canvas (drag its top edge to resize it). It has the same cues as the Timeline tab and you can edit them **without leaving the Show tab**:

- **A ruler with real time marks** (`m:ss`), a readout of the time under the pointer (the grid line a click would land on), and each cue labelled with its exact start time. Times are written exactly as stored: a cue at 12 250 ms reads `0:12.25`, never a rounded `0:12.3`.
- **Move a cue** by dragging it; **change its length** by dragging its right edge. Both snap to the **Snap** grid (0.1, 0.25, 0.5 or 1 s, or none; the default is 0.25 s, remembered in this browser). Hold **Alt** to place freely. The exact time follows the pointer while you drag.
- **Type an exact time:** click the cue and type in its **Time** field (`12.5` or `0:12.5`), then Enter. Esc cancels, and anything that is not a time is marked and refused, never applied. **Length** works the same way (empty means none).
- **Nudge** the focused cue with ← and → (to the next grid line, so an off-grid cue lands on the grid) or Shift + ← → (one second).
- **Say what a cue does** in its **Value** box (it is named for the department: Scene, Look, Cue, Snapshot, Program). On a robot lane it offers the programs that device allows. Whether the tool accepts the value is the Checks panel's job: a program the robot does not allow is flagged the moment you press Enter.
- **Add** a cue by double-clicking a lane: it appears on the grid with its Value box focused, so **double-click, type, Enter** makes a complete cue. **Remove** the focused cue with Delete (or Remove). Every change is one step of the normal undo.
- **Dense lanes:** cue boxes are only as wide as they need to be; when one is too narrow for its time the time moves to the tooltip, and the selected cue is always wide enough to read.
- **Zoom:** Ctrl + scroll or the − / + buttons; **Fit** shows the whole scene. Zoomed in, the ruler shows finer marks (down to tenths of a second) and the strip scrolls sideways.

What it does not do: moving a cue to another lane, a cue's note and number, and which device a lane uses are still done on the Timeline tab or the Cue sheet. On the **Live** tab the strip has the ruler, readout and zoom but is read-only.

## Timeline tab: cues inside a scene

Pick a scene; its **lanes** appear, one per department (visual, lighting, laser, audio, other). Cues sit at an offset from entering the scene.

- **Add a cue:** double-click a lane, or type `at 4 seconds lighting look blue in intro`, or name a tool: `at 2 seconds qlab cue 3 in drop`.
- **Edit:** drag a cue to move it, drag its right edge to set its length (display only: the action fires once, at its time). Edit its time, value, note and cue number in the inspector.
- **Which device:** each lane has a picker for the rig device that takes its cues. With only one device for the department, it is chosen automatically; with several, you must pick (the Doors check says so if you did not).
- **Preview:** `Space` plays the timeline in the editor. It sends nothing.
- **Leaving a scene cancels cues that have not fired.** A cue due at the very moment the scene ends counts as cut off.
- **Cue numbers** are assigned once and kept (Q12; 12.5 for one inserted later). A deleted cue's number is never reused.

## Cue sheet tab

Every cue in calling order, for the stage manager. Printable. Numbers, scene, time, department, what it does. It is the same data as the Timeline, read down instead of across.

## Rig tab: who takes part

The rig is the set of systems and how they connect. Add them from **＋ Add** or by typing: `add qlab`, `add x32 called FOH desk at 192.168.1.20`, `add 3 lasers`.

- **Systems.** Tool profiles (Resolume, grandMA3, Eos, X32, BEYOND, QLab, MagicQ, Art-Net, sACN, OBS), generic devices (laser, lighting desk, audio console, projector, LED wall), renderers (Unreal, TouchDesigner, Notch, Vork) and others. Each has a **host**, **port**, and the **department** it serves.
- **Edges** say what a link *does*: activate, stream, trigger, control.
- **Check.** The rig tab flags lanes with no device ("no laser device in the rig"), several devices where one must be chosen, and lanes that point at a device that no longer exists.

How a cue becomes a tool's message is in [profiles.md](profiles.md); the connection model is in [connecting-systems.md](connecting-systems.md).

## Live tab: operating a running show

The Live tab connects to the runtime (WebSocket `/ws`). It shows:

- **Where the show is:** the current scene, how long it has run, the context values, and a timeline strip with what is due next.
- **GO.** Takes the first way out of the scene that would fire now. If a department that the *next* scene needs is on hold, GO asks for a second press ("GO ANYWAY"). If the scene has a minimum that has not elapsed, the press is **held** and the button counts down.
- **Ready board.** A chip per department (and per device): *ready*, *on hold*, or *no link*. Click one to set it. Chips also show whether a laser is **armed**, whether a device's link is lost, and what became of its last cue (sent, dry-run, failed).
- **Inputs:** STOP LASERS (always allowed, the safe direction), arm a laser (operator only), try a cue, test-fire a device.
- **Spoken or typed live commands:** "lasers ready", "hold lighting".
- **Event console:** the stream of what the runtime is doing.
- **Role banner:** says who you are and what you may do; **Sign in** with your own key when the runtime has a roster. When the runtime is a **standby**, a blue note says so; when it has **paused** because it lost its lease, the banner says that. If a backup address is configured, Studio can follow a takeover to it.

### Runtime tools (buttons on the Live tab)

| Button | What it does | Read |
|---|---|---|
| **Send show to runtime…** | shows the diff (scenes, cues, minimums, devices, what would not work), then sends the edited show to the running one; or **Propose for approval** | [roles.md](roles.md) |
| **Undo last send** | puts the running show back as it was before the last send | |
| **Doors check** | the whole show against the rig and each tool: what will not work | |
| **Rehearse** | plays the edited show against the run so far; nothing is sent | [rehearse.md](rehearse.md) |
| **Run report** | cue timing, failed sends, edits made mid-show | [report.md](report.md) |
| **Hand-over page** | the whole run as one page to hand over | [report.md](report.md) |
| **Memory** | ask the show's memory; review drafts; settle disagreements | [memory.md](memory.md) |
| **Replay a run** | drag through a run, or open a saved recording | [replay.md](replay.md) |
| **Waiting for you** | edit proposals from agents, with the agent's reason and the rehearsal | [mcp.md](mcp.md), [narration.md](narration.md) |

Sending an edit may need the operator key (`--edit-key`) or an operator or editor role. Studio keeps a key in the browser tab only.

## Department pages

A department can have its own page (the `DeptPage` view) so the laser operator or the lighting programmer sees only their department's readiness and cues, and can report READY or hold without seeing the rest. They connect to the same runtime; their role is limited in the roster ([roles.md](roles.md#limit-a-person-to-a-department)).

## What Studio does not do

- It does not talk to your gear. The runtime does.
- It does not decide who may do what. The runtime does; Studio's buttons only ask.
- It is a single-show editor: no multi-user simultaneous editing of one show file.
