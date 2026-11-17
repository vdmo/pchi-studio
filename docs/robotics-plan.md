# Robotics and motion: how PCHI could control them (a plan, not a feature)

**Focus: KUKA and Universal Robots, see [robotics-kuka-ur.md](robotics-kuka-ur.md).**

Nothing here is built. This is what the landscape looks like, what PCHI would do and not do, and an order to build it in. It comes from web research on one day (2026-11-17), so every vendor claim carries how sure it is, and what must be checked on the vendor's own documentation or on real gear before relying on it.

## The one rule: PCHI asks, the robot's own safety system decides

A robot or a motorised rig can hurt people in ways a light or a screen cannot. So:

- **PCHI sends high-level, bounded requests**: run this named program, go to this stored position, start this move, pause, stop. It never streams joint positions in real time and never replaces the robot's own safety.
- **Safety stays in the safety system**: the robot's emergency stop, safety-rated zones and speed limits, and for flown people or loads the rigging and automation system's own safety chain. A PCHI "stop" is a convenience and a record. It is **not an emergency stop** and the docs and the Studio must never call it one.
- **Standards apply to the whole installation, not to PCHI**: for performer flying and powered hoists, the entertainment-technology standards (ANSI E1.43, performer flying systems, edition 2025 per the [ANSI webstore](https://webstore.ansi.org/standards/esta/ansie1432025); the E1.6 powered-hoist family, whose control-system part I could not confirm is published), and for industrial arms the robot standards. Whoever certifies the installation decides what PCHI may be connected to. Treat this document as a starting point for that conversation, not as safety advice.
- **Same pattern as lasers**: PCHI already holds cues to a disarmed laser until someone with the right role arms it, lets anyone disarm, and drops back to disarmed after a restart ([safety-drill.md](safety-drill.md)). Robotics should work the same way: disarmed by default, armed by a person on site with a specific role, held cues while disarmed, and "stop" never locked behind a role.

## What people use (and how sure I am about each)

| System | What it is | How software talks to it | Sure? |
|---|---|---|---|
| **Universal Robots** (cobots) | the common collaborative arm | TCP: a **Dashboard server** (port 29999: load, play, pause, stop programs, read state; plain text), script ports 30001-30003, **RTDE** (30004, fast data in and out); official client library and ROS 2 driver. The driver needs the **External Control URCap** running on the robot | Good: UR's [client interfaces](https://www.universal-robots.com/articles/ur/overview-of-client-interfaces) and [driver docs](https://docs.universal-robots.com/Universal_Robots_ROS2_Documentation/). Dashboard password from Polyscope 5.13; the primary port is unauthenticated |
| **KUKA** | the common show arm (KR C4 controllers, for example at [Europalco](https://roboticsandautomationnews.com/2023/03/29/europalco-buys-two-robots-from-kuka-to-add-wow-factor-to-shows/66293); live-tour integrations go through partners such as Event Robotics) | RSI and FRI (ROS 2 drivers are community maintained, [ros2_control list](https://control.ros.org/iron/doc/supported_robots/supported_robots.html)); a third-party blog describes a "KUKA.EntertainTech" package synchronising with cues over UDP | **Low**: the EntertainTech claim is from a third-party blog and I could not confirm it on KUKA's site. Needs KUKA documentation |
| **ABB** | industrial arms; **RoboScreen** (an arm moving an LED screen) | EGM (community ROS 2 support); andyRobot's software drives ABB arms for screen and animatronic shows | Medium: from [andyRobot coverage](https://lesterbanks.com/?p=36963) and the ros2_control list; no ABB protocol documentation read |
| **MRMC** (camera and motion-control robots) | broadcast and film robots; Flair software | Flair has serial and Ethernet channels and supports **FreeD and OSC**; it can trigger a lighting desk over OSC; the Bolt head has an API | Medium: from MRMC's own product pages as summarised by search ([Flair](https://www.mrmoco.com/motion-control/flair)); **no API document read**, no ports or message format known |
| **Kinesys (Vector, K2) and TAIT Navigator** | automated hoists and staging (Kinesys is part of TAIT since 2019) | Disguise has a **Kinesys driver** that reads axis data from K2 or Vector ([disguise docs](https://help.disguise.one/designer/devices/position-drivers/kinesys-driver)); I could **not** confirm that Vector, K2 or Navigator speak OSC | **Low** for control. Needs the vendors' integration documents. Likely a position *feed out* before it is a control *in* |
| **Stage Technologies, others** | stage automation consoles | not researched | Unknown |
| **ROS 2 + rosbridge** | the open robotics middleware; most research, AMR and many custom rigs | **rosbridge**: JSON messages over a WebSocket (or TCP), each with an `op` (publish, subscribe, call_service). Official protocol document in the [rosbridge_suite repo](https://github.com/robotwebtools/rosbridge_suite). It has **no authentication by default** | Good for the protocol; the safety of a given rig is entirely its own |
| **Beckhoff TwinCAT (PLC)** | the PLC under many animatronic and ride systems | ADS, OPC UA (incl. Pub/Sub), MQTT bridges ([ADS over MQTT](https://download.beckhoff.com/download/document/automation/twincat3/ADS-over-MQTT_ZH.pdf)) | Good for the protocols; I found nothing entertainment-specific |
| **Tracking feeds** (PosiStageNet, FreeD, BlackTrax) | where things *are*, for lighting and media to follow | UDP | Not researched here beyond FreeD above |

The honest finding: **there is no common live-events robot protocol.** Each system has its own interface, mostly vendor software, integrators or general-purpose links (UDP, OSC, a robot's own TCP commands, ROS). Where OSC exists (MRMC, possibly others) PCHI can already speak it.

## The shortlist: what is most likely to matter first

**Popularity is thinly evidenced.** I found no market-share ranking for live-events robotics (the free search results give none), and the cobot market reports name the same vendors (Universal Robots, FANUC, ABB, Doosan, KUKA, Techman, AUBO) without free per-vendor numbers (e.g. [MarketsandMarkets](https://www.marketsandmarkets.com/ResearchInsight/collaborative-robot-market.asp)). So this ranking is a judgement from the evidence below plus how cheap and safe each is to build. Argue with it.

**What the evidence does say, for live shows**

- Show arms are mostly **KUKA**: six KUKA arms carrying lasers at Riyadh Season 2024 with Motorized Precision ([case study](https://www.motorizedprecision.com/case-studies-riyadh24)), Europalco's KR210 arms. But they are run through an **integrator's software**, not by showing a robot a PCHI message.
- Touring kinetic gear is **Kinesys / TAIT** (for example 3G Productions' [Kinesys purchase](https://lsionline.com/news/3g-productions-invests-in-kinesys-automation)), and **Beckhoff** PC-based control sits under many tour systems ([Beckhoff show industry](https://www.beckhoff.com/en-en/industries/entertainment-industry/show-industry/)).
- Broadcast and virtual production use **tracking and camera-robot** feeds: FreeD is an open, long-standing protocol; **OpenTrackIO** is a newer SMPTE protocol that Unreal Engine has supported since 5.6 ([Sony](https://pro.sony/en_AE/technology/opentrackio)).
- Disguise's RenderStream is a render-engine link, not a robot interface; I found no ready-made robot plugin for Disguise, Notch or TouchDesigner ([RenderStream](https://help.disguise.one/Content/Configuring/RenderStream/RenderStream.htm)).

**The pattern that matters:** in live events the thing you control is usually the **control layer in front of the machine** (a Kinesys Vector or TAIT Navigator console, an integrator's motion software such as MRMC's Flair, a Beckhoff PLC), the same way PCHI talks to a grandMA3 or an Eos console and not to each light. PCHI should aim at those control layers first and at bare robot controllers second.

| # | Target | Why it is here | Main risk or unknown |
|---|---|---|---|
| 1 | **MRMC Flair (OSC)** | Documented OSC and FreeD; PCHI already speaks OSC, so it may need only a profile | I have no address list or API document; needs MRMC's documentation and a unit or their demo |
| 2 | **ROS 2 via rosbridge** | One integration covers a very wide range of robots, including custom builds; free simulators to test against | Per-rig safety is entirely the rig's; rosbridge has no authentication by default |
| 3 | **Universal Robots** | Best-documented interface (TCP dashboard, RTDE, official libraries), named a market leader in the cobot reports, URSim to test without a robot | Less common than KUKA on show stages; cobots are small |
| 4 | **Kinesys Vector / TAIT Navigator** (via Beckhoff where that is what is there) | The touring kinetic standard; position feed out may come before control in | I could not confirm any control protocol: needs vendor integration documents, and flown loads are the highest-risk case |
| 5 | **KUKA arms, through the integrator's layer** (Motorized Precision, Event Robotics) | The most visible show arm | Direct control is vendor-specific (RSI, EKI) and integrator-led; likely out of reach without a partner |
| 6 | **Tracking feeds in** (FreeD, OpenTrackIO, PosiStageNet) | Lets lighting and media follow a moving thing; low risk because it only listens | Which one first depends on what you have |
| 7 | **ABB, FANUC, Doosan** | Large in industry (ABB sockets and EGM; RoboScreen-style shows) | Little show-world pull; build when someone asks |
| – | **Drones, humanoids, walking robots** | Not now | Different regulation and control models |

## What PCHI already has, and what is missing

Already there: OSC out through data-driven **profiles** (no code to add a tool that takes OSC: [profiles.md](profiles.md)); a direct WebSocket client (OBS); devices with a host and port; per-department roles, arming, held cues and a safe drill; a **twin** that stands in for gear and can fail ([twin.md](twin.md)); a record and receipts of every decision.

Missing:

- **A department for motion.** Departments are a fixed list (visual, lighting, laser, audio) in the Rust runtime, the SDK and Studio. Robotics needs its own (working name `motion`) with arming like lasers, its own role (a motion safety person, like the `lso`), and its own line in the drill.
- **A TCP or WebSocket device kind that is not OSC**, for a robot's own command language.
- **Feedback in**: positions, "program running", fault states, and a way for a robot to say it is not ready. The OBS script already reports READY per department; a robot needs the same, and a *fault* must pull the show to a held state.
- **Honest delivery state**: UDP says "sent". A robot should report "accepted" or "running", and the show must not assume.

## Order of work

Each step ends with something that can be tested without hardware first, then on the real thing, and says what it did not prove (the rule of the rest of these docs).

1. **A robot twin (no hardware).** Extend `pchi-twin` with a pretend arm and a pretend hoist that accept commands, report state, take a "stop", and can be made to fault, lag or vanish. Everything after is developed against it. *Done when* the bench and drill can run a robotics show against the twin.
2. **The `motion` department and its safety behaviour.** Disarmed by default, held cues, arm by a named role, a STOP that goes out first and is never gated, drops to disarmed on a restart, appears in the drill ("a fault in the arm holds the show"). This is the part that matters most, and it is independent of any one robot. *Done when* the drill covers it, on the twin.
3. **OSC robots through profiles (cheapest first real gear; shortlist #1).** A profile for anything that takes OSC, starting with **MRMC Flair** if you have access: needs the actual address list from MRMC. Zero new transport code.
4. **A Universal Robots device.** Dashboard server over TCP: load a named program, play, pause, stop, read state, with the dashboard password. It is the best documented robot interface and cobots are common. Needs the External Control or a program on the robot, and a UR to test on (or UR's own simulator, URSim).
5. **A ROS 2 device through rosbridge.** Publish a command to a named topic, call a named service, subscribe to a state topic. Covers a wide range of custom and research robots with one integration. Needs a rosbridge running on the robot side, and **authentication in front of it** (it has none by default).
6. **Feeds and the big systems.** Read positions (FreeD, PosiStageNet) so lighting and media follow a moving thing; investigate Kinesys, TAIT Navigator and Stage Technologies once the vendors' integration documents are in hand. KUKA and ABB through an integrator's software or their own interfaces, only with the vendor and an integrator involved.
7. **Gear day for motion**: `pchi-gearcheck` steps for each robot (asks for "AREA CLEAR", sends the exact message, listens, tests the stop), added to [real-gear-day.md](real-gear-day.md).

## What I need from you

- **Which robots or rigs do you actually have, or expect to?** Brands and models decide the order; the table above is a survey, not your list.
- **Who certifies the installation** (rigger, automation vendor, integrator)? Their rules decide what PCHI may do. People in flown or loaded-over-people applications need that conversation before anything past step 2.
- **Which job first**: arms moving screens or props, camera robots, kinetic hoists, animatronics, or drones? Drones are a different world (flight controllers, regulations) and not covered here.
- **Access to a real unit** or its simulator (URSim for UR, a ROS 2 simulator such as Gazebo) so steps 4 and 5 can run against something.

## What this research did not cover

I did not find KUKA's own documentation on show synchronisation, ABB's protocols first-hand, MRMC's API document, any OSC or protocol statement from Kinesys or TAIT, or Stage Technologies. Where a row says Low or Unknown, that is where the next reading goes. Standards edition numbers and publication status should be checked in the ESTA and ISO catalogues before they are quoted in a safety case.
