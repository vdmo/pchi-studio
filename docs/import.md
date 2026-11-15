# Bringing a cue list in from another tool

```bash
pchi-import cues.csv --out gala.pchi
pchi-import workspace.qlab4 --out gala.pchi --map Audio=audio.snapshot
```

The result is an ordinary show file: run it with `pchi-studio-api gala.pchi`, or open it in Studio. It has a timeline of cues, a scene for each GO, and **no rig**: add your devices in Studio's Rig tab so cues have somewhere to go. Nothing is sent for a cue that was not mapped to an action.

Every cue that comes in without a PCHI action is listed when you import, and sits on a `notes` lane with the original's name, so the sheet is complete and nothing is dropped silently.

## A cue sheet (CSV or TSV)

The format every tool can export and every stage manager can type. The first row names the columns (any order, any case; comma, tab or semicolon):

| Column | Meaning |
|---|---|
| `time` (required) | when the cue fires, from the start of its scene: `1:23.5`, `0:01:23`, `83.5` or `250ms` |
| `action` (required) | `light`, `audio`, `laser` or `scene`, or a full kind such as `lighting.look`. Anything else becomes a note |
| `value` | what to do: the look, snapshot, laser cue or scene name |
| `label` | what the cue is called on the sheet (default: the value) |
| `number` | the cue number the stage manager calls (`12`, `Q12`, `12.5`) |
| `lane` | the lane to put it on (default: its department) |
| `length` | how long it lasts (display only) |
| `scene` | which scene it belongs to. Scenes run in the order they first appear, one GO each. Without it, one scene |

Lines starting with `#` are ignored. A row that cannot be read stops the import and names the row.

## A QLab 4 workspace (`.qlab4`)

Read from its property list: the cues of each cue list, timed from their **pre-waits** and **continue modes**.

- A cue that waits for a GO (continue mode "do not continue") starts a new **scene**, named from its number and name. Pressing GO in PCHI enters the next scene.
- **Auto-continue**: the next cue is triggered after this cue's post-wait, counted from when its action started (after its pre-wait). **Auto-follow**: after its action ended (its duration) plus its post-wait.
- A **Light** cue becomes `lighting.look` with the cue's name as the look. Every other type comes in as a note, unless you map it: `--map Audio=audio.snapshot` (repeat for more). A mapped cue carries the cue's name as the argument the kind expects.
- A **Group** is flattened: its children are timed from the group's start, continue and follow modes respected, and a child with neither starts with the group. QLab's group modes are not interpreted, and each flattened group is listed.

**What was checked.** The reader follows the documented structure of a QLab 4 workspace, and was tested on workspaces written by hand from that structure (timing, groups, mapping, refusal of what is not a workspace). **It has not been tried on real QLab workspaces**: a real one may use something it does not know, and the notes will say what it skipped. Check the first import against the QLab cue list before you trust it, and tell us what it got wrong.

## What is not read

- **QLab 5** workspaces (`.qlab5`): refused with a message. Save or export in QLab 4 form, or export the cue list as a sheet.
- **Chataigne** (`.noisette`) and **ossia** (`.score`) files: not read. We had no sample of either to check a reader against, and a reader written from memory would be a guess. Export or type the cue list as a sheet.
- Fades, levels, media files and what is *inside* a cue. Only when it fires and what it is called come across.
