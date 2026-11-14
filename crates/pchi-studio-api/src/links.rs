//! Listening to the tools we send to.
//!
//! Each rig device that is sent to (or expected to talk back) gets ONE UDP socket that is kept open.
//! Tools that answer "to whoever sent the request" (the X32 does) reach us because it is the same socket,
//! and tools that push feedback to a port you set in them (Eos, QLab) reach us on the device's
//! `listen_port`. Everything heard from the device's address is decoded and published as a
//! `feedback` message, and the time it was last heard feeds the device's *link* state (see `output`).
//!
//! Link is not READY. A mixer that answers a ping is on the network; whether the department is ready for
//! the show is still the people's word.

use crate::rig::Device;
use rosc::{decoder, OscPacket, OscType};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{lookup_host, UdpSocket};

/// Where decoded feedback goes (the show publishes it to every listener).
pub type Sink = Arc<dyn Fn(Value) + Send + Sync>;

/// At most this many feedback messages per device per second are published; a tool that streams
/// everything (a console with meters on) must not flood the feed. The rest are counted, not shown.
const MAX_PER_SEC: u32 = 20;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub struct Link {
    pub socket: Arc<UdpSocket>,
    pub target: SocketAddr,
    key: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct Links {
    map: tokio::sync::Mutex<HashMap<String, Arc<Link>>>,
    heard: Mutex<HashMap<String, Arc<AtomicI64>>>,
    /// Read on every datagram, so a sink set after a link opened still gets its feedback.
    sink: Arc<Mutex<Option<Sink>>>,
}

impl Links {
    pub fn set_sink(&self, s: Sink) {
        *self.sink.lock().unwrap() = Some(s);
    }

    /// When this device was last heard from (ms since epoch), 0 if never.
    pub fn last_heard(&self, id: &str) -> i64 {
        self.heard.lock().unwrap().get(id).map_or(0, |a| a.load(Ordering::Relaxed))
    }

    /// The device's socket, opened on first use. It is reopened if the device's address or listen port changed.
    pub async fn get(&self, dev: &Device) -> Result<Arc<Link>, String> {
        let addr = dev.address().ok_or_else(|| format!("{} has no host and port in the rig", dev.label))?;
        let key = format!("{addr}|{:?}", dev.listen_port);
        let mut map = self.map.lock().await;
        if let Some(l) = map.get(&dev.id).filter(|l| l.key == key) {
            return Ok(Arc::clone(l));
        }
        let target = tokio::time::timeout(Duration::from_millis(1500), lookup_host(&addr))
            .await
            .map_err(|_| format!("resolving {addr} timed out"))?
            .map_err(|e| format!("cannot resolve {addr}: {e}"))?
            .next()
            .ok_or_else(|| format!("cannot resolve {addr}"))?;
        let local = match (target.is_ipv6(), dev.listen_port) {
            (true, p) => format!("[::]:{}", p.unwrap_or(0)),
            (false, p) => format!("0.0.0.0:{}", p.unwrap_or(0)),
        };
        let socket = Arc::new(UdpSocket::bind(&local).await.map_err(|e| match dev.listen_port {
            Some(p) => format!("{} cannot listen on port {p}: {e} (is another device or program using it?)", dev.label),
            None => format!("cannot open a socket: {e}"),
        })?);
        let heard = Arc::clone(self.heard.lock().unwrap().entry(dev.id.clone()).or_default());
        let sink = Arc::clone(&self.sink);
        let task = tokio::spawn(listen(Arc::clone(&socket), target.ip(), dev.id.clone(), dev.label.clone(), heard, sink));
        let link = Arc::new(Link { socket, target, key, task });
        map.insert(dev.id.clone(), Arc::clone(&link));
        Ok(link)
    }
}

/// Read everything the device sends back. Datagrams from other addresses are ignored.
async fn listen(socket: Arc<UdpSocket>, from_ip: IpAddr, id: String, label: String, heard: Arc<AtomicI64>, sink: Arc<Mutex<Option<Sink>>>) {
    let mut buf = vec![0u8; 8192];
    let (mut window, mut count, mut dropped) = (0i64, 0u32, 0u64);
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
        if from.ip() != from_ip {
            continue;
        }
        let now = now_ms();
        heard.store(now, Ordering::Relaxed);
        let Some(sink) = sink.lock().unwrap().clone() else { continue };
        if now - window >= 1000 {
            window = now;
            count = 0;
        }
        for (address, args) in flatten(&buf[..n]) {
            if count >= MAX_PER_SEC {
                dropped += 1;
                continue;
            }
            count += 1;
            let mut m = json!({"type": "feedback", "ts": now, "device": id, "label": label, "address": address, "args": args});
            if dropped > 0 {
                m["dropped"] = json!(dropped);
                dropped = 0;
            }
            sink(m);
        }
    }
}

/// The messages in an OSC packet (bundles are opened), as (address, arguments).
pub fn flatten(bytes: &[u8]) -> Vec<(String, Vec<Value>)> {
    fn walk(p: OscPacket, out: &mut Vec<(String, Vec<Value>)>) {
        match p {
            OscPacket::Message(m) => out.push((m.addr, m.args.iter().map(arg).collect())),
            OscPacket::Bundle(b) => b.content.into_iter().for_each(|c| walk(c, out)),
        }
    }
    let mut out = Vec::new();
    if let Ok((_, p)) = decoder::decode_udp(bytes) {
        walk(p, &mut out);
    }
    out
}

fn arg(a: &OscType) -> Value {
    match a {
        OscType::Int(i) => json!(i),
        OscType::Long(i) => json!(i),
        OscType::Float(f) => json!(f),
        OscType::Double(f) => json!(f),
        OscType::String(s) => json!(s),
        OscType::Bool(b) => json!(b),
        OscType::Blob(b) => json!(format!("<{} bytes>", b.len())),
        other => json!(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{encoder, OscMessage};

    fn packet(addr: &str, args: Vec<OscType>) -> Vec<u8> {
        encoder::encode(&OscPacket::Message(OscMessage { addr: addr.into(), args })).unwrap()
    }

    #[test]
    fn flatten_reads_messages_and_opens_bundles() {
        let m = flatten(&packet("/info", vec![OscType::String("X32".into()), OscType::Int(3), OscType::Float(0.5)]));
        assert_eq!(m, vec![("/info".to_string(), vec![json!("X32"), json!(3), json!(0.5)])]);
        let bundle = encoder::encode(&OscPacket::Bundle(rosc::OscBundle {
            timetag: (0, 1).into(),
            content: vec![OscPacket::Message(OscMessage { addr: "/a".into(), args: vec![] }), OscPacket::Message(OscMessage { addr: "/b".into(), args: vec![OscType::Bool(true)] })],
        }))
        .unwrap();
        assert_eq!(flatten(&bundle).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["/a", "/b"]);
        assert!(flatten(b"not osc").is_empty(), "garbage is ignored, not an error");
    }
}
