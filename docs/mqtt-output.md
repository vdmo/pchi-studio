# MQTT output: publish one message from a closed list of topics

A rig device can answer a cue by **publishing one MQTT message** to a broker, for installations, sensors and building systems that listen on MQTT. The topics come from a profile file (`"transport": "mqtt"`), the same idea as [HTTP output](http-output.md) and [JSON-RPC output](jsonrpc-output.md): the profile fixes the topic and the payload shape; a cue supplies only a **value**.

There is **no built-in MQTT profile**: topics belong to the installation, so you write one.

## What a cue can and cannot do

- The value must match the action's pattern. A part of it that goes into a **topic** must be letters, digits, `_`, `.` or `-`, up to 40 characters, so it cannot add a topic level (`/`) or a wildcard (`+`, `#`). A part that goes into the **payload** is escaped for the payload's `format` (`text`, or `json`).
- A cue cannot choose the broker, the topic, the QoS or the retain flag.
- Topics in a profile are letters, digits and `/ _ . -`, no wildcards, not starting with `$` or `/`. Laser and motion profiles are refused.

## What it does not do

- **Publish only.** It never subscribes, so nothing is read from the broker.
- **No username, password or TLS.** A broker that needs either refuses the connection and the answer says "not authorised". Many installation brokers on a closed network accept anonymous clients; check yours.
- One connection per cue (connect, publish, disconnect). Protocol level 3.1.1, clean session, QoS 0 or 1.
- **QoS 0 is `sent`:** the message was written to the broker's socket and no reply exists. **QoS 1 is `accepted`** once the broker's acknowledgement arrives. In both cases that is the *broker* having it, not anything that listens having acted on it.
- A retained message stays on the broker until someone clears it. Use `retain` deliberately.

## Writing a profile

A file in the folder given to `--profiles`:

```json
{
  "id": "hall",
  "name": "Hall controller",
  "transport": "mqtt",
  "dept": "lighting",
  "port": 1883,
  "kinds": ["lighting.scene", "lighting.blackout"],
  "hint": "a scene name, letters and digits",
  "notes": "From the installer's topic list, rev 3.",
  "actions": {
    "lighting.scene": {
      "value": "([A-Za-z0-9_-]{1,30})",
      "topic": "hall/lights/scene",
      "payload": "{{\"scene\":\"{1}\"}}",
      "format": "json",
      "qos": 1,
      "basis": "vendor",
      "source": "installer's topic list, rev 3"
    },
    "lighting.blackout": {
      "topic": "hall/lights/blackout",
      "payload": "1",
      "retain": true,
      "basis": "vendor",
      "source": "installer's topic list, rev 3"
    }
  }
}
```

- `{0}` is the whole value; `{1}` to `{9}` are the bracketed parts of the value's pattern. A literal brace is written twice (`{{`), as in the JSON payload above. Topics contain no braces except placeholders.
- `basis` is `vendor`, `community` or `memory` with a `source`, never `proven`.
- Then add a device of type **OSC** (or any) with the parameter `profile` set to your id and its `host` set. The port defaults to the profile's, then 1883. The cue kinds are not yet in the Cue sheet's menus.

The Doors check refuses a cue whose value the profile does not accept, and a dry run shows the topic, QoS and payload that would be published.

## What is tested

The profile rules, the exact CONNECT and PUBLISH bytes (the length encoding, QoS 0 and 1, retain), that no value can change a topic, JSON escaping, and a **stand-in broker** that receives CONNECT, PUBLISH and DISCONNECT, acknowledges QoS 1, refuses a connection, is not a broker, or is not there. **Not tested: a real broker** (Mosquitto, EMQX and the like), a broker that requires login, or any device listening on a topic.
