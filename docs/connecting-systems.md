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
