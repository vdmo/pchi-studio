//! MQTT publish: a cue becomes one message published to a broker, for installations, sensors and building systems that listen on MQTT.
//!
//! The topics come from a **closed list in a profile file** (`"transport": "mqtt"`): for each cue kind a topic template, an optional
//! payload template, a QoS (0 or 1) and a retain flag. A cue supplies only a value, checked against the action's pattern. Any part of
//! the value that goes into a **topic** must be letters, digits, `_`, `.` or `-` (so it cannot add a topic level or a wildcard); in a
//! payload it is escaped for the payload's format. **A cue cannot choose the broker, the topic, QoS or retain.**
//!
//! Built from MQTT 3.1.1 packets by hand (no library): CONNECT, wait for CONNACK, one PUBLISH, wait for PUBACK when QoS is 1, DISCONNECT.
//!
//! - Publish only. No subscribing, so nothing here reads from a broker, and no `+` or `#` can be sent in a topic.
//! - **No username, password or TLS.** A broker that needs either refuses the connection, and the answer says so.
//! - One connection per cue. QoS 0 is `sent` (handed to the broker's socket, no reply exists). QoS 1 is `accepted` when the broker's
//!   PUBACK arrives. Either way that is the **broker** receiving it, not any device acting on it.
//! - Laser and motion profiles are refused, as for HTTP and JSON-RPC.

use crate::httpout::{placeholders, render, Enc};
use crate::rig::Device;
use crate::textlink::{LinkError, Outcome};
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONNECT: Duration = Duration::from_secs(2);
const REPLY: Duration = Duration::from_secs(3);
const VALUE_MAX: usize = 64;
const TOPIC_MAX: usize = 200;
const PAYLOAD_MAX: usize = 500;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    value: Option<String>,
    topic: String,
    #[serde(default)]
    payload: String,
    /// `text` (default) or `json`: how a value is escaped into the payload.
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    qos: u8,
    #[serde(default)]
    retain: bool,
    #[serde(default = "memory")]
    basis: String,
    #[serde(default)]
    source: String,
}

fn memory() -> String {
    "memory".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    id: String,
    name: String,
    #[allow(dead_code)]
    transport: String,
    dept: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    notes: String,
    kinds: Vec<String>,
    actions: HashMap<String, RawAction>,
}

#[derive(Debug)]
pub struct MqttAction {
    pub value: Option<Regex>,
    pub topic: String,
    pub payload: String,
    pub json: bool,
    pub qos: u8,
    pub retain: bool,
    pub basis: String,
    pub source: String,
}

#[derive(Debug)]
pub struct MqttProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub port: Option<u16>,
    pub hint: String,
    pub notes: String,
    pub kinds: Vec<String>,
    pub actions: HashMap<String, MqttAction>,
    pub built_in: bool,
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

fn topic_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "/_.-".contains(c)
}

impl MqttProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<MqttProfile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a valid MQTT profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("profile id '{id}' must be letters, digits, - or _ (up to 40)"));
        }
        if !matches!(raw.dept.as_str(), "visual" | "lighting" | "audio") {
            return Err(format!("{id}: dept '{}' is not one of visual, lighting, audio (a laser or motion device needs an acknowledgement and a stop, which a published message does not give)", raw.dept));
        }
        if raw.actions.is_empty() {
            return Err(format!("{id}: no actions"));
        }
        let mut actions = HashMap::new();
        for (kind, a) in raw.actions {
            let what = format!("{id} {kind}");
            let topic_used = placeholders(&a.topic).map_err(|e| format!("{what}: topic {e}"))?;
            // the literal text of the topic: placeholders and doubled braces removed
            let literal: String = a.topic.replace("{{", "").replace("}}", "");
            let mut lit = String::new();
            let mut rest = literal.as_str();
            while let Some(i) = rest.find('{') {
                lit.push_str(&rest[..i]);
                rest = &rest[i + 3..];
            }
            lit.push_str(rest);
            if a.topic.contains("{{") || a.topic.contains("}}") {
                return Err(format!("{what}: a topic has no braces"));
            }
            if lit.is_empty() || a.topic.len() > TOPIC_MAX || !lit.chars().all(topic_char) {
                return Err(format!("{what}: a topic is letters, digits and / _ . - only (no + or # wildcards, no spaces), plus placeholders"));
            }
            if a.topic.starts_with('$') || a.topic.starts_with('/') || a.topic.ends_with('/') || a.topic.contains("//") {
                return Err(format!("{what}: a topic does not start with $ or /, end with /, or contain //"));
            }
            if a.qos > 1 {
                return Err(format!("{what}: qos is 0 or 1"));
            }
            let json = match a.format.as_deref() {
                None | Some("text") => false,
                Some("json") => true,
                Some(f) => return Err(format!("{what}: format '{f}' is text or json")),
            };
            if a.payload.len() > PAYLOAD_MAX || a.payload.chars().any(|c| c.is_control() && c != '\n') {
                return Err(format!("{what}: a payload is up to {PAYLOAD_MAX} plain characters"));
            }
            let mut used = topic_used;
            used.extend(placeholders(&a.payload).map_err(|e| format!("{what}: payload {e}"))?);
            let value = a.value.as_deref().map(full).transpose().map_err(|e| format!("{what}: {e}"))?;
            if !used.is_empty() && value.is_none() {
                return Err(format!("{what}: placeholders need a value pattern"));
            }
            if let Some(re) = &value {
                if let Some(k) = used.iter().find(|k| **k > 0 && **k >= re.captures_len()) {
                    return Err(format!("{what}: {{{k}}} but the value pattern has no bracketed part {k}"));
                }
            }
            if !matches!(a.basis.as_str(), "vendor" | "community" | "memory") {
                return Err(format!("{what}: basis '{}' is not vendor, community or memory (nothing here may call itself proven)", a.basis));
            }
            actions.insert(kind, MqttAction { value, topic: a.topic, payload: a.payload, json, qos: a.qos, retain: a.retain, basis: a.basis, source: a.source });
        }
        for k in &raw.kinds {
            if !actions.contains_key(k) {
                return Err(format!("{id}: kind '{k}' has no action"));
            }
        }
        Ok(MqttProfile { id, name: raw.name, dept: raw.dept, port: raw.port, hint: raw.hint, notes: raw.notes, kinds: raw.kinds, actions, built_in })
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<MqttProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<MqttProfile>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn get(id: &str) -> Option<Arc<MqttProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

/// Load the MQTT profiles among a folder's files; `profiles::load_dir` leaves those to this. There is no built-in one: topics are the
/// installation's own.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(MqttProfile::from_json(&item.to_string(), false).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.contains_key(&p.id);
        lines.push(format!("{} ({}), an MQTT publisher{}", p.name, p.id, if replaced { ", replaces an earlier one" } else { "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

pub fn is_mqtt_json(v: &Value) -> bool {
    v.get("transport").and_then(Value::as_str) == Some("mqtt")
}

/// What a cue publishes.
#[derive(Debug, PartialEq)]
pub struct Publish {
    pub topic: String,
    pub payload: String,
    pub qos: u8,
    pub retain: bool,
}

/// Turn a cue into the message the profile makes of it, or say what is wrong with the cue.
pub fn publish_for(p: &MqttProfile, action: &Action) -> Result<Publish, String> {
    let a = p.actions.get(&action.kind).ok_or_else(|| format!("{} does not take a '{}' cue (it takes {})", p.name, action.kind, p.kinds.join(", ")))?;
    let v = crate::textlink::value_of(action);
    if v.chars().any(|c| c.is_control()) || v.len() > VALUE_MAX {
        return Err(format!("the value for {} is up to {VALUE_MAX} plain characters", p.name));
    }
    let caps: Vec<String> = match &a.value {
        Some(re) => {
            let c = re.captures(&v).ok_or_else(|| format!("'{v}' is not a value {} accepts for {}{}", p.name, action.kind, if p.hint.is_empty() { String::new() } else { format!(". {}", p.hint) }))?;
            (1..c.len()).map(|i| c.get(i).map(|m| m.as_str().to_string()).unwrap_or_default()).collect()
        }
        None => {
            if !v.is_empty() {
                return Err(format!("{} for {} takes no value, but the cue has '{v}'", action.kind, p.name));
            }
            Vec::new()
        }
    };
    // whatever goes into the topic is one plain name: no slash (a new level), no wildcard, nothing odd
    for k in placeholders(&a.topic).unwrap_or_default() {
        let part = if k == 0 { v.as_str() } else { caps.get(k - 1).map(String::as_str).unwrap_or("") };
        if part.is_empty() || part.len() > 40 || !part.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
            return Err(format!("'{part}' cannot go into a topic: a topic part is letters, digits, _ . or - (up to 40)"));
        }
    }
    Ok(Publish { topic: render(&a.topic, &v, &caps, Enc::Text), payload: render(&a.payload, &v, &caps, if a.json { Enc::Json } else { Enc::Text }), qos: a.qos, retain: a.retain })
}

/// The Doors check's note when this device's rules do not all rest on the vendor's document.
pub fn basis_note(dev: &Device) -> Option<String> {
    let p = get(dev.profile_key())?;
    crate::httpout::weak_note(&dev.label, &p.name, p.actions.values().map(|a| a.basis.as_str()))
}

fn profile_of(dev: &Device) -> Result<Arc<MqttProfile>, String> {
    get(dev.profile_key()).ok_or_else(|| format!("{} has no MQTT profile", dev.label))
}

/// Check a cue for an MQTT device. Sends nothing.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    publish_for(&*profile_of(dev)?, action).map(|_| ())
}

/// What sending the cue publishes, in words.
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let p = profile_of(dev)?;
    let m = publish_for(&p, action)?;
    let host = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or("(no host)");
    Ok(vec![(format!("{} MQTT to {host}: publish {} QoS {}{}", p.name, m.topic, m.qos, if m.retain { " retained" } else { "" }), vec![Value::String(m.payload)])])
}

fn target_of(dev: &Device, p: &MqttProfile) -> Result<String, String> {
    let host = dev.host.as_deref().map(str::trim).unwrap_or("");
    if host.is_empty() {
        return Err(format!("{} has no host in the rig", dev.label));
    }
    if host.len() > 100 || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) {
        return Err(format!("{}: the host '{host}' is not a plain address or name", dev.label));
    }
    let port = dev.port.or(p.port).unwrap_or(1883);
    Ok(format!("{host}:{port}"))
}

// ---- MQTT 3.1.1 packets ----

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

/// The "remaining length" of a packet, 1 to 4 bytes of 7 bits each.
pub fn remaining_length(mut n: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            return out;
        }
    }
}

fn packet(first: u8, body: Vec<u8>) -> Vec<u8> {
    let mut p = vec![first];
    p.extend(remaining_length(body.len()));
    p.extend(body);
    p
}

/// CONNECT: clean session, no will, no username or password, a 10-second keep-alive.
pub fn connect_packet(client_id: &str) -> Vec<u8> {
    let mut b = Vec::new();
    put_str(&mut b, b"MQTT");
    b.push(4); // protocol level 3.1.1
    b.push(0x02); // clean session only
    b.extend_from_slice(&10u16.to_be_bytes());
    put_str(&mut b, client_id.as_bytes());
    packet(0x10, b)
}

/// PUBLISH. A packet id goes with QoS 1 only.
pub fn publish_packet(m: &Publish, id: u16) -> Vec<u8> {
    let mut b = Vec::new();
    put_str(&mut b, m.topic.as_bytes());
    if m.qos > 0 {
        b.extend_from_slice(&id.to_be_bytes());
    }
    b.extend_from_slice(m.payload.as_bytes());
    packet(0x30 | (m.qos << 1) | m.retain as u8, b)
}

pub const DISCONNECT: [u8; 2] = [0xE0, 0x00];

fn connack_reason(rc: u8) -> String {
    match rc {
        1 => "the broker does not speak MQTT 3.1.1".into(),
        2 => "the broker refused this client's id".into(),
        3 => "the broker is not available".into(),
        4 => "bad username or password".into(),
        5 => "not authorised: this client sends no username, password or certificate, so a broker that needs one will refuse it".into(),
        n => format!("refused with code {n}"),
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);
static PACKET_ID: AtomicU16 = AtomicU16::new(1);

/// Connect, publish once, and (QoS 1) wait for the broker's acknowledgement.
pub async fn apply(dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
    let p = profile_of(dev).map_err(LinkError::Failed)?;
    let m = publish_for(&p, action).map_err(LinkError::Failed)?;
    let target = target_of(dev, &p).map_err(LinkError::Failed)?;
    let mut sock = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(&target))
        .await
        .map_err(|_| LinkError::Failed(format!("{} did not answer at {target} (timed out)", dev.label)))?
        .map_err(|e| LinkError::Failed(format!("cannot reach {} at {target}: {e}", dev.label)))?;
    let client_id = format!("pchi-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let id = PACKET_ID.fetch_add(1, Ordering::Relaxed).max(1);
    let (topic, qos) = (m.topic.clone(), m.qos);
    let go = async {
        sock.write_all(&connect_packet(&client_id)).await.map_err(|e| LinkError::Failed(format!("could not send: {e}")))?;
        let mut ack = [0u8; 4];
        sock.read_exact(&mut ack).await.map_err(|e| LinkError::Failed(format!("no CONNACK from the broker: {e}")))?;
        if ack[0] != 0x20 || ack[1] != 0x02 {
            return Err(LinkError::Failed("the answer is not an MQTT CONNACK: is this an MQTT broker?".into()));
        }
        if ack[3] != 0 {
            return Err(LinkError::Refused(format!("the broker refused the connection: {}", connack_reason(ack[3]))));
        }
        sock.write_all(&publish_packet(&m, id)).await.map_err(|e| LinkError::Failed(format!("could not publish: {e}")))?;
        if qos == 1 {
            let mut pa = [0u8; 4];
            sock.read_exact(&mut pa).await.map_err(|e| LinkError::Failed(format!("no PUBACK from the broker: {e}")))?;
            if pa[0] != 0x40 || pa[1] != 0x02 || u16::from_be_bytes([pa[2], pa[3]]) != id {
                return Err(LinkError::Failed("the broker's answer to the publish is not its PUBACK".into()));
            }
        }
        let _ = sock.write_all(&DISCONNECT).await;
        Ok(())
    };
    tokio::time::timeout(REPLY, go).await.map_err(|_| LinkError::Failed(format!("{} took the connection but did not answer (timed out)", dev.label)))??;
    let (status, note) = if qos == 1 { ("accepted", format!("published {topic}; the broker acknowledged it")) } else { ("sent", format!("published {topic} (QoS 0: no acknowledgement exists)")) };
    Ok(Outcome { status, note, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(actions: &str) -> String {
        format!(r#"{{"id":"x","name":"X","transport":"mqtt","dept":"lighting","kinds":[],"actions":{actions}}}"#)
    }
    fn bad(actions: &str) -> String {
        MqttProfile::from_json(&profile(actions), false).err().unwrap_or_default()
    }
    fn act(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"v": v}))
    }
    const RECIPE: &str = r#"{
      "id": "test-hall", "name": "Hall controller", "transport": "mqtt", "dept": "lighting", "port": 1883,
      "kinds": ["lighting.scene", "lighting.blackout"],
      "hint": "a scene name, letters and digits",
      "notes": "test",
      "actions": {
        "lighting.scene": {"value": "([A-Za-z0-9_-]{1,30})", "topic": "hall/lights/scene", "payload": "{{\"scene\":\"{1}\"}}", "format": "json", "qos": 1, "basis": "vendor", "source": "test"},
        "lighting.zone": {"value": "([a-z]{1,10})/(\\d{1,3})", "topic": "hall/zone/{1}/level", "payload": "{2}", "basis": "vendor", "source": "test"},
        "lighting.blackout": {"topic": "hall/lights/blackout", "payload": "1", "retain": true, "basis": "vendor", "source": "test"}
      }
    }"#;

    fn recipe() -> MqttProfile {
        let r = RECIPE.replace(r#""kinds": ["lighting.scene", "lighting.blackout"]"#, r#""kinds": ["lighting.scene", "lighting.blackout", "lighting.zone"]"#);
        MqttProfile::from_json(&r, false).unwrap()
    }
    fn dev(host: &str, port: Option<u16>) -> Device {
        Device { id: "m1".into(), label: "Hall".into(), kind: "test-hall".into(), dept: "lighting".into(), host: Some(host.into()), port, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }

    #[test]
    fn remaining_length_is_seven_bits_a_byte() {
        assert_eq!(remaining_length(0), [0]);
        assert_eq!(remaining_length(127), [0x7F]);
        assert_eq!(remaining_length(128), [0x80, 0x01]);
        assert_eq!(remaining_length(16_383), [0xFF, 0x7F]);
        assert_eq!(remaining_length(16_384), [0x80, 0x80, 0x01]);
    }

    #[test]
    fn the_packets_are_the_documented_bytes() {
        // CONNECT: MQTT, level 4, clean session, keep-alive 10, client id "ab"
        assert_eq!(connect_packet("ab"), [0x10, 0x0E, 0, 4, b'M', b'Q', b'T', b'T', 4, 0x02, 0, 10, 0, 2, b'a', b'b']);
        let m = |qos, retain| Publish { topic: "a/b".into(), payload: "hi".into(), qos, retain };
        assert_eq!(publish_packet(&m(0, false), 9), [0x30, 7, 0, 3, b'a', b'/', b'b', b'h', b'i']);
        assert_eq!(publish_packet(&m(1, false), 0x0102), [0x32, 9, 0, 3, b'a', b'/', b'b', 1, 2, b'h', b'i']);
        assert_eq!(publish_packet(&m(0, true), 9)[0], 0x31);
        assert_eq!(publish_packet(&m(1, true), 9)[0], 0x33);
        assert_eq!(DISCONNECT, [0xE0, 0]);
    }

    #[test]
    fn a_cue_makes_the_message_the_profile_says() {
        let p = recipe();
        assert_eq!(publish_for(&p, &act("lighting.scene", "Warm-1")).unwrap(), Publish { topic: "hall/lights/scene".into(), payload: r#"{"scene":"Warm-1"}"#.into(), qos: 1, retain: false });
        assert_eq!(publish_for(&p, &act("lighting.zone", "stage/80")).unwrap(), Publish { topic: "hall/zone/stage/level".into(), payload: "80".into(), qos: 0, retain: false });
        assert_eq!(publish_for(&p, &act("lighting.blackout", "")).unwrap(), Publish { topic: "hall/lights/blackout".into(), payload: "1".into(), qos: 0, retain: true });
        assert!(publish_for(&p, &act("lighting.blackout", "x")).unwrap_err().contains("takes no value"));
        assert!(publish_for(&p, &act("lighting.other", "x")).unwrap_err().contains("does not take"));
    }

    #[test]
    fn a_value_cannot_change_the_topic() {
        let t = r#"{"k":{"value":"(.{1,40})","topic":"room/{1}/set","payload":"{0}"}}"#;
        let p = MqttProfile::from_json(&profile(t), false).unwrap();
        for v in ["a/b", "+", "#", "a+b", "../x", "a b", "$SYS", "x/#", "", "é", &"a".repeat(41)] {
            assert!(publish_for(&p, &act("k", v)).is_err(), "{v:?}");
        }
        assert_eq!(publish_for(&p, &act("k", "stage-2.left_1")).unwrap().topic, "room/stage-2.left_1/set");
        // the payload may hold anything printable, and a JSON payload escapes it
        let j = r#"{"k":{"value":"(.{1,40})","topic":"room/all","payload":"{{\"v\":\"{1}\"}}","format":"json"}}"#;
        let p = MqttProfile::from_json(&profile(j), false).unwrap();
        assert_eq!(publish_for(&p, &act("k", r#"a"b"#)).unwrap().payload, r#"{"v":"a\"b"}"#);
    }

    #[test]
    fn a_profile_can_only_say_what_is_safe() {
        assert!(bad(r#"{"k":{"topic":"a/+/b"}}"#).contains("wildcards"));
        assert!(bad(r#"{"k":{"topic":"a/#"}}"#).contains("wildcards"));
        assert!(bad(r#"{"k":{"topic":"a b"}}"#).contains("wildcards"));
        assert!(bad(r#"{"k":{"topic":"$SYS/x"}}"#).contains("letters, digits"));
        assert!(bad(r#"{"k":{"topic":"/a"}}"#).contains("start with $ or /"));
        assert!(bad(r#"{"k":{"topic":"a//b"}}"#).contains("//"));
        assert!(bad(r#"{"k":{"topic":"a/"}}"#).contains("end with /"));
        assert!(bad(r#"{"k":{"topic":""}}"#).contains("letters, digits"));
        assert!(bad(r#"{"k":{"topic":"a","qos":2}}"#).contains("qos is 0 or 1"));
        assert!(bad(r#"{"k":{"topic":"a","format":"xml"}}"#).contains("text or json"));
        assert!(bad(r#"{"k":{"topic":"a/{0}"}}"#).contains("need a value pattern"));
        assert!(bad(r#"{"k":{"topic":"a/{2}","value":"(.)"}}"#).contains("no bracketed part 2"));
        assert!(bad(r#"{"k":{"topic":"a","basis":"proven"}}"#).contains("proven"));
        assert!(bad(r#"{"k":{"topic":"a","extra":1}}"#).contains("unknown field"));
        assert!(bad("{}").contains("no actions"));
        assert!(MqttProfile::from_json(&profile(r#"{"k":{"topic":"a"}}"#).replace("lighting", "laser"), false).unwrap_err().contains("laser or motion"));
    }

    #[test]
    fn a_cue_is_checked_and_described_without_sending() {
        let _ = load_values(vec![serde_json::from_str(&RECIPE.replace(r#""kinds": ["lighting.scene", "lighting.blackout"]"#, r#""kinds": ["lighting.scene"]"#)).unwrap()], "test").unwrap();
        assert!(is_mqtt_json(&serde_json::from_str(RECIPE).unwrap()));
        let d = dev("10.0.0.7", None);
        assert!(check_action(&d, &act("lighting.scene", "Warm")).is_ok());
        assert!(check_action(&d, &act("lighting.scene", "a b")).is_err());
        let l = describe(&d, &act("lighting.scene", "Warm")).unwrap();
        assert_eq!(l[0].0, "Hall controller MQTT to 10.0.0.7: publish hall/lights/scene QoS 1");
        assert_eq!(l[0].1, vec![json!(r#"{"scene":"Warm"}"#)]);
        assert_eq!(target_of(&d, &get("test-hall").unwrap()).unwrap(), "10.0.0.7:1883");
        assert!(target_of(&dev("", None), &get("test-hall").unwrap()).is_err());
        assert_eq!(dept_of("test-hall").as_deref(), Some("lighting"));
    }

    /// A stand-in broker: answers CONNECT with `connack_rc`, reads the PUBLISH, answers PUBACK if asked, returns every packet it read.
    async fn broker(connack_rc: u8, puback: bool) -> (u16, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut seen = Vec::new();
            async fn read_packet(s: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
                let mut first = [0u8; 1];
                s.read_exact(&mut first).await.ok()?;
                let (mut len, mut mult) = (0usize, 1usize);
                let mut raw = vec![first[0]];
                loop {
                    let mut b = [0u8; 1];
                    s.read_exact(&mut b).await.ok()?;
                    raw.push(b[0]);
                    len += (b[0] & 0x7F) as usize * mult;
                    mult *= 128;
                    if b[0] & 0x80 == 0 {
                        break;
                    }
                }
                let mut body = vec![0u8; len];
                s.read_exact(&mut body).await.ok()?;
                raw.extend(body);
                Some(raw)
            }
            seen.push(read_packet(&mut s).await.unwrap());
            s.write_all(&[0x20, 0x02, 0x00, connack_rc]).await.unwrap();
            if connack_rc != 0 {
                return seen;
            }
            let publ = read_packet(&mut s).await.unwrap();
            if puback {
                // topic length is at bytes 2..4 for a short packet; the packet id follows the topic
                let tl = u16::from_be_bytes([publ[2], publ[3]]) as usize;
                s.write_all(&[0x40, 0x02, publ[4 + tl], publ[5 + tl]]).await.unwrap();
            }
            seen.push(publ);
            if let Some(d) = read_packet(&mut s).await {
                seen.push(d);
            }
            seen
        });
        (port, h)
    }

    #[tokio::test]
    async fn a_stand_in_broker_receives_connect_publish_and_disconnect() {
        let _ = load_values(vec![serde_json::from_str(&RECIPE.replace(r#""kinds": ["lighting.scene", "lighting.blackout"]"#, r#""kinds": ["lighting.scene", "lighting.blackout", "lighting.zone"]"#)).unwrap()], "test").unwrap();
        // QoS 1: accepted when the PUBACK arrives
        let (port, h) = broker(0, true).await;
        let o = apply(&dev("127.0.0.1", Some(port)), &act("lighting.scene", "Warm")).await.unwrap();
        assert_eq!((o.status, o.target.as_str()), ("accepted", format!("127.0.0.1:{port}").as_str()));
        let seen = h.await.unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0][0], 0x10, "CONNECT first");
        assert_eq!(&seen[0][2..8], [0, 4, b'M', b'Q', b'T', b'T']);
        assert_eq!(seen[1][0], 0x32, "PUBLISH, QoS 1");
        assert!(seen[1].ends_with(br#"{"scene":"Warm"}"#));
        assert_eq!(seen[2], DISCONNECT);

        // QoS 0: sent, and no reply is waited for
        let (port, h) = broker(0, false).await;
        let o = apply(&dev("127.0.0.1", Some(port)), &act("lighting.blackout", "")).await.unwrap();
        assert_eq!(o.status, "sent");
        let seen = h.await.unwrap();
        assert_eq!(seen[1][0], 0x31, "retained, QoS 0");

        // a broker that refuses
        let (port, _) = broker(5, false).await;
        let e = apply(&dev("127.0.0.1", Some(port)), &act("lighting.blackout", "")).await.unwrap_err();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("not authorised") && e.text().contains("no username"), "{}", e.text());

        // not a broker, and nothing listening
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut b = [0u8; 64];
            let _ = s.read(&mut b).await;
            let _ = s.write_all(b"HTTP").await;
        });
        assert!(apply(&dev("127.0.0.1", Some(port)), &act("lighting.blackout", "")).await.unwrap_err().text().contains("not an MQTT CONNACK"));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(apply(&dev("127.0.0.1", Some(dead)), &act("lighting.blackout", "")).await.unwrap_err().status(), "failed");
        // a bad value is refused before any connection
        assert!(apply(&dev("127.0.0.1", Some(dead)), &act("lighting.scene", "a b")).await.unwrap_err().text().contains("not a value"));
    }
}
