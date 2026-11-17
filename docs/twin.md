# The rig twin: a stand-in for the gear

A **twin** is a set of fake devices on loopback that a real runtime can send to, so a whole show (and an agent working on it) can be exercised with nothing real involved, and so a device can be made to go wrong on purpose.

```bash
pchi-twin my-show.pchi --runtime 127.0.0.1:3100         # stand the devices up, write my-show.twin.pchi, report each department READY
pchi-studio-api my-show.twin.pchi --send                # run the show on the copy that points at them
curl 127.0.0.1:3400/seen/grandma-1                      # what the lighting desk twin received
curl -X POST 127.0.0.1:3400/fault -d '{"device":"grandma-1","fault":"down"}'
```


**A pretend UR.** A device of type `ur` (Universal Robots, see [ur.md](ur.md)) gets a pretend Dashboard Server instead of a UDP socket: it answers the commands in UR's manual, writes down every command it was sent (`GET /seen/<id>` shows them, so you can prove what PCHI never sent), and takes the faults `protective-stop`, `local-control`, `refuse-play`, `down` and `healthy` (`POST /fault`). It is written from the manual, not from a real controller.

## What a twin does

Each UDP device in the rig (OSC tools, Art-Net, sACN) gets one twin: a socket on a free loopback port. `pchi-twin` writes a **copy** of the show with each device's `host` and `port` changed to its twin (and `listen_port` removed, so nothing else is bound); your show file is not touched. The twin:

- **records** every message: when it arrived, its OSC address and arguments (or, for DMX, the protocol and universe), and whether the device *acted on it*;
- **answers**, when healthy, if the tool is one that answers a probe (the X32 does), so the runtime's link state moves as it would with the tool;
- can be made to **fail**.

| Fault | What the twin does | What the runtime sees |
|---|---|---|
| `healthy` | acts on everything, answers probes | `sent`, link `linked` |
| `down` | the packet still arrives (UDP does not know) and is recorded as **not acted on**; no answers | `sent`, then the link goes `lost` for a tool that answers probes |
| `lag:500` | acts, 500 ms late | `sent` |

The `down` case is the point. A runtime that says "sent" and a desk that did nothing are indistinguishable over UDP; the twin lets you test what the *rest of the system* does about that: readiness, the doors check, the Live tab, an agent's report.

## Control API (`pchi-twin --control ADDR`, default `127.0.0.1:3400`)

| Route | |
|---|---|
| `GET /devices` | each device: type, department, fault, how many messages received and acted on; and which devices have no twin |
| `GET /seen/<id>` | everything that device received, oldest first |
| `POST /fault` `{"device","fault"}` | `healthy`, `down` or `lag:<ms>` |
| `POST /ready` `{"dept","ready","note"}` | report a department standing by or on hold to the runtime (needs `--runtime`) |
| `POST /clear` | forget what was received |

`--runtime HOST:PORT` reports every department READY at start, as the people at the desks would. Add `--token` if the runtime has one.

## In code

`pchi_studio_api::twin::Twin::start(&devices)` returns a twin; `twin.rig()` is the rig pointed at it; `set_fault`, `seen`, `acted`, `json` read and drive it. The benchmark ([bench.md](bench.md)) builds one for every scenario.

## What it does not do

- It does not know what a tool *does* with a message. It shows what PCHI put on the wire, and what happens around a device that goes quiet. It cannot show that a console reacts, and a profile that is wrong about a tool's real OSC dialect looks right to a twin. That is what `pchi-gearcheck` is for ([gear-check.md](gear-check.md)).
- OBS is reached over a websocket and has no twin (it is listed as skipped).
- A twin on loopback has no network: no delay, loss or jitter beyond what you add with `lag:`. It says nothing about the venue's switches or Wi-Fi.
- It does not stand in for the *runtime* either. Failover needs two runtimes (see [failover.md](failover.md)); the twin gives each of them devices to send to.
