//! Sending cues to rig devices.
//!
//! Each cue (and each scene enter/exit action) is routed to the device that
//! handles it: the lane's chosen device, else the only device of its department.
//! The message is generic PCHI-over-OSC (UDP): address `/pchi/<kind with . as />`,
//! arguments are the action's payload values in key order. A desk that does not
//! speak this needs an adapter that translates it (see docs/connecting-systems.md).
//!
//! Two honest limits:
//! * UDP has no acknowledgement. "sent" means the packet left this machine, not
//!   that the device acted on it. Only the device's own READY report says that.
//! * Sending is OFF unless the runtime is started with `--send`. Without it every
//!   delivery is reported as "dry-run" with where it would have gone.
//!
//! Nothing here can stop or delay the show: sends run in their own tasks with a
//! timeout and a failure is only ever reported.

use crate::rig::{dept_of_kind, resolve, Device};
use pchi_showchart::Action;
use crate::links::Links;
use crate::profiles;
use rosc::{encoder, OscMessage, OscPacket, OscType};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{lookup_host, UdpSocket};

const SEND_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Output {
    devices: Vec<Device>,
    send: bool,
    /// Laser devices the operator has armed. A laser gets nothing from the show until it is armed,
    /// and every runtime start begins disarmed.
    armed: Mutex<BTreeSet<String>>,
    /// Open sockets to the devices, shared across live edits so listening never stops.
    links: Arc<Links>,
    /// How often a device that can be probed is asked to answer, and how long silence counts as lost.
    pub probe_every: Duration,
    pub link_timeout: Duration,
    /// Art-Net and sACN devices: their universes and the tasks that keep sending them. Shared across live edits.
    dmx: Arc<crate::dmx::Dmx>,
    /// OBS devices: their connections, kept open. Shared across live edits.
    obs: Arc<crate::obs::Obs>,
}

/// What routing decided for one action.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Nothing to do: the action belongs to no department (e.g. `cue.trigger`).
    NotRouted,
    /// No device can take it. The string says why.
    NoDevice { dept: String, reason: String },
    To(Device),
}

impl Output {
    pub fn new(devices: Vec<Device>, send: bool) -> Self {
        Self { devices, send, armed: Mutex::new(BTreeSet::new()), links: Arc::new(Links::default()), probe_every: Duration::from_secs(5), link_timeout: Duration::from_secs(15), dmx: Arc::new(crate::dmx::Dmx::default()), obs: Arc::new(crate::obs::Obs::default()) }
    }

    /// Change how often devices are probed and when silence counts as lost (the defaults are 5 s and 15 s).
    pub fn with_timing(mut self, probe_every: Duration, link_timeout: Duration) -> Self {
        self.probe_every = probe_every;
        self.link_timeout = link_timeout;
        self
    }

    pub fn links(&self) -> &Arc<Links> {
        &self.links
    }

    /// Whether this device is expected to answer: it can be probed, or has a listen port.
    pub fn expects_feedback(dev: &Device) -> bool {
        dev.kind == "obs" || profiles::probe(dev.profile_key()).is_some() || dev.listen_port.is_some()
    }

    /// The device's link: "linked" (answered a probe recently), "lost" (answered before, now silent),
    /// "heard" (a tool we only listen to has sent something), "silent" (expected to talk, has not yet)
    /// or "none" (this tool gives no feedback). Absent when the runtime is not sending.
    pub fn link_state(&self, dev: &Device) -> Option<Value> {
        if !self.send {
            return None;
        }
        if dev.kind == "obs" {
            // OBS's own connection says whether it is there
            let (st, text) = self.obs.state(&dev.id);
            return Some(json!({"state": st, "heard_ms_ago": null, "probed": true, "note": text}));
        }
        let heard = self.links.last_heard(&dev.id);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let age = (heard > 0).then(|| (now - heard).max(0));
        let probed = profiles::probe(dev.profile_key()).is_some();
        let state = match (probed, dev.listen_port.is_some(), age) {
            (true, _, Some(a)) if a <= self.link_timeout.as_millis() as i64 => "linked",
            (true, _, Some(_)) => "lost",
            (false, _, Some(_)) => "heard",
            (_, true, None) | (true, _, None) => "silent",
            (false, false, None) => "none",
        };
        Some(json!({"state": state, "heard_ms_ago": age, "probed": probed}))
    }

    /// The same output with a new device list (after a live edit), keeping the armed state of lasers that are still there.
    pub fn with_devices(&self, devices: Vec<Device>) -> Self {
        let keep: BTreeSet<String> = self.armed.lock().unwrap().iter().filter(|id| devices.iter().any(|d| &d.id == *id && d.dept == "laser")).cloned().collect();
        Self { devices, send: self.send, armed: Mutex::new(keep), links: Arc::clone(&self.links), probe_every: self.probe_every, link_timeout: self.link_timeout, dmx: Arc::clone(&self.dmx), obs: Arc::clone(&self.obs) }
    }

    pub fn is_armed(&self, id: &str) -> bool {
        self.armed.lock().unwrap().contains(id)
    }
    /// Arm or disarm a laser device. Returns false if there is no such laser in the rig.
    pub fn set_armed(&self, id: &str, armed: bool) -> bool {
        if !self.devices.iter().any(|d| d.id == id && d.dept == "laser") {
            return false;
        }
        let mut a = self.armed.lock().unwrap();
        if armed {
            a.insert(id.to_string());
        } else {
            a.remove(id);
        }
        true
    }
    pub fn lasers(&self) -> Vec<Device> {
        self.devices.iter().filter(|d| d.dept == "laser").cloned().collect()
    }
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }
    pub fn sending(&self) -> bool {
        self.send
    }

    /// Decide where an action goes. `dept` and `device` come from its lane (enter and exit actions have none).
    pub fn plan(&self, kind: &str, dept: Option<&str>, device: Option<&str>) -> Plan {
        let dept = dept.filter(|d| !d.is_empty()).unwrap_or_else(|| dept_of_kind(kind));
        if dept == "other" {
            return Plan::NotRouted;
        }
        match resolve(&self.devices, dept, device) {
            Ok(d) => Plan::To(d.clone()),
            Err(reason) => Plan::NoDevice { dept: dept.to_string(), reason },
        }
    }
}

/// The OSC message for an action.
pub fn encode(action: &Action) -> Result<Vec<u8>, String> {
    let clean: String = action.kind.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '/' }).collect();
    let mut args = Vec::new();
    if let Value::Object(p) = &action.payload {
        for (k, v) in p {
            if k == "type" {
                continue;
            }
            args.push(match v {
                Value::String(s) => OscType::String(s.clone()),
                Value::Bool(b) => OscType::Bool(*b),
                Value::Number(n) => match n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                    Some(i) => OscType::Int(i),
                    None => OscType::Float(n.as_f64().unwrap_or(0.0) as f32),
                },
                other => OscType::String(other.to_string()),
            });
        }
    }
    encoder::encode(&OscPacket::Message(OscMessage { addr: format!("/pchi/{clean}"), args })).map_err(|e| format!("{e:?}"))
}

/// Send one action to a device. Returns the `host:port` it went to. A device of a profiled type
/// (see the `profiles` module) gets that tool's own messages; anything else gets the generic ones.
pub async fn deliver(dev: &Device, action: &Action) -> Result<String, String> {
    send_packets(dev, packets_for(dev, action)?).await
}

impl Output {
    /// Start (or check) the connection of an OBS device.
    pub async fn obs_ensure(&self, dev: &Device) -> Result<(), String> {
        self.obs.ensure(dev).await
    }

    /// Send an action to a device through its kept socket, so the device can answer on it.
    pub async fn deliver(&self, dev: &Device, action: &Action) -> Result<String, String> {
        if dev.is_dmx() {
            return self.dmx.apply(dev, action).await;
        }
        if dev.kind == "obs" {
            return self.obs.apply(dev, action).await;
        }
        self.send_packets(dev, packets_for(dev, action)?).await
    }

    /// Send ready-made OSC packets to a device through its kept socket.
    pub async fn send_packets(&self, dev: &Device, packets: Vec<Vec<u8>>) -> Result<String, String> {
        if dev.is_direct() {
            return Err(format!("{} is reached by its own protocol, not by OSC messages", dev.label));
        }
        let addr = dev.address().ok_or_else(|| format!("{} has no host and port in the rig", dev.label))?;
        let go = async {
            let link = self.links.get(dev).await?;
            for bytes in &packets {
                link.socket.send_to(bytes, link.target).await.map_err(|e| format!("send to {addr} failed: {e}"))?;
            }
            Ok::<_, String>(addr.clone())
        };
        tokio::time::timeout(SEND_TIMEOUT, go).await.map_err(|_| format!("sending to {addr} timed out"))?
    }
}

/// What sending `action` to `dev` puts on the wire, as (address, arguments) lines for a person to read: the OSC messages, or for
/// a DMX device which universe gets which channels. Errors say what to fix.
pub fn wire_lines(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    if dev.is_dmx() {
        let summary = crate::dmx::describe(dev, action)?;
        let proto = crate::dmx::proto_of(&dev.kind).ok_or("not a DMX device")?;
        return Ok(vec![(format!("{} universe {}", proto.name(), dev.universe.unwrap_or(proto.default_universe())), vec![Value::String(crate::dmx::value_of(action)), Value::String(summary)])]);
    }
    if dev.kind == "obs" {
        let scene = crate::obs::scene_of(action)?;
        return Ok(vec![("OBS: switch the program scene to".into(), vec![Value::String(scene)])]);
    }
    Ok(packets_for(dev, action)?.iter().flat_map(|p| crate::links::flatten(p)).collect())
}

pub fn packets_for(dev: &Device, action: &Action) -> Result<Vec<Vec<u8>>, String> {
    if dev.is_direct() {
        return Err(format!("{} is reached by its own protocol, not by OSC messages", dev.label));
    }
    Ok(match (action.kind.as_str(), profiles::encode(dev.profile_key(), action)) {
        ("test", _) | (_, None) => vec![encode(action)?],
        (_, Some(r)) => r?,
    })
}

/// Send ready-made OSC packets to a device.
pub async fn send_packets(dev: &Device, packets: Vec<Vec<u8>>) -> Result<String, String> {
    let addr = dev.address().ok_or_else(|| format!("{} has no host and port in the rig", dev.label))?;
    let go = async {
        let target = lookup_host(&addr).await.map_err(|e| format!("cannot resolve {addr}: {e}"))?.next().ok_or_else(|| format!("cannot resolve {addr}"))?;
        let bind = if target.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let sock = UdpSocket::bind(bind).await.map_err(|e| format!("cannot open a socket: {e}"))?;
        for bytes in &packets {
            sock.send_to(bytes, target).await.map_err(|e| format!("send to {addr} failed: {e}"))?;
        }
        Ok::<_, String>(addr.clone())
    };
    tokio::time::timeout(SEND_TIMEOUT, go).await.map_err(|_| format!("sending to {addr} timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::dept_of_type;
    use serde_json::json;

    fn dev(id: &str, kind: &str, port: u16) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: None }
    }

    #[test]
    fn plan_routes_by_lane_then_by_kind_and_skips_unrouted() {
        let out = Output::new(vec![dev("l1", "laser", 1), dev("d1", "lighting-desk", 2)], false);
        assert_eq!(out.plan("laser.cue", Some("laser"), None), Plan::To(dev("l1", "laser", 1)));
        // an enter action has no lane: the kind decides
        assert_eq!(out.plan("lighting.look", None, None), Plan::To(dev("d1", "lighting-desk", 2)));
        assert_eq!(out.plan("cue.trigger", None, None), Plan::NotRouted);
        match out.plan("audio.snapshot", Some("audio"), None) {
            Plan::NoDevice { dept, reason } => {
                assert_eq!(dept, "audio");
                assert!(reason.contains("no audio device"));
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn lasers_start_disarmed_and_keep_their_state_across_a_live_edit() {
        let out = Output::new(vec![dev("l1", "laser", 1), dev("b1", "beyond", 2), dev("d1", "lighting-desk", 3)], false);
        assert!(!out.is_armed("l1") && !out.is_armed("b1"));
        assert!(out.set_armed("b1", true));
        assert!(!out.set_armed("d1", true), "only lasers can be armed");
        assert!(!out.set_armed("nope", true));
        assert_eq!(out.lasers().len(), 2);
        // a new device list: b1 is still there (stays armed), l1 is gone
        let next = out.with_devices(vec![dev("b1", "beyond", 2), dev("d1", "lighting-desk", 3)]);
        assert!(next.is_armed("b1"));
        assert!(out.set_armed("b1", false));
        assert!(!out.is_armed("b1"));
    }

    #[tokio::test]
    async fn a_profiled_device_gets_the_tools_own_message() {
        let rx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = rx.local_addr().unwrap().port();
        deliver(&dev("e1", "eos", port), &Action::new("lighting.look", json!({"look": "3/9"}))).await.unwrap();
        let mut buf = [0u8; 256];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), rx.recv_from(&mut buf)).await.unwrap().unwrap();
        let OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1 else { panic!() };
        assert_eq!(m.addr, "/eos/cue/3/9/fire");
        // a value the tool cannot use is an error, not a guess
        assert!(deliver(&dev("e1", "eos", port), &Action::new("lighting.look", json!({"look": "warm"}))).await.is_err());
        // the test message stays generic, so it is harmless on any tool
        deliver(&dev("e1", "eos", port), &Action::new("test", json!({}))).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), rx.recv_from(&mut buf)).await.unwrap().unwrap();
        let OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1 else { panic!() };
        assert_eq!(m.addr, "/pchi/test");
    }

    #[test]
    fn encodes_a_generic_osc_message() {
        let bytes = encode(&Action::new("lighting.look", json!({"look": "blue", "fade": 2, "level": 0.5}))).unwrap();
        let (_, pkt) = rosc::decoder::decode_udp(&bytes).unwrap();
        let OscPacket::Message(m) = pkt else { panic!() };
        assert_eq!(m.addr, "/pchi/lighting/look");
        assert_eq!(m.args, vec![OscType::Int(2), OscType::Float(0.5), OscType::String("blue".into())]); // key order: fade, level, look
    }

    #[tokio::test]
    async fn delivers_over_udp_and_reports_a_device_without_an_address() {
        let rx = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = rx.local_addr().unwrap().port();
        let d = dev("l1", "laser", port);
        let target = deliver(&d, &Action::new("laser.cue", json!({"cue": "sweep"}))).await.unwrap();
        assert_eq!(target, format!("127.0.0.1:{port}"));
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), rx.recv_from(&mut buf)).await.unwrap().unwrap();
        let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        let OscPacket::Message(m) = pkt else { panic!() };
        assert_eq!(m.addr, "/pchi/laser/cue");

        let mut bare = dev("l2", "laser", 1);
        bare.host = None;
        assert!(deliver(&bare, &Action::new("laser.cue", json!({}))).await.unwrap_err().contains("no host and port"));
    }
}
