# KUKA (step 4 of the robotics plan): what is built, what is not, and what I need

**There is no KUKA driver, and none can be honestly written yet.** KUKA has no single documented command interface like Universal Robots' Dashboard Server: how an outside system starts a program is decided per site, by the controller, its options and the integrator. What exists is the general machinery any such device needs, an **example** of what a KUKA profile could look like, and the **contract** a KUKA receiver would have to meet. None of it has met a KUKA robot, a KUKA simulator or a KUKA controller.

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

## Routes that are not built

- **Automatic External through a PLC.** The conventional industrial route: a PLC starts, stops and selects a program by number over a fieldbus. PCHI would need a **Modbus TCP or OPC UA client** to talk to the PLC. Not built; it depends on the site's PLC and I/O mapping, which are the integrator's.
- **KUKA.OPC UA.** Reads and writes predefined data; I could not confirm it can start programs. Possibly useful for reading state.
- **RSI** (real-time motion streaming): out of scope, PCHI does not stream motion.
- **An integrator's own show software** (Motorized Precision, Event Robotics and similar): if you work with one, the right integration is probably with their software, not the robot. I know nothing of their interfaces.

## What I need from you to go further

1. The controller: **KRC4 or KRC5**, and the **KSS version**.
2. Which **options are installed**: EthernetKRL (EKI)? OPC UA? A fieldbus (PROFINET or EtherNet/IP)?
3. Is there a **PLC** between the show system and the robot?
4. **Who programs the robot**, and would they write the receiver? A receiver is KRL on a machine that can hurt people; it must be theirs.
5. Is there a **simulator** (KUKA OfficeLite is a lead from one blog, unverified, and needs a licence)?

With answers 1 to 4 the next step is concrete: either hand the robot's programmer the contract above and adapt the example to what they build, or build the PLC route. Without them, further KUKA code would be a guess.

## What is not proven

Everything about KUKA. The EKI behaviour I describe is from third-party blogs that disagree on ports and details; KUKA's own documentation is behind its customer portal and I have not read it. The pretend XML device in the tests proves PCHI's framing and checks, not that a KUKA controller behaves that way.
