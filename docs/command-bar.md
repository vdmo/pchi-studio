# The command bar: building a show by typing

Press **Ctrl K** (or click the box on the Show or Timeline tab) and type what you want in plain words.
"Scene" and "state" mean the same thing. Everything is applied at once, and the scene part can be undone
with Ctrl Z. If a sentence is not understood, the bar offers the closest command that works, using
the names in your show: click it to put it in the box.

The **＋ Add** button next to the box is the quick way in: a scene name box, and one button per common
system (QLab, X32, Resolume, grandMA3, Eos, MagicQ, BEYOND, and generic lasers, lighting desks and audio
consoles).

## Scenes

| Say | What happens |
|---|---|
| `add scene drop` / `new scene drop` / `add a scene called drop` | adds a scene |
| `add scenes intro, drop and finale` | adds several (no links between them) |
| `intro then drop then finale` / `intro -> drop -> finale` | adds them and links each to the next |
| `make a show with intro, drop and finale` | the same, as a whole show |
| `from intro to drop on go` / `go from intro to drop` | a transition on an event |
| `after 30 seconds go to finale` | a timed transition out of the scene you are looking at |
| `rename intro to opener` | |
| `delete scene drop` / `delete drop` / `remove the drop scene` | removes it and its links |
| `duplicate scene intro as intro_b` | copies a scene and its cues (the copy gets its own cue numbers and no links) |

## Systems (the tools and devices the show talks to)

| Say | What happens |
|---|---|
| `add qlab` | adds QLab at the tool's usual port |
| `add x32 called FOH desk at 192.168.1.20` | a name and an address (add `:10023`, or `port 10023`, for the port) |
| `add resolume on 10.0.0.5` | |
| `add eos listening on 8001` | the port the tool sends feedback to |
| `add grandma3 for visuals` | the department it serves, if not the usual one |
| `add 3 lasers` | several at once (named Laser, Laser 2, Laser 3) |
| `set qlab host to 192.168.1.50` / `set foh port 10024` | change an address or port |
| `rename qlab to Playback` | |
| `qlab for visuals` | change the department it serves |
| `remove foh` | takes it out of the rig |
| `use foh desk for audio` / `use qlab for audio in intro` | picks which device gets a department's cues (the same as the picker on each Timeline lane): in every scene, or just one |

Names understood: resolume, grandma3 (ma3), eos, x32 / m32 / behringer, beyond / pangolin, qlab,
magicq / chamsys, laser, lighting desk, audio console, touchdesigner, notch, unreal, vork, projector, led wall.
Systems are **not** part of Ctrl Z yet: say `remove <name>` to take one out.

## Cues, with a tool's own name

`at 4 seconds lighting look blue in intro` still works. You can also name the tool:
`at 2 seconds qlab cue 3 in drop`, `x32 scene 12`, `eos cue 5`, `grandma3 402`, `resolume 2/5`,
`beyond sweep`, `magicq 3`. The value is whatever that tool's profile understands (see
`connecting-systems.md`).

## More than one thing at once

Separate parts with `;`. The whole sentence is applied together or not at all:
`add scene encore; add qlab; at 2 seconds qlab cue 3 in encore`.
