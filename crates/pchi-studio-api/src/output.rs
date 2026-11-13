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
use rosc::{encoder, OscMessage, OscPacket, OscType};
use serde_json::Value;
use std::time::Duration;
use tokio::net::{lookup_host, UdpSocket};

const SEND_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Output {
    devices: Vec<Device>,
    send: bool,
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
        Self { devices, send }
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

/// Send one action to a device. Returns the `host:port` it went to.
pub async fn deliver(dev: &Device, action: &Action) -> Result<String, String> {
    let addr = dev.address().ok_or_else(|| format!("{} has no host and port in the rig", dev.label))?;
    let bytes = encode(action)?;
    let go = async {
        let target = lookup_host(&addr).await.map_err(|e| format!("cannot resolve {addr}: {e}"))?.next().ok_or_else(|| format!("cannot resolve {addr}"))?;
        let bind = if target.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let sock = UdpSocket::bind(bind).await.map_err(|e| format!("cannot open a socket: {e}"))?;
        sock.send_to(&bytes, target).await.map_err(|e| format!("send to {addr} failed: {e}"))?;
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
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(port) }
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
