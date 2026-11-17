//! A digital twin of the rig: stand-ins for the devices, on loopback, that take what the runtime sends and write it down.
//!
//! The runtime can run against these with `--send` and nothing real is involved. Each twin device is one UDP socket. It decodes what
//! arrives (OSC, or the header of an Art-Net or sACN packet), records it with the time, and can be made to misbehave:
//!
//! * `Down`: it stops answering. Packets still arrive (UDP does not know) and are recorded as *not acted on*, which is exactly
//!   the difference between "sent" and "the desk did it".
//! * `Lag(ms)`: it acts, but that long late.
//! * `Healthy` again.
//!
//! A healthy device whose tool answers a probe (the X32 does) answers it, so the runtime's link state moves as it would with the
//! tool. A twin can also report a department READY or on hold to a runtime, as a person at that desk would.
//!
//! What a twin is not: it does not know what any tool *does* with a message. It proves what PCHI put on the wire, and that the
//! runtime copes when a device goes quiet. It does not prove a console reacts, and it cannot make a tool's real OSC dialect correct.

use crate::links::flatten;
use crate::rig::Device;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fault {
    Healthy,
    /// Stops acting and answering.
    Down,
    /// Acts this many milliseconds late.
    Lag(u64),
}

impl Fault {
    /// `healthy`, `down` or `lag:500`.
    pub fn parse(s: &str) -> Result<Fault, String> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "healthy" | "ok" | "up" => Ok(Fault::Healthy),
            "down" | "dead" => Ok(Fault::Down),
            _ => match s.strip_prefix("lag:").and_then(|n| n.trim().parse::<u64>().ok()) {
                Some(ms) => Ok(Fault::Lag(ms)),
                None => Err(format!("'{s}' is not a fault: use healthy, down or lag:<milliseconds>")),
            },
        }
    }
    pub fn text(&self) -> String {
        match self {
            Fault::Healthy => "healthy".into(),
            Fault::Down => "down".into(),
            Fault::Lag(ms) => format!("lag:{ms}"),
        }
    }
}

/// One message a twin device received.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// When it arrived (ms since the epoch).
    pub at_ms: i64,
    /// When the device acted on it: `at_ms` plus any lag; `None` while the device was down.
    pub acted_ms: Option<i64>,
    pub address: String,
    pub args: Vec<Value>,
}

impl Seen {
    pub fn json(&self) -> Value {
        json!({"at_ms": self.at_ms, "acted_ms": self.acted_ms, "address": self.address, "args": self.args})
    }
}

struct Dev {
    fault: Fault,
    seen: Vec<Seen>,
    kind: String,
    dept: String,
    label: String,
}

pub struct Twin {
    /// Pretend controllers for devices driven by a text interface (a UR): they answer like the real thing and write down every command.
    urs: BTreeMap<String, crate::ursim::UrSim>,
    devs: Arc<Mutex<BTreeMap<String, Dev>>>,
    rig: Vec<Device>,
    skipped: Vec<(String, String)>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Twin {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// What a UDP datagram is, for devices reached by DMX: the protocol and universe, not the channels.
fn dmx_summary(kind: &str, bytes: &[u8]) -> (String, Vec<Value>) {
    if kind == "artnet" && bytes.len() >= 18 && bytes.starts_with(b"Art-Net\0") {
        let universe = u16::from(bytes[14]) | (u16::from(bytes[15]) << 8);
        return ("artnet/dmx".into(), vec![json!(universe), json!(bytes.len().saturating_sub(18))]);
    }
    if kind == "sacn" && bytes.len() >= 126 {
        let universe = (u16::from(bytes[113]) << 8) | u16::from(bytes[114]);
        return ("sacn/dmx".into(), vec![json!(universe), json!(bytes.len().saturating_sub(126))]);
    }
    (format!("{kind}/unknown"), vec![json!(bytes.len())])
}

impl Twin {
    /// Stand up one twin for every device that is reached by UDP (OSC, Art-Net, sACN). OBS is reached by a websocket and is left out, and
    /// listed in [`Twin::skipped`]. Each twin listens on a free loopback port; [`Twin::rig`] is the same rig pointed at them.
    pub async fn start(devices: &[Device]) -> Result<Twin, String> {
        let devs: Arc<Mutex<BTreeMap<String, Dev>>> = Arc::default();
        let (mut rig, mut skipped, mut tasks) = (Vec::new(), Vec::new(), Vec::new());
        let mut urs: BTreeMap<String, crate::ursim::UrSim> = BTreeMap::new();
        for d in devices {
            if d.is_link() && d.kind == "ur" {
                // a Universal Robots controller: the pretend one has the device's allowed programs "on the robot"
                let names = crate::textlink::allowed_programs(d);
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let sim = crate::ursim::UrSim::start(&refs).await?;
                devs.lock().unwrap().insert(d.id.clone(), Dev { fault: Fault::Healthy, seen: Vec::new(), kind: d.kind.clone(), dept: d.dept.clone(), label: d.label.clone() });
                let mut twinned = d.clone();
                twinned.host = Some("127.0.0.1".into());
                twinned.port = Some(sim.port);
                twinned.listen_port = None;
                rig.push(twinned);
                urs.insert(d.id.clone(), sim);
                continue;
            }
            if d.kind == "obs" {
                skipped.push((d.id.clone(), "OBS is reached by a websocket: it has no twin".to_string()));
                rig.push(d.clone());
                continue;
            }
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.map_err(|e| format!("cannot open a socket for {}: {e}", d.id))?);
            let port = socket.local_addr().map_err(|e| e.to_string())?.port();
            devs.lock().unwrap().insert(d.id.clone(), Dev { fault: Fault::Healthy, seen: Vec::new(), kind: d.kind.clone(), dept: d.dept.clone(), label: d.label.clone() });
            let mut twinned = d.clone();
            twinned.host = Some("127.0.0.1".into());
            twinned.port = Some(port);
            twinned.listen_port = None;
            let answers = crate::profiles::probe(d.profile_key()).is_some();
            rig.push(twinned);
            let (id, kind, map) = (d.id.clone(), d.kind.clone(), Arc::clone(&devs));
            tasks.push(tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                loop {
                    let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
                    let at = now_ms();
                    let msgs = {
                        let m = flatten(&buf[..n]);
                        if m.is_empty() {
                            let (a, args) = dmx_summary(&kind, &buf[..n]);
                            vec![(a, args)]
                        } else {
                            m
                        }
                    };
                    let fault = {
                        let mut g = map.lock().unwrap();
                        let Some(dev) = g.get_mut(&id) else { continue };
                        let acted = match dev.fault {
                            Fault::Healthy => Some(at),
                            Fault::Lag(ms) => Some(at + ms as i64),
                            Fault::Down => None,
                        };
                        for (address, args) in msgs {
                            dev.seen.push(Seen { at_ms: at, acted_ms: acted, address, args });
                        }
                        dev.fault
                    };
                    if answers && fault == Fault::Healthy {
                        // the tool says it is there: any datagram from it marks the link as heard
                        let reply = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: "/info".into(), args: vec![rosc::OscType::String("pchi twin".into())] })).unwrap_or_default();
                        let _ = socket.send_to(&reply, from).await;
                    }
                }
            }));
        }
        Ok(Twin { urs, devs, rig, skipped, tasks })
    }

    /// The rig with every twinned device's host and port pointed at its twin (and no listen port, so nothing else is bound).
    pub fn rig(&self) -> Vec<Device> {
        self.rig.clone()
    }

    /// Devices that have no twin, and why.
    pub fn skipped(&self) -> &[(String, String)] {
        &self.skipped
    }

    /// A fault only a pretend controller has (`protective-stop`, `local-control`, `refuse-play`): see `ursim`.
    pub fn set_named_fault(&self, id: &str, name: &str) -> Result<(), String> {
        match self.urs.get(id) {
            Some(sim) => sim.fault(name),
            None => Err(format!("'{name}' is not a fault: use healthy, down or lag:<milliseconds> (a UR twin also takes protective-stop, local-control and refuse-play)")),
        }
    }

    pub fn set_fault(&self, id: &str, fault: Fault) -> Result<(), String> {
        if let Some(sim) = self.urs.get(id) {
            return match fault {
                Fault::Healthy => sim.fault("healthy"),
                Fault::Down => sim.fault("down"),
                Fault::Lag(_) => Err("a UR twin does not lag: use down, protective-stop, local-control or refuse-play".into()),
            };
        }
        let mut g = self.devs.lock().unwrap();
        g.get_mut(id).map(|d| d.fault = fault).ok_or_else(|| format!("no twin device '{id}'"))
    }

    pub fn fault(&self, id: &str) -> Option<Fault> {
        self.devs.lock().unwrap().get(id).map(|d| d.fault)
    }

    /// Everything one device has received, oldest first.
    pub fn seen(&self, id: &str) -> Vec<Seen> {
        if let Some(sim) = self.urs.get(id) {
            // a UR twin writes down commands, not OSC: each is shown as one message whose address is the command text
            let at = now_ms();
            return sim.seen().into_iter().map(|c| Seen { at_ms: at, acted_ms: Some(at), address: c, args: Vec::new() }).collect();
        }
        self.devs.lock().unwrap().get(id).map(|d| d.seen.clone()).unwrap_or_default()
    }

    /// What the device actually *did* something about: received while it was not down.
    pub fn acted(&self, id: &str) -> Vec<Seen> {
        self.seen(id).into_iter().filter(|s| s.acted_ms.is_some()).collect()
    }

    pub fn clear(&self) {
        for sim in self.urs.values() {
            sim.with(|s| s.seen.clear());
        }
        for d in self.devs.lock().unwrap().values_mut() {
            d.seen.clear();
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.devs.lock().unwrap().keys().cloned().collect()
    }

    /// Every device with its state, as JSON (the twin's own report).
    pub fn json(&self) -> Value {
        let g = self.devs.lock().unwrap();
        json!(g.iter().map(|(id, d)| match self.urs.get(id) {
            Some(sim) => {
                let st = sim.snapshot();
                json!({"id": id, "label": d.label, "type": d.kind, "dept": d.dept, "fault": if st.down { "down".to_string() } else if st.safety != "NORMAL" { st.safety.to_lowercase().replace('_', "-") } else if !st.remote { "local-control".to_string() } else if st.refuse_play { "refuse-play".to_string() } else { "healthy".to_string() }, "received": st.seen.len(), "acted": st.seen.len(), "program": st.program, "loaded": st.loaded})
            }
            None => json!({"id": id, "label": d.label, "type": d.kind, "dept": d.dept, "fault": d.fault.text(), "received": d.seen.len(), "acted": d.seen.iter().filter(|s| s.acted_ms.is_some()).count()}),
        }).collect::<Vec<_>>())
    }

    /// Say, to a runtime, that a department is standing by (or on hold), as the person at that desk would.
    pub async fn report_ready(runtime: &str, token: Option<&str>, dept: &str, ready: bool, note: &str) -> Result<(), String> {
        let mut headers: Vec<(&str, String)> = Vec::new();
        if let Some(t) = token {
            headers.push(("Authorization", format!("Bearer {t}")));
        }
        let hs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (status, v) = crate::failover::http(runtime, "POST", &format!("/ready/{dept}"), &hs, Some(&json!({"ready": ready, "note": note}).to_string())).await?;
        if status == 200 {
            Ok(())
        } else {
            Err(format!("the runtime said {status}: {}", v["error"].as_str().unwrap_or("")))
        }
    }
}

/// Point a show file's devices at a twin's loopback ports: the same file, with each twinned node's `host` and `port` changed and
/// its `listen_port` removed. Returns the new text.
pub fn rewrite_show(text: &str, twin: &Twin) -> Result<String, String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let nodes = v.get_mut("nodes").and_then(Value::as_array_mut).ok_or("this file has no rig (no nodes): open it in Studio and Save as .pchi")?;
    for d in twin.rig() {
        let Some(n) = nodes.iter_mut().find(|n| n["id"] == d.id) else { continue };
        if twin.skipped().iter().any(|(id, _)| *id == d.id) {
            continue;
        }
        n["params"]["host"] = json!("127.0.0.1");
        n["params"]["port"] = json!(d.port);
        if let Some(p) = n["params"].as_object_mut() {
            p.remove("listen_port");
        }
    }
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Output;
    use pchi_showchart::Action;

    fn dev(id: &str, kind: &str) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: crate::rig::dept_of_type(kind).unwrap().into(), host: Some("10.9.9.9".into()), port: Some(1), listen_port: Some(9999), profile: None, universe: None, patch: None, password: None, programs: None }
    }

    #[test]
    fn faults_parse_and_say_what_is_wrong() {
        assert_eq!(Fault::parse("down").unwrap(), Fault::Down);
        assert_eq!(Fault::parse("Lag: 250").unwrap(), Fault::Lag(250));
        assert_eq!(Fault::parse("healthy").unwrap(), Fault::Healthy);
        assert!(Fault::parse("broken").unwrap_err().contains("lag:<milliseconds>"));
        assert_eq!(Fault::Lag(7).text(), "lag:7");
    }

    #[tokio::test]
    async fn a_twin_takes_what_the_runtime_sends_and_a_dead_one_does_not_act() {
        let twin = Twin::start(&[dev("l1", "lighting-desk"), dev("o1", "obs")]).await.unwrap();
        assert_eq!(twin.skipped().len(), 1, "OBS has no twin");
        let rig = twin.rig();
        assert_eq!(rig[0].host.as_deref(), Some("127.0.0.1"));
        assert_ne!(rig[0].port, Some(1));
        assert_eq!(rig[0].listen_port, None);

        let out = Output::new(rig.clone(), true);
        let action = Action::new("lighting.look", json!({"look": "warm"}));
        out.deliver(&rig[0], &action).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let got = twin.acted("l1");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].address, "/pchi/lighting/look");
        assert_eq!(got[0].args, vec![json!("warm")]);

        // down: UDP still "sends", the packet arrives, nothing acts on it
        twin.set_fault("l1", Fault::Down).unwrap();
        out.deliver(&rig[0], &action).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(twin.seen("l1").len(), 2, "it arrived");
        assert_eq!(twin.acted("l1").len(), 1, "but nobody acted on it");

        twin.set_fault("l1", Fault::Lag(400)).unwrap();
        out.deliver(&rig[0], &action).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let last = twin.seen("l1").pop().unwrap();
        assert_eq!(last.acted_ms.unwrap() - last.at_ms, 400);
        assert!(twin.set_fault("nope", Fault::Down).is_err());
    }

    #[test]
    fn dmx_packets_are_named_by_protocol_and_universe() {
        let mut art = b"Art-Net\0".to_vec();
        art.extend_from_slice(&[0x00, 0x50, 0, 14, 0, 0, 3, 0, 0, 4]);
        art.extend_from_slice(&[0u8; 4]);
        let (a, args) = dmx_summary("artnet", &art);
        assert_eq!(a, "artnet/dmx");
        assert_eq!(args[0], json!(3));
        assert_eq!(dmx_summary("artnet", b"xx").0, "artnet/unknown");
    }

    #[tokio::test]
    async fn rewriting_a_show_points_each_device_at_its_twin() {
        let text = r#"{"version":"1.0","nodes":[{"id":"l1","type":"lighting-desk","label":"Desk","position":{"x":0,"y":0},"params":{"host":"192.168.1.5","port":7000,"listen_port":8000}}],
            "showchart":{"initial":"A","states":[{"id":"A"}],"transitions":[]}}"#;
        let (_, rig) = crate::rig::load(text).unwrap();
        let twin = Twin::start(&rig.unwrap()).await.unwrap();
        let out = rewrite_show(text, &twin).unwrap();
        let (_, rig) = crate::rig::load(&out).unwrap();
        let d = &rig.unwrap()[0];
        assert_eq!(d.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(d.port, twin.rig()[0].port);
        assert_eq!(d.listen_port, None);
        assert!(rewrite_show(r#"{"initial":"A","states":[],"transitions":[]}"#, &twin).is_err(), "a bare showchart has no rig to point");
    }
}
