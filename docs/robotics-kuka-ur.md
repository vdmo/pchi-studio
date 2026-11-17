# KUKA and Universal Robots: the design

Follows [robotics-plan.md](robotics-plan.md). **Status: step 1 is built** (the `motion` department, arming, the `mso` role, STOP MOTION, drill and gear-check lines, a pretend device: see [motion.md](motion.md)). **Step 2 (the UR driver, as a data-driven text link) is built, against a pretend UR only: see [ur.md](ur.md) and [link-profiles.md](link-profiles.md). The KUKA driver is not.** It says what PCHI would send to each, what it must never send, how a fault is handled, and what has to be decided or checked first. Facts about Universal Robots come from UR's own manual (version 5.25 of the Dashboard Server page, [here](https://www.universal-robots.com/manuals/EN/HTML/SW5_25/Content/prod-dashboard/Dashboard_table.htm)). Facts about KUKA are **weaker**: KUKA's documentation is behind its customer portal, and what I could reach is third-party blogs that disagree with each other. Those are marked.

## What PCHI is, and is not, in this

A cue says **"run the program called X"**. The robot's own programmer (UR program, KUKA KRL) owns every path, speed, zone and limit, and the robot's own safety system (emergency stop, safety functions, safeguards, safety-rated zones) owns safety. PCHI starts, pauses and stops *named programs that someone qualified already wrote and validated*. It does not send positions, does not stream motion, does not power the robot on, does not clear a safety stop. If a rig needs those, it is the wrong rig for PCHI.

## Universal Robots

### The interface (documented)

The **Dashboard Server**, TCP port 29999, plain text, one command per line ending in a newline. It only accepts program control in **Remote Control** mode. The commands PCHI would use, with UR's exact replies:

| PCHI action | Sends | Success reply | Failure reply |
|---|---|---|---|
| check it is usable | `is in remote control` | `true` | `false` |
| what is loaded | `get loaded program` | `Loaded program: <path>` or `No program loaded` | |
| load a program | `load <name>.urp` | `Loading program: <name>.urp` | `File not found: …` / `Error while loading program: …` |
| start | `play` | `Starting program` | `Failed to execute: play` |
| hold | `pause` | `Pausing program` | `Failed to execute: pause` |
| stop | `stop` | `Stopped` | `Failed to execute: stop` |
| read state (about 2 Hz) | `programState` | `STOPPED`, `PLAYING` or `PAUSED` | |
| | `robotmode` | `Robotmode: RUNNING` (also IDLE, POWER_OFF, BOOTING, …) | |
| | `safetystatus` | `Safetystatus: NORMAL` (otherwise a fault) | |

### What PCHI must refuse to send, even if someone asks

`power on`, `power off`, `brake release`, `unlock protective stop`, `close safety popup`, `restart safety`, `load installation`, `shutdown`, `set operational mode`. Each of those is a decision about the physical state of the machine and the people near it. PCHI's UR device has an **allow-list** of the commands above and nothing else; there is no "send raw command" path from a cue, an agent or the command bar. (A person on site does those from the pendant.)

### A cue, step by step

`motion.run` with value `OPENING_ARM` to a UR device means: refuse if the device is **disarmed** (held, like a laser); refuse if the program name is not in the device's **allow-list of program names** (a typo or an agent cannot start an arbitrary file); check `is in remote control` is `true` and `safetystatus` is `NORMAL`; `load` it if it is not the loaded program and check the reply; `play`; then confirm by reading `programState` until `PLAYING`. The delivery record says what really happened: **sent**, then **accepted** (the robot answered `Starting program`), then **running** (state read back), or **refused** with the robot's own words. It never says "done" because the robot started something: it says what it saw.

### Ready, and a fault

The motion department reports READY when: remote control is on, safety status is `NORMAL`, robot mode is `RUNNING`, and the cue's program is available. Anything else (a protective stop, a lost connection, the robot not answering for a few seconds, the mode changing) makes it **not ready and pulls the show to a held state**: no further motion cues go out, the operator sees why in the robot's own words, and PCHI does **not** try to clear the fault or restart the program. Someone on site inspects and recovers it.

### Testing it without a robot

UR publishes a free simulator, **URSim** (a Docker image, `universalrobots/ursim_e-series`, and a Linux download; the download needs a free account). Its Dashboard Server should answer as above, but I have **not confirmed that the official image publishes port 29999** (only a third-party image shows that), so that is the first thing to check. The twin ([twin.md](twin.md)) gets a pretend UR that speaks exactly the table above and can fail, so development does not wait for that.

### What I have not verified for UR

Real replies from a real controller; behaviour on CB-series (the older generation has a separate Dashboard document); how `load` behaves when the installation needs safety confirmation (the manual says it fails); the right pace for polling without disturbing the robot.

## KUKA

There is **no single documented, simple command interface** like UR's Dashboard. The options, strongest first for PCHI's job (start and stop named programs):

| Route | What it is | Confidence | Notes |
|---|---|---|---|
| **Automatic External** over a fieldbus, through a PLC | The controller's standard way for a supervisory PLC to start, stop, reset and choose a program, mapped as I/O in WorkVisual | Medium (a third-party article for KRC5, KSS 8.6/8.7) | Needs a PLC or a fieldbus gateway between PCHI and the robot. Program chosen by a **number**. The most conventional and most acceptable to an integrator |
| **EthernetKRL (EKI)** | The robot opens a TCP or UDP link and exchanges XML with an outside system. A **KRL program on the robot** reads the messages and calls named subprograms | Low-medium (blogs only; they disagree on default ports, and warn that safety functions and mode changes are blocked by design) | The XML format is **defined per site** in the robot's own EKI configuration, so PCHI cannot hard-code it. It needs a KRL "receiver" written and validated by the robot programmer |
| **KUKA.OPC UA** (an option) | Makes the controller an OPC UA server for reading and writing predefined data | Low: KUKA's own page lists reading and writing data, and I could not confirm starting programs | Good for **state in** (positions, modes). Probably not for starting programs |
| **RSI** | Real-time sensor and motion streaming | n/a | **Out of scope**: PCHI does not stream motion |
| **The integrator's own layer** (Motorized Precision, Event Robotics, andyRobot and similar) | The show software that drives the arms on the stages I found | Medium that it is how shows work | Needs their API. May be the easiest real route if you work with one |

### How PCHI would reach KUKA

Both realistic routes need a **TCP or fieldbus client that is not OSC**, so the same piece of work serves UR and KUKA:

- **A data-driven TCP device**, in the way OSC tools use profile files ([profiles.md](profiles.md)): a profile says what text or XML to send for each action, what reply means accepted or refused, and which commands are allowed, so a UR profile and a KUKA-EKI profile are both just files, and a site's EKI XML is one file written once by whoever wrote the KRL receiver.
- **Modbus TCP or OPC UA client** for the PLC route (Automatic External), when that is what the site has.

### What I need to decide the KUKA route

1. The controller: **KRC4 or KRC5**, and the **KSS version**.
2. Which options are installed: **EthernetKRL** (EKI)? **OPC UA**? A fieldbus (PROFINET, EtherNet/IP)?
3. **Is there a PLC** between the show system and the robot?
4. **Who programs the robot** (you, an integrator, KUKA)? They write the program, and the KRL receiver if EKI is used.
5. A **simulator**: KUKA OfficeLite is the virtual controller I have a lead on (from one blog, unverified); it needs a licence.

## The same safety behaviour for both: a `motion` department

Both devices belong to a new **`motion`** department that behaves like lasers do ([safety-drill.md](safety-drill.md)):

- **Disarmed** whenever the runtime starts, restarts, loses a witness lease or is fenced by a takeover. A cue to a disarmed motion device is **held**, not sent.
- **Arming** needs a role: the operator, or a new **motion safety** role (as the `lso` is for lasers). Arming is a person's decision on site; remote people never arm ([remote-setup.md](remote-setup.md)).
- **STOP** (hold or stop every motion device) is never gated by a role, needs no key, works for agents too, and **always also disarms**. It is shown in Studio as "stop programs", never as an emergency stop: *it is a software request that depends on the network and the robot being alive*.
- The **drill** gets its own lines: cues held while disarmed, a fault holds the show, restart comes back disarmed, an unreachable robot is reported not ready, a refused command is shown in the robot's words.
- **The cost:** the runtime hard-codes "laser" as the one armable department in about 300 places across 23 files, plus the SDK and Studio. Doing this properly means making "armable department" a property and proving, with tests, that lasers behave exactly as they do now. That touches the safety path of the existing system, so it is done first, alone, with the regression tests written before the change.

## Order of work for these two

1. **Make arming generic** (laser behaviour unchanged, proven by the existing tests plus new ones), add the `motion` department, role and STOP. Against a **pretend device** only.
2. **The data-driven TCP device** (profile, allow-list, expected replies, status polling, link state, honest delivery states), with the UR profile, against the **twin's pretend UR**.
3. **Point it at URSim** and then a real UR (first the gear-check steps: `AREA CLEAR`, the exact message, the reply, a stop test).
4. **KUKA**, once the five questions above are answered: the same TCP device with an EKI profile, or a PLC route.
5. **Drill, gear check and a gear-day chapter** for motion.

## What this does not do, and what I cannot promise

- It does not make a robot safe, and it does not replace a risk assessment or a safety-rated system.
- A PCHI stop is **not** an emergency stop.
- UR is documented well enough to build against; **KUKA is not, until I know your controller and who programs it.** Anything more specific about KUKA would be a guess.
- Nothing here has been run against a real robot, a simulator or a twin yet.
