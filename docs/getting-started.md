# Getting started

From a fresh checkout to a running show with Studio open, in about ten minutes. Nothing here sends anything to real gear.

## What you need

- **Rust** (stable) with `cargo`, to build the runtime and tools.
- **Node.js 18 or newer** with `npm`, to run Studio.
- Optional: Python 3, to run `scripts/verify_governance_chain.py` and the protocol bridges.
- Optional system libraries for timecode inputs (see below).

## 1. Build

```bash
cd pchi-life-2.5
cargo build --release          # builds every binary in the workspace
```

The binaries land in `target/release/`:

| Binary | What it is |
|---|---|
| `pchi-studio-api` | the runtime |
| `pchi` | showchart tools: validate, simulate, inspect, run, replay |
| `pchi-drill`, `pchi-gearcheck`, `pchi-netcheck`, `pchi-venueday` | pre-show checks |
| `pchi-witness`, `pchi-follow` | failover helpers |
| `pchi-import`, `pchi-run-report`, `pchi-memory`, `pchi-mcp` | import, reporting, memory, agents |

Put `target/release` on your `PATH`, or call the binaries by path. The examples in this manual assume they are on the `PATH`.

**MIDI and LTC timecode** are off by default because they need a system library. Build with `--features midi` and/or `--features audio` on the runtime (`cargo build --release -p pchi-studio-api --features midi,audio`); on Linux install `libasound2-dev` first. `pchi-studio-api --midi-ports` and `--audio-devices` list what the machine has. See [timecode-and-midi.md](timecode-and-midi.md).

## 2. Check a show without running it

The repository ships example shows:

```bash
pchi validate crates/pchi-showchart/examples/timelines.pchi.json   # structural problems
pchi inspect  crates/pchi-showchart/examples/timelines.pchi.json   # scenes, actions, events, dead ends
pchi simulate crates/pchi-showchart/examples/timelines.pchi.json   # every path through it; emits nothing
```

`validate` catches unknown targets, two unguarded transitions on the same event, bad guards, a timer shorter than a scene's minimum, a bad timecode, and unreachable scenes.

## 3. Start the runtime

```bash
pchi-studio-api crates/pchi-showchart/examples/timelines.pchi.json --db show.db
```

It listens on `127.0.0.1:3100`, in a **dry run**: cues are reported ("would go to ...") and nothing leaves the machine. `--db show.db` records the run to SQLite; without it the log lives in memory and is lost on exit.

Check it is alive:

```bash
curl http://127.0.0.1:3100/state
curl http://127.0.0.1:3100/preflight
```

## 4. Start Studio

```bash
cd pchi-studio-react
npm install
npm run dev          # http://localhost:5173
```

Studio's dev server proxies `/api` to the runtime on `127.0.0.1:3100` (and `/core` to the PrimeSwarm conductor on `:3000`). Open `http://localhost:5173`.

For a production build: `npm run build` and serve `dist/` from any static host. If it is served from a different origin than the runtime, start the runtime with `--allow-origin http://host:port`.

## 5. Build or open a show

On the **Show** tab you can:

- press **Ctrl K** and type `chain preshow, intro, drop, finale` (see [command-bar.md](command-bar.md)), or pick a template on the start screen (Festival set, Theatre show, Installation), or
- **Open** a `.pchi` file (the examples under `crates/pchi-showchart/examples/` are plain showcharts; Studio also saves full `.pchi` files holding the rig).

Then add systems on the **Rig** tab (`add qlab`, `add x32 called FOH desk at 192.168.1.20`), place cues on the **Timeline** tab, and **Save**. See [studio-guide.md](studio-guide.md).

## 6. Run it

Save from Studio as `my-show.pchi`, then restart the runtime on it:

```bash
pchi-studio-api my-show.pchi --db show.db            # dry run
```

Open the **Live** tab. Press **GO** (or Space) to move the show on. Mark departments ready or on hold. Watch each device's delivery status: `dry-run` until you add `--send`.

When the rig is real and checked:

```bash
pchi-studio-api my-show.pchi --db show.db --send
```

Before you do, work through [show-day-handbook.md](show-day-handbook.md).

## 7. After the run

```bash
pchi-studio-api --db show.db --export-run run.json     # a recording you can keep and send
pchi-studio-api --replay run.json                      # print it as a timeline and check it replays
pchi-run-report run.json --out report.html             # the one-page hand-over report
```

## A first networked setup

When other machines (a desk, a button box, department pages) take part, bind to the network and protect it:

```bash
pchi-studio-api my-show.pchi --db show.db --bind 0.0.0.0:3100 \
  --token "<secret>" --roster people.json --governance ./receipts --send
```

Create roster entries with `pchi-studio-api --new-roster-entry Alex:operator`. Read [security.md](security.md) before doing this on a shared network.

## Running the tests

```bash
cargo test                                   # runtime, engine, memory, store, protocol
cd pchi-studio-react && npm install && npm run typecheck && npm run test:sdk && npm run test:ui
```

`test:sdk` tests the pure logic in `pchi-sdk` (rig checks, cue sheet, commands). `test:ui` renders the Studio's screens in a simulated browser (jsdom) against a stand-in runtime and checks that it starts, every tab opens, and the main flows work: adding a device from a runtime profile, a new cue starting as a kind its profile takes, the live cue check, and the Tool profiles dialog. It also scans the source for re-encoded characters. It cannot see layout or colour, so look at a visual change in the app too.
