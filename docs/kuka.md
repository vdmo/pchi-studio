# KUKA (step 4 of the robotics plan): what is built, what is not, and what I need

**There is no KUKA driver, and none can be honestly written yet.** (There is a Modbus TCP route for a PLC in front of a robot, below, tested only against a pretend PLC.) KUKA has no single documented command interface like Universal Robots' Dashboard Server: how an outside system starts a program is decided per site, by the controller, its options and the integrator. What exists is the general machinery any such device needs, an **example** of what a KUKA profile could look like, and the **contract** a KUKA receiver would have to meet. None of it has met a KUKA robot, a KUKA simulator or a KUKA controller.

## What is built

The text link ([link-profiles.md](link-profiles.md)) learned three things a KUKA EthernetKRL (EKI) device needs and a UR does not:

- **Messages that frame themselves.** EKI exchanges XML, not lines. A profile can set `request_end` (`""` for none, so nothing is appended to a request) and `reply_until` (for example `</Reply>`): the reply is read up to and including that text, however it arrives in pieces, with a 64 KB cap. It is tested against a pretend device that sends XML in two pieces with no line end at all, which would hang a reader that waits for a newline.
- **Several values from one reply.** A status read can `extract` many variables from the one reply (a pattern per variable), instead of one command per value.
- **A never-list that reads XML.** A forbidden command is caught whether it is written as text (`power on`) or as an element (`<Power>`, `<Brake release="1"/>`, `<RESET/>`): a profile that could send one is refused when it is loaded.

And an example profile, [tools/kuka/kuka-eki-example.json](../tools/kuka/kuka-eki-example.json), which a test loads so it cannot rot. It is **not a KUKA format**: it is PCHI's own contract (below), written out the way the profile format needs it.

## The contract a KUKA receiver has to meet

On a KUKA controller with **EthernetKRL** installed, a program written in KRL (on the robot, by whoever programs it) would listen for XML messages from PCHI and answer them. The messages (one XML element, no line end; replies end with `</Reply>`):

| PCHI sends | The receiver replies |
|---|---|
| `<Cmd><Run>OPEN_ARM</Run></Cmd>` | `<Reply><Ack>OK</Ack></Reply>`, or `<Reply><Nack>why</Nack></Reply>` |
| `<Cmd><Pause/></Cmd>`, `<Cmd><Stop/></Cmd>` | the same |
| `<Cmd><Status/></Cmd>` | `<Reply><Remote>true</Remote><Safety>OK</Safety><State>IDLE</State></Reply>` (state: IDLE, RUNNING, PAUSED, FAULT) |

What the receiver **must** guarantee. These are the robot programmer's to implement, review and own, not PCHI's:

1. **A fixed list of programs.** It runs only subprograms from a list written into the receiver. A name it does not know gets `Nack`. A message carries a **name and nothing else**: no positions, no speeds, no targets, ever.
2. **No authority over the robot's state.** It never changes the operating mode, acknowledges or resets a fault, powers drives, releases brakes, changes the override or bypasses a safety function because a network message said so. PCHI's profile lists these under `never` as a second lock, but the first lock is the receiver.
3. **The robot's own safety rules.** It starts a program only in the state and mode its owner decides (for example only in an external automatic mode, with the safety status OK). Otherwise `Nack` with the reason.
4. **It answers every request** within about a second, with a reason on every `Nack`.
5. **`Stop` is always accepted and never queued behind another request.** What a `Stop` does to a moving robot (a controlled stop, and where it comes to rest) is decided and documented by the programmer.
6. **What happens when the connection drops** is decided and documented: does a running program continue, pause or stop? PCHI cannot decide this and will not assume it.
7. **Bounded, one at a time**: one request is handled at a time, and an oversized or malformed message is refused, not guessed at.

PCHI adds, on its own side (all tested against a pretend device): a program must pass a strict name pattern **and** be on the device's own `programs` list or nothing is sent; each reply is checked and the first `Nack` stops the sequence; the robot's state is read about once a second, and a robot that is not ready holds cues and, if armed, is disarmed until a person arms it again; results are `accepted`, `running`, `refused` or `failed`, never "done".

**An example is not a design review.** The commands, element names and states above are a starting point to agree with the robot's programmer, who may change them all, and the example profile with them.

## The PLC route (built, against a pretend PLC)

The conventional industrial route: a **PLC** sits between the show system and the robot, and its program starts, stops and selects the robot's program (for a KUKA, through its Automatic External inputs, which the PLC drives). PCHI talks to the **PLC** over **Modbus TCP**, a published standard, implemented from the specification (`modbus.rs`): read coils, discrete inputs, holding and input registers, and write a single coil or register, nothing more. PCHI never talks to the robot, and never needs to know what the PLC does with a write.

A profile ([tools/kuka/kuka-plc-example.json](../tools/kuka/kuka-plc-example.json), `"transport": "modbus-tcp"`) writes each step as a small readable line (`write_reg 100 {n}`, `write_coil 10 1`, `read_ireg 200`), so the same machinery as the UR and the XML device applies: closed set of commands, checked replies, state read about once a second, faults hold cues and disarm, results `accepted` / `running` / `refused` / `failed`. What is new for a PLC:

- **Programs by number.** The device's allow-list gives each name its number: `programs: "OPEN_ARM=3, PARK=5"`. A cue chooses a name; the profile's `{n}` becomes its number; a name with no number is refused before anything is sent.
- **Forbidden addresses.** `never_addresses` (`"coil:30-39"`, `"reg:100"`) are addresses the profile promises never to write: a profile with a step that writes one is **refused at load**, and every write is checked again at the moment it is sent. In the example, coils 30 to 39 stand for the machine's drives and fault acknowledgement.
- **A start is a pulse.** The number is written, the start coil is raised (and held for `wait_ms` so the PLC sees it), then lowered. If anything fails after a write was attempted, the action's `cleanup` runs, so a start coil is not left raised. If nothing was written (the PLC said no at the first check) **nothing is written to put back**.
- **Reads decide first.** The example reads "accepting commands" and "safety OK" and "idle" before it writes anything, and stops at the first no.

### The contract a PLC programmer would implement

The example's register map is **invented for the tests**, in the spirit of what an integrator might write. The real one is the PLC programmer's, and the profile is edited to match it:

| address | what (example) |
|---|---|
| holding register 100 | the program number to start |
| coil 10 | START: a rising edge starts that program |
| coil 11 | STOP |
| coils 30 to 39 | RESERVED: drives and fault acknowledgement. **Never written by PCHI** |
| discrete input 20 | accepting commands from outside |
| discrete input 21 | safety status OK |
| input register 200 | state: 0 idle, 1 running, 2 fault |

What the PLC program must guarantee (the integrator's to implement and own, as for the XML receiver above): it starts only numbers on a fixed list; it never lets PCHI change mode, acknowledge a fault, power drives or override safety (the reserved coils must not be wired to anything PCHI can write); it starts a program only when the robot's own conditions hold; Stop is always honoured; and what happens if the PCHI connection drops is decided and documented.

**Check the profile against your PLC with `pchi-linkcheck`:** `pchi-linkcheck kuka-plc-example --profiles tools/kuka --host <plc> --out readings.json` reads the status addresses (read-only) and says whether the PLC answers in the form the profile expects.

## Routes that are not built

- **OPC UA to a PLC**, for sites that use it instead of Modbus: not built.
- **KUKA.OPC UA.** Reads and writes predefined data; I could not confirm it can start programs. Possibly useful for reading state.
- **RSI** (real-time motion streaming): out of scope, PCHI does not stream motion.
- **An integrator's own show software** (Motorized Precision, Event Robotics and similar): if you work with one, the right integration is probably with their software, not the robot. I know nothing of their interfaces.

## What I need from you to go further

1. The controller: **KRC4 or KRC5**, and the **KSS version**.
2. Which **options are installed**: EthernetKRL (EKI)? OPC UA? A fieldbus (PROFINET or EtherNet/IP)?
3. Is there a **PLC** between the show system and the robot, and **which one**? Does it speak **Modbus TCP**? (Many do; some need an option or a gateway. A PLC that only does PROFINET or EtherNet/IP is not reached by this route.)
4. **Who programs the robot and the PLC**, and would they write the PLC program (or the XML receiver) to a contract like the ones above? It is code on a machine that can hurt people; it must be theirs.
5. Is there a **simulator** (KUKA OfficeLite is a lead from one blog, unverified, and needs a licence)?

With answers 3 and 4 the PLC route becomes concrete: the real register map goes into the profile, `pchi-linkcheck` reads the real PLC, and the gear check follows. Without them, the examples stay examples.

## What is not proven

Everything about KUKA, and every real PLC. The pretend PLC (`plcsim.rs`) has a register map I invented and answers as the Modbus specification says: it proves PCHI's Modbus framing, its checks, its cleanup and its refusal to write forbidden addresses, not that any real PLC or robot behaves like it. Modbus TCP is implemented from the standard but has not met a real device: gateways differ in unit ids, timing and how they treat unusual requests. The EKI behaviour I describe is from third-party blogs that disagree on ports and details; KUKA's own documentation is behind its customer portal and I have not read it. The pretend XML device in the tests proves PCHI's framing and checks, not that a KUKA controller behaves that way.
