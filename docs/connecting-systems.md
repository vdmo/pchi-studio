# Connecting other systems to PCHI Studio

How lighting, lasers, audio and visuals at FOH take part in a show, today and next.

## What works now

The runtime (`pchi-studio-api`) is the one place the show lives. Everything else is a client.

| Who | How | Example |
|---|---|---|
| Studio (operator) | WebSocket `/ws` | the Live tab |
| Any desk, script, button box, Stream Deck / Bitfocus Companion | plain HTTP | `curl -X POST host:3100/ready/lighting -d '{"ready":true,"note":"cue 12 loaded"}'` |
| A department's own app | WebSocket `{"type":"ready","dept":"laser","ready":false,"note":"safety check"}` | |
| A hardware GO button | `POST /input {"event":"NEXT"}` | |

- **Readiness** is "I am standing by" / "I am on hold", per department. It never blocks the
  show: the operator sees it on the Live tab and GO asks for a second press while anyone is on hold.
  Departments that have not reported show as **no link** and do not stop GO.
- Every readiness change and every input is recorded in the event store, so a night can be replayed.
- Voice/typing on the Live tab: "lasers ready", "hold lighting".

### Putting it on the venue network

```bash
pchi-studio-api show.pchi.json --bind 0.0.0.0:3100 --token <secret> --db show.db
```

- Without `--token`, anyone on the network can send inputs. The server warns when you bind to a
  non-local address without one.
- Clients pass the token as `?token=<secret>` or `Authorization: Bearer <secret>`.
- Studio reads the token from the browser: in the console, `localStorage.setItem('pchi.token', '<secret>')`.
- The token is plain text on the wire. Use a closed show network, or put the runtime behind TLS
  (a reverse proxy) if the network is shared with the public or with guests.

### Sending cues to the rig

Run the runtime on a Studio `.pchi` file (Save from Studio) and it knows the rig as well as the show:

```bash
pchi-studio-api my-show.pchi            # dry run: reports where each cue WOULD go
pchi-studio-api my-show.pchi --send     # really sends
```

- Each cue goes to its lane's device (chosen on the Timeline), or the only device for its department.
  Scene enter/exit actions go by their kind (`lighting.look` is lighting, and so on).
- The message is generic OSC over UDP to the device's `host` and `port` (set in the Rig tab):
  address `/pchi/<kind>` (`lighting.look` becomes `/pchi/lighting/look`), arguments are the action's
  values in key order. A desk with its own OSC dialect needs a small adapter to translate it.
- Studio's Live tab shows, per device, READY and what became of its last cue (sent, dry-run, failed),
  and lists cues that no device could take ("no laser device in the rig").
- A device may report readiness under its own id (`POST /ready/<device id>`); otherwise its
  department's report stands for it.
- **UDP has no acknowledgement.** "sent" means the packet left this machine, not that the desk acted on it.
  Only the device's own READY report (or a feedback adapter) says that.
- A failed or unreachable send is reported and never stops or delays the show.
- Sending is off unless `--send` is given, so opening a show file can never fire a laser by accident.
  Use a dry run first and test each device before doors.

### Department pages, live edits, report, test fire

- **Department page.** Open `http://<studio host>/?dept=lighting` (or a device id, `?dept=laser-1`) on a
  tablet or desk. It shows what that department has coming (STANDBY within 10 s, GO when due) and one big
  READY button. While READY it checks in every 10 s; if it stops for 30 s the operator's board shows
  "no link" rather than a stale READY. Add `?token=...` once if the runtime has a token (it is kept in
  that browser and removed from the address bar). Check-ins are not written to the event log.
- **Call sheet.** On Live, the **Call sheet** view follows the running show: STANDBY and GO per cue, done
  cues struck through, and the next scene's first cues below.
- **Send show to runtime.** Live > "Send show to runtime…" shows exactly what would change (scenes,
  transitions, cues moved/added/removed, devices) and any effect on the scene running now (for example a
  cue moved to a time already past will not fire late). It is refused if the running scene was removed.
  The scene, its clock and the context are kept. `POST /show` (`?dry=1` to preview) does the same.
- **Who must be ready.** GO warns only about the departments (or devices) the NEXT scene has cues for.
  A department on hold that the next scene does not use is shown but does not warn
  ("Audio on hold but not needed in INTRO"). Chips the next scene needs are marked.
- **Operator key.** Start the runtime with `--edit-key <secret>` (or `PCHI_EDIT_KEY`) and sending a show,
  undoing a send and test-fire need the key (`X-Pchi-Edit-Key` header or `?edit_key=`). Department pages and
  viewers can still read the show and report READY. Studio asks for the key once per browser tab (kept in
  that tab's session only). This is one shared key for the operator, not per-person roles. It is sent in
  clear text unless you put the runtime behind HTTPS, so treat it like the token.
- **Undo a send.** The runtime remembers the last 10 versions it replaced. "Undo last send" (or `POST /show/undo`,
  `?dry=1` to preview) puts the running show back, scene and clock kept. Your Studio file is not touched.
- **Heads-up before sending.** If an edited cue still to fire in the running scene goes to a device that is
  on hold, or has not reported READY, the preview says so. It never blocks the send.
- **Run report.** Live > "Run report" (or `GET /report`): each cue's planned and actual time, failed sends,
  cues with no device, and edits made mid-show.
- **Test fire.** Live > "Test <device>" (or `POST /devices/<id>/test`) sends one harmless `/pchi/test`
  message; in a dry run it only reports where it would go. With `--send` Studio asks first (and warns for
  lasers).
- **Brief.** On the start screen, paste one scene per line (with a length and the words lights, lasers,
  visuals, audio) and press "Draft the commands". It is a small parser, not an AI: it only fills the command
  box for you to read and edit, and lists what it did not understand.

**Not done, and why:** nothing here has been tried against real Strobe, a lighting desk, a laser or an audio
console (only against a local UDP listener); desk-specific protocols (MIDI, Art-Net, vendor OSC dialects) need
an adapter per device, and which ones to build depends on the actual gear.

## Next, in order

1. **Departments see the show too.** A small page or app per department that shows "next up" and a
   READY button, so lighting can tap ready on a tablet instead of calling it. (The data is already
   in the feed.)
2. **Stale readiness.** A heartbeat so a department that drops off the network turns to "no link"
   instead of staying "READY".
3. **Required departments per scene.** The next scene's timeline lanes say who must be ready; GO
   warns only for those.
4. **Adapters that speak the desks' languages**, each a small program that turns PCHI actions into
   the protocol and reports readiness back:
   - OSC (Strobe, Resolume, QLab, many audio desks): Strobe adapter exists in `pchi-strobe-adapter/`.
   - MIDI (lighting desks, controllers, foot pedals).
   - Art-Net / sACN (lighting fixtures and nodes), only if PCHI is to drive lights directly.
   - Audio consoles with their own network control (e.g. OSC on X32/Wing, Q-SYS/Dante control).
5. **More than one Studio.** A second operator view (read-only, or with its own GO) connecting to
   the same runtime. The feed already allows this; what is missing is roles (who may send GO).

## Bluetooth, honestly

- Browsers can talk to Bluetooth Low Energy devices (Web Bluetooth: Chrome and Edge only, not
  Safari or Firefox, and the user must grant each device). That suits a **BLE button box or foot
  pedal** acting as GO or READY, via a small bridge page.
- Bluetooth is a poor fit for show data: short range, one-to-few, easily disturbed by a crowded
  2.4 GHz venue. Use a wired or Wi-Fi LAN for the runtime and the departments; use Bluetooth only
  for a personal controller beside the operator.
- Phones and tablets of department leads should join over Wi-Fi to the same runtime, not pair.

## Safety notes

- The show must keep running if a connection drops: the runtime owns timers and cues, so a lost
  Studio or department link never stops or restarts a show.
- Anything that can send `/input` can move the show: treat the token like a stage key.
