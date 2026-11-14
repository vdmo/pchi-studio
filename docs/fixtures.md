# Fixture profiles and the patch

Channel numbers are fine for one dimmer, and hopeless for a stage of LED pars. A **fixture type** says what a fixture's channels mean;
the **patch** says which fixtures sit where on a DMX device. A cue can then say what it wants:

```text
wash1=blue lamp=40% walls.white=100% over 2
```

## The patch

Add a `patch` parameter to an Art-Net or sACN device in the Rig tab (one line of text):

```text
wash1=rgbw@1, wash2=rgbw@5, spot=rgb-dimmer@20, lamp=dimmer@30, walls=wash1+wash2
```

- `name=type@address` places a fixture: `rgbw` at DMX address 1 uses channels 1 to 4. A fixture with several modes says which:
  `spot=mover-x:long@40`.
- `name=a+b` makes a **group**; a cue on the group goes to every member.
- Names start with a letter. Two fixtures may not share a channel: the patch says which two and which channel.
- The Doors check reads the patch (*"Rack: patch places 3 fixtures: wash1, wash2, spot"*) and fails on a patch that does not make sense.

## Writing a cue

| You write | It means |
|---|---|
| `wash1=blue` | a color word sets red, green, blue (and clears white) on that fixture |
| `wash1=40%` | a level (a percentage, `0` to `255`, `full`, `out`) is the dimmer |
| `walls=out` | the same, for a group |
| `wash1.white=100%` | any attribute the fixture has: `dimmer`, `red`, `green`, `blue`, `white`, `pan`, `tilt`, `strobe`... |
| `spot.strobe=fast` | a **named value** from the fixture file |
| `1-3=50%` `blackout` | raw channels still work, in the same cue |
| `... over 2` | everything in the cue fades over 2 seconds |

Colors known: red, green, blue, white, warm, cool, amber, orange, yellow, cyan, magenta, pink, purple, off. A fixture with its own
white or amber emitter uses it for those colors; a plain RGB fixture mixes them. A fixture with no red, green and blue cannot show a
color, and the cue says so. A cue that names a fixture that is not patched lists what is.

16-bit attributes (pan, tilt) take a percentage to full resolution, as a coarse and a fine channel.

## Fixture files

Four generic types are built in: `dimmer`, `rgb`, `rgbw` and `rgb-dimmer` (dimmer, then red, green, blue). For anything else write a
file and give the folder to the runtime with `--fixtures <folder>` (and to `pchi-gearcheck`). `GET /fixtures` lists what is loaded.
A file with a built-in's `id` replaces it.

```json
{
  "id": "mover-x",
  "name": "Mover X",
  "notes": "From the Mover X manual, page 22.",
  "modes": [
    {"name": "short", "channels": 4, "attributes": {
      "pan": {"channel": 1, "fine": 2},
      "dimmer": {"channel": 3},
      "strobe": {"channel": 4, "values": {"off": 0, "slow": 40, "fast": 200}}}},
    {"name": "long", "channels": 6, "attributes": {
      "pan": {"channel": 1, "fine": 2}, "tilt": {"channel": 3, "fine": 4},
      "dimmer": {"channel": 5}, "strobe": {"channel": 6}}}
  ]
}
```

- Channels are numbered from 1 within the fixture. `fine` is the channel with the low byte of a 16-bit attribute.
- `values` are names a cue can use (`strobe=fast`), each a DMX value 0 to 255.
- Attribute names are lower case letters, digits and `_`. The file is refused if a channel is outside the fixture, two attributes use
  one channel, or a value is out of range, and it says which.

## What this does not do

- **A fixture file is only as good as the manual it came from.** Nothing here knows how a real fixture behaves. A wrong channel
  order is a wrong cue. Prove a type on the real fixture with `pchi-gearcheck` (it asks for levels, for example `wash1=blue`) before
  a show depends on it. None of the built-in types has been run against real fixtures.
- No ready-made library of real fixtures: only the four generic types. Write the file from the fixture's manual.
- No effects, no color mixing beyond the named colors, no patch editor in Studio: the patch is a line of text and the files are files.
- No merging with a console or other DMX source (see `artnet-sacn.md`).
