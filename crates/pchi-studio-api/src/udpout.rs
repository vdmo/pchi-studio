//! UDP text output: a cue becomes one datagram of text sent to one device, for lighting and media gear that listens for text commands on a
//! UDP port (this is how many show controllers, media servers and relay boxes are driven).
//!
//! The commands come from a **closed list in a profile file** (`"transport": "udp"`): per cue kind a text template, a line ending and a
//! value pattern. A cue supplies only a value, checked against the pattern; a value cannot contain a control character, so it cannot end
//! the text early or add a second command. **A cue cannot choose the destination**: it is the device's `host` and `port` in the rig.
//!
//! - Unicast only. A broadcast, multicast or unspecified address is refused, so one cue cannot reach every machine on a network. (A
//!   subnet-directed broadcast such as 192.168.1.255 cannot be told from a normal address without knowing the netmask: do not put one in.)
//! - One datagram per cue. UDP gives no acknowledgement: the best outcome is `sent`, meaning the datagram was handed to the network.
//!   Nothing says it arrived or that the device acted on it.
//! - Text only, printable ASCII, up to 200 characters. No binary bytes, no checksums.
//! - Laser and motion profiles are refused, as for the other profile-driven outputs.

use crate::httpout::{placeholders, render, Enc};
use crate::rig::Device;
use crate::textlink::{LinkError, Outcome};
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

const VALUE_MAX: usize = 64;
const LINE_MAX: usize = 200;
const RESOLVE: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    value: Option<String>,
    send: String,
    /// `none` (default), `cr`, `lf` or `crlf`.
    #[serde(default)]
    end: Option<String>,
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
pub struct UdpAction {
    pub value: Option<Regex>,
    pub send: String,
    pub end: &'static [u8],
    pub basis: String,
    pub source: String,
}

#[derive(Debug)]
pub struct UdpProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub port: Option<u16>,
    pub hint: String,
    pub notes: String,
    pub kinds: Vec<String>,
    pub actions: HashMap<String, UdpAction>,
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

impl UdpProfile {
    pub fn from_json(text: &str) -> Result<UdpProfile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a valid UDP profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("profile id '{id}' must be letters, digits, - or _ (up to 40)"));
        }
        if !matches!(raw.dept.as_str(), "visual" | "lighting" | "audio") {
            return Err(format!("{id}: dept '{}' is not one of visual, lighting, audio (a laser or motion device needs an acknowledgement and a stop, which UDP does not give)", raw.dept));
        }
        if raw.port == Some(0) {
            return Err(format!("{id}: port 0 is not a port"));
        }
        if raw.actions.is_empty() {
            return Err(format!("{id}: no actions"));
        }
        let mut actions = HashMap::new();
        for (kind, a) in raw.actions {
            let what = format!("{id} {kind}");
            if a.send.is_empty() || a.send.len() > LINE_MAX || a.send.chars().any(|c| !(c.is_ascii() && !c.is_control())) {
                return Err(format!("{what}: a command is 1 to {LINE_MAX} printable ASCII characters (the line ending is set by `end`)"));
            }
            let end: &'static [u8] = match a.end.as_deref() {
                None | Some("none") => b"",
                Some("cr") => b"\r",
                Some("lf") => b"\n",
                Some("crlf") => b"\r\n",
                Some(e) => return Err(format!("{what}: end '{e}' is none, cr, lf or crlf")),
            };
            let used = placeholders(&a.send).map_err(|e| format!("{what}: {e}"))?;
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
            actions.insert(kind, UdpAction { value, send: a.send, end, basis: a.basis, source: a.source });
        }
        for k in &raw.kinds {
            if !actions.contains_key(k) {
                return Err(format!("{id}: kind '{k}' has no action"));
            }
        }
        Ok(UdpProfile { id, name: raw.name, dept: raw.dept, port: raw.port, hint: raw.hint, notes: raw.notes, kinds: raw.kinds, actions })
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<UdpProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<UdpProfile>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn get(id: &str) -> Option<Arc<UdpProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

pub fn is_udp_json(v: &Value) -> bool {
    v.get("transport").and_then(Value::as_str) == Some("udp")
}

/// Load the UDP profiles among a folder's files; `profiles::load_dir` leaves those to this. There is no built-in one: the commands are
/// the device's own.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(UdpProfile::from_json(&item.to_string()).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.contains_key(&p.id);
        lines.push(format!("{} ({}), a UDP text device{}", p.name, p.id, if replaced { ", replaces an earlier one" } else { "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

/// The bytes one cue puts in its datagram.
pub fn datagram_for(p: &UdpProfile, action: &Action) -> Result<Vec<u8>, String> {
    let a = p.actions.get(&action.kind).ok_or_else(|| format!("{} does not take a '{}' cue (it takes {})", p.name, action.kind, p.kinds.join(", ")))?;
    let v = crate::textlink::value_of(action);
    if v.chars().any(|c| c.is_control() || !c.is_ascii()) || v.len() > VALUE_MAX {
        return Err(format!("the value for {} is up to {VALUE_MAX} plain ASCII characters", p.name));
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
    let text = render(&a.send, &v, &caps, Enc::Text);
    if text.len() > LINE_MAX {
        return Err(format!("the command for {} would be longer than {LINE_MAX} characters", p.name));
    }
    let mut b = text.into_bytes();
    b.extend_from_slice(a.end);
    Ok(b)
}

fn profile_of(dev: &Device) -> Result<Arc<UdpProfile>, String> {
    get(dev.profile_key()).ok_or_else(|| format!("{} has no UDP profile", dev.label))
}

/// The destination as `host:port`, from the rig only.
fn target_of(dev: &Device, p: &UdpProfile) -> Result<String, String> {
    let host = dev.host.as_deref().map(str::trim).unwrap_or("");
    if host.is_empty() {
        return Err(format!("{} has no host in the rig", dev.label));
    }
    if host.len() > 100 || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) {
        return Err(format!("{}: the host '{host}' is not a plain address or name", dev.label));
    }
    let port = dev.port.or(p.port).ok_or_else(|| format!("{} has no port in the rig", dev.label))?;
    if port == 0 {
        return Err(format!("{}: port 0 is not a port", dev.label));
    }
    // a bare IPv6 address needs its brackets to take a port
    if host.contains(':') && !host.starts_with('[') {
        return Ok(format!("[{host}]:{port}"));
    }
    Ok(format!("{host}:{port}"))
}

/// Whether an address would reach more than one machine, or none.
fn not_unicast(ip: &IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(a) if a.is_broadcast() => Some("a broadcast address"),
        IpAddr::V4(a) if a.is_multicast() => Some("a multicast address"),
        IpAddr::V4(a) if a.is_unspecified() => Some("the unspecified address"),
        IpAddr::V6(a) if a.is_multicast() => Some("a multicast address"),
        IpAddr::V6(a) if a.is_unspecified() => Some("the unspecified address"),
        _ => None,
    }
}

/// Check a cue for a UDP device. Sends nothing.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    let p = profile_of(dev)?;
    datagram_for(&p, action)?;
    let t = target_of(dev, &p)?;
    // a literal address can be judged now; a name is judged when it is resolved
    if let Ok(sa) = t.parse::<std::net::SocketAddr>() {
        if let Some(why) = not_unicast(&sa.ip()) {
            return Err(format!("{}: {} is {why}; one device at a time only", dev.label, sa.ip()));
        }
    }
    Ok(())
}

fn show(b: &[u8]) -> String {
    b.iter().map(|c| match c {
        b'\r' => "\\r".to_string(),
        b'\n' => "\\n".to_string(),
        c => (*c as char).to_string(),
    }).collect()
}

/// What sending the cue puts on the network, in words.
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let p = profile_of(dev)?;
    let b = datagram_for(&p, action)?;
    let to = target_of(dev, &p).unwrap_or_else(|_| "(no destination)".into());
    Ok(vec![(format!("{} UDP to {to}: {}", p.name, show(&b)), vec![])])
}

/// What the Doors check says about a profile whose rules do not rest on the vendor's document.
pub fn weak_note(dev: &Device) -> Option<String> {
    let p = get(dev.profile_key())?;
    crate::httpout::weak_note(&dev.label, &p.name, p.actions.values().map(|a| a.basis.as_str()))
}

/// Send a cue: one datagram. `sent` is the most this can say.
pub async fn apply(dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
    let p = profile_of(dev).map_err(LinkError::Failed)?;
    let bytes = datagram_for(&p, action).map_err(LinkError::Failed)?;
    let target = target_of(dev, &p).map_err(LinkError::Failed)?;
    let addr = tokio::time::timeout(RESOLVE, tokio::net::lookup_host(&target))
        .await
        .map_err(|_| LinkError::Failed(format!("could not resolve {target} (timed out)")))?
        .map_err(|e| LinkError::Failed(format!("could not resolve {target}: {e}")))?
        .next()
        .ok_or_else(|| LinkError::Failed(format!("{target} resolves to no address")))?;
    if let Some(why) = not_unicast(&addr.ip()) {
        return Err(LinkError::Refused(format!("{}: {} is {why}; one device at a time only", dev.label, addr.ip())));
    }
    let local = if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    let sock = tokio::net::UdpSocket::bind(local).await.map_err(|e| LinkError::Failed(format!("could not open a socket: {e}")))?;
    sock.send_to(&bytes, addr).await.map_err(|e| LinkError::Failed(format!("send to {target} failed: {e}")))?;
    Ok(Outcome { status: "sent", note: format!("{} bytes handed to the network; UDP says nothing about arrival", bytes.len()), target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(actions: &str) -> String {
        format!(r#"{{"id":"x","name":"X","transport":"udp","dept":"lighting","port":7000,"kinds":[],"actions":{actions}}}"#)
    }
    fn bad(actions: &str) -> String {
        UdpProfile::from_json(&profile(actions)).err().unwrap_or_default()
    }
    fn act(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"v": v}))
    }
    const BOX: &str = r#"{
      "id": "test-udp", "name": "Test box", "transport": "udp", "dept": "lighting", "port": 7000,
      "kinds": ["lighting.cue", "lighting.blackout"],
      "hint": "a cue number, 1 to 999",
      "notes": "test",
      "actions": {
        "lighting.cue": {"value": "([1-9][0-9]{0,2})", "send": "GO {1}", "end": "lf", "basis": "vendor", "source": "test"},
        "lighting.blackout": {"send": "BLACKOUT", "basis": "vendor", "source": "test"}
      }
    }"#;
    fn dev(host: &str, port: Option<u16>) -> Device {
        Device { id: "u1".into(), label: "Box".into(), kind: "test-udp".into(), dept: "lighting".into(), host: Some(host.into()), port, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }
    fn load() {
        let _ = load_values(vec![serde_json::from_str(BOX).unwrap()], "test").unwrap();
    }

    #[test]
    fn a_bad_profile_is_refused() {
        assert!(UdpProfile::from_json(BOX).is_ok());
        assert!(UdpProfile::from_json(&BOX.replace("lighting\", \"port", "laser\", \"port")).unwrap_err().contains("not one of"));
        assert!(UdpProfile::from_json(&BOX.replace("\"port\": 7000", "\"port\": 0")).unwrap_err().contains("port 0"));
        assert!(bad(r#"{"a":{"send":"GO\u0007","basis":"memory"}}"#).contains("printable ASCII"));
        assert!(bad(r#"{"a":{"send":"GO {1}","basis":"memory"}}"#).contains("need a value pattern"));
        assert!(bad(r#"{"a":{"send":"GO {2}","value":"(\\d)","basis":"memory"}}"#).contains("no bracketed part 2"));
        assert!(bad(r#"{"a":{"send":"GO","basis":"proven"}}"#).contains("nothing here may call itself proven"));
        assert!(bad(r#"{"a":{"send":"GO","end":"nul","basis":"memory"}}"#).contains("end 'nul'"));
        assert!(bad(r#"{"a":{"send":"","basis":"memory"}}"#).contains("printable ASCII"));
        assert!(bad("{}").contains("no actions"));
        assert!(UdpProfile::from_json(&BOX.replace("\"kinds\": [\"lighting.cue\", \"lighting.blackout\"]", "\"kinds\": [\"lighting.zzz\"]")).unwrap_err().contains("has no action"));
    }

    #[test]
    fn a_cue_makes_the_datagram_the_profile_says() {
        load();
        let p = get("test-udp").unwrap();
        assert_eq!(datagram_for(&p, &act("lighting.cue", "12")).unwrap(), b"GO 12\n");
        assert_eq!(datagram_for(&p, &Action::new("lighting.blackout", json!({}))).unwrap(), b"BLACKOUT");
        for v in ["0", "1000", "1\n2", "1\r2", "a", ""] {
            assert!(datagram_for(&p, &act("lighting.cue", v)).is_err(), "{v:?}");
        }
        assert!(datagram_for(&p, &act("lighting.blackout", "5")).unwrap_err().contains("takes no value"));
        assert!(datagram_for(&p, &act("lighting.zzz", "")).unwrap_err().contains("does not take"));
        assert!(datagram_for(&p, &act("lighting.cue", &"1".repeat(70))).is_err());
    }

    #[test]
    fn the_destination_comes_only_from_the_rig() {
        load();
        let p = get("test-udp").unwrap();
        assert_eq!(target_of(&dev("10.0.0.5", None), &p).unwrap(), "10.0.0.5:7000");
        assert_eq!(target_of(&dev("10.0.0.5", Some(9)), &p).unwrap(), "10.0.0.5:9");
        assert_eq!(target_of(&dev("fe80::1", None), &p).unwrap(), "[fe80::1]:7000");
        assert!(target_of(&dev("", None), &p).is_err());
        assert!(target_of(&dev("a b", None), &p).is_err());
        assert!(target_of(&dev("host/../x", None), &p).is_err());
    }

    #[test]
    fn broadcast_multicast_and_unspecified_are_refused() {
        load();
        for h in ["255.255.255.255", "224.0.0.1", "239.255.0.1", "0.0.0.0", "ff02::1", "::"] {
            let e = check_action(&dev(h, None), &act("lighting.blackout", "")).unwrap_err();
            assert!(e.contains("one device at a time"), "{h}: {e}");
        }
        assert!(check_action(&dev("10.0.0.5", None), &act("lighting.blackout", "")).is_ok());
        assert!(check_action(&dev("10.0.0.5", None), &act("lighting.cue", "0")).is_err());
    }

    #[tokio::test]
    async fn the_exact_bytes_reach_a_listener_and_the_outcome_is_only_sent() {
        load();
        let rx = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = rx.local_addr().unwrap().port();
        let d = dev("127.0.0.1", Some(port));
        let o = apply(&d, &act("lighting.cue", "42")).await.unwrap();
        assert_eq!(o.status, "sent");
        assert_eq!(o.target, format!("127.0.0.1:{port}"));
        let mut buf = [0u8; 64];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), rx.recv_from(&mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf[..n], b"GO 42\n");
        // a bad cue sends nothing
        assert!(apply(&d, &act("lighting.cue", "0")).await.is_err());
        assert!(tokio::time::timeout(Duration::from_millis(200), rx.recv_from(&mut buf)).await.is_err(), "nothing was sent");
        // a multicast host is refused before any socket is opened
        let m = dev("224.0.0.1", Some(port));
        assert!(matches!(apply(&m, &act("lighting.blackout", "")).await, Err(LinkError::Refused(_))));
    }

    #[test]
    fn what_a_cue_would_send_can_be_read_without_sending_it() {
        load();
        let w = describe(&dev("10.0.0.5", None), &act("lighting.cue", "7")).unwrap();
        assert_eq!(w[0].0, "Test box UDP to 10.0.0.5:7000: GO 7\\n");
    }

    #[test]
    fn a_profile_that_rests_on_memory_is_called_out() {
        let _ = load_values(vec![serde_json::from_str(&BOX.replace("\"vendor\"", "\"memory\"").replace("test-udp", "test-udp-mem")).unwrap()], "test").unwrap();
        let mut d = dev("10.0.0.5", None);
        d.kind = "test-udp-mem".into();
        assert!(weak_note(&d).unwrap().contains("memory"));
        load();
        assert!(weak_note(&dev("10.0.0.5", None)).is_none());
    }
}
