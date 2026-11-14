//! DMX over the network: Art-Net and sACN (E1.31).
//!
//! A DMX device in the rig (type `artnet` or `sacn`) holds a universe of 512 channels. A lighting cue changes some of them,
//! optionally fading over a time, and the runtime keeps sending the whole universe, because DMX receivers hold a level only
//! while they keep being told: Art-Net nodes and fixtures fall back to dark, and sACN receivers time out after a couple of seconds.
//!
//! A cue's value is written in channels:
//!
//! ```text
//! 1=255                    channel 1 at full
//! 1-3=50%  10=0            channels 1 to 3 at half, channel 10 off
//! 1-8=100% over 2          channels 1 to 8 fade up to full over 2 seconds
//! blackout over 3          every channel fades to 0 over 3 seconds
//! ```
//!
//! Levels not mentioned stay where they are. What this does NOT do: merge with other sources, patch fixtures, or read back anything
//! from the nodes. It is a single sender of one universe per device. It has been tested against its own decoder and over loopback
//! UDP, not against a real Art-Net node, sACN receiver or fixture: that is what `pchi-gearcheck` is for.

use crate::rig::Device;
use pchi_showchart::Action;
use serde_json::Value;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{lookup_host, UdpSocket};
use tokio::sync::Notify;

pub const CHANNELS: usize = 512;
/// How often the universe is re-sent when nothing is changing.
const REFRESH: Duration = Duration::from_millis(500);
/// How often a fade sends a new frame.
const FADE_FRAME: Duration = Duration::from_millis(40);
/// The longest fade a cue may ask for.
const MAX_FADE_S: f64 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Proto {
    ArtNet,
    Sacn,
}

pub fn proto_of(kind: &str) -> Option<Proto> {
    match kind {
        "artnet" => Some(Proto::ArtNet),
        "sacn" => Some(Proto::Sacn),
        _ => None,
    }
}

impl Proto {
    pub fn name(self) -> &'static str {
        match self {
            Proto::ArtNet => "Art-Net",
            Proto::Sacn => "sACN",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Proto::ArtNet => 6454,
            Proto::Sacn => 5568,
        }
    }
    /// Art-Net universes (port-addresses) are 0 to 32767 and usually start at 0; sACN's are 1 to 63999.
    pub fn default_universe(self) -> u32 {
        match self {
            Proto::ArtNet => 0,
            Proto::Sacn => 1,
        }
    }
    pub fn universe_ok(self, u: u32) -> bool {
        match self {
            Proto::ArtNet => u <= 32767,
            Proto::Sacn => (1..=63999).contains(&u),
        }
    }
}

/// The multicast address sACN uses for a universe: 239.255.<high>.<low>.
pub fn sacn_multicast(universe: u16) -> Ipv4Addr {
    Ipv4Addr::new(239, 255, (universe >> 8) as u8, (universe & 0xFF) as u8)
}

/// What a cue asks for: levels to set (channel numbers from 0), and how long to take.
#[derive(Debug, PartialEq)]
pub struct Look {
    pub set: Vec<(usize, u8)>,
    pub fade_s: f64,
}

fn level(s: &str) -> Result<u8, String> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        let v: f64 = p.trim().parse().map_err(|_| format!("'{s}' is not a level"))?;
        if !(0.0..=100.0).contains(&v) {
            return Err(format!("'{s}': a percentage goes from 0 to 100"));
        }
        return Ok((v * 255.0 / 100.0).round() as u8);
    }
    match s.to_lowercase().as_str() {
        "full" => return Ok(255),
        "out" | "off" => return Ok(0),
        _ => {}
    }
    let v: u32 = s.parse().map_err(|_| format!("'{s}' is not a level: use 0 to 255, a percentage like 50%, full or out"))?;
    u8::try_from(v).map_err(|_| format!("'{s}': a level goes from 0 to 255"))
}

/// Read a cue's value for a device: channels, and fixture names if the device has a patch (see the `fixtures` module).
pub fn look_for(dev: &Device, text: &str) -> Result<Look, String> {
    let patch = match dev.patch.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Some(crate::fixtures::Patch::parse(p).map_err(|e| format!("{} patch: {e}", dev.label))?),
        None => None,
    };
    parse_look_with(text, patch.as_ref())
}

/// Read a cue's value as channels only (see the module docs).
pub fn parse_look(text: &str) -> Result<Look, String> {
    parse_look_with(text, None)
}

/// Read a cue's value. With a patch, `wash1=blue`, `walls=40%` and `spot.strobe=fast` also work.
pub fn parse_look_with(text: &str, patch: Option<&crate::fixtures::Patch>) -> Result<Look, String> {
    let how = "use channels like 1=255, 1-3=50% or blackout, optionally ending in 'over 2' (seconds)";
    let t = text.trim();
    if t.is_empty() {
        return Err(format!("no channels: {how}"));
    }
    if t.len() > 400 {
        return Err("that is too long for one cue".into());
    }
    // an optional trailing "over N" / "fade N"
    let lower = t.to_ascii_lowercase();
    let found = ["over", "fade"].iter().filter_map(|w| lower.rfind(&format!(" {w} ")).map(|i| (i, w.len()))).max_by_key(|x| x.0);
    let (body, fade_s) = match found {
        Some((i, wlen)) => {
            let n = t[i + wlen + 2..].trim().trim_end_matches(['s', 'S']).trim();
            let secs: f64 = n.parse().map_err(|_| format!("'{}' after '{}' is not a number of seconds", t[i + wlen + 2..].trim(), &t[i + 1..i + 1 + wlen]))?;
            if !(0.0..=MAX_FADE_S).contains(&secs) {
                return Err(format!("a fade goes from 0 to {MAX_FADE_S} seconds"));
            }
            (&t[..i], secs)
        }
        None => (t, 0.0),
    };
    let mut set: Vec<(usize, u8)> = Vec::new();
    for term in body.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|x| !x.is_empty()) {
        if term.eq_ignore_ascii_case("blackout") {
            set.extend((0..CHANNELS).map(|c| (c, 0)));
            continue;
        }
        let (chans, lv) = term.split_once('=').ok_or_else(|| format!("'{term}': {how}"))?;
        // a name that starts with a letter is a fixture or group (wash1=blue, spot.strobe=fast), not a channel
        if chans.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            let (target, attr) = chans.split_once('.').map_or((chans, None), |(t, a)| (t, Some(a)));
            let patch = patch.ok_or_else(|| format!("'{term}': this device has no patch, so fixture names mean nothing yet: add a patch parameter such as wash1=rgb@1"))?;
            set.extend(patch.resolve(target, attr, lv)?);
            continue;
        }
        let lv = level(lv)?;
        let (a, b) = match chans.split_once('-') {
            Some((a, b)) => (a, b),
            None => (chans, chans),
        };
        let chan = |s: &str| -> Result<usize, String> {
            let n: usize = s.trim().parse().map_err(|_| format!("'{chans}' is not a channel: channels are 1 to {CHANNELS}"))?;
            if (1..=CHANNELS).contains(&n) { Ok(n) } else { Err(format!("channel {n} does not exist: channels are 1 to {CHANNELS}")) }
        };
        let (a, b) = (chan(a)?, chan(b)?);
        if a > b {
            return Err(format!("'{chans}': write the lower channel first"));
        }
        set.extend((a..=b).map(|c| (c - 1, lv)));
    }
    if set.is_empty() {
        return Err(format!("no channels: {how}"));
    }
    Ok(Look { set, fade_s })
}

/// The cue's value as text (its first payload value).
pub fn value_of(action: &Action) -> String {
    match &action.payload {
        Value::Object(p) => p.values().next().and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string()))).unwrap_or_default(),
        _ => String::new(),
    }
}

/// The same check for an action, for the Doors check and rehearsal: `Ok` with a line saying what it will do.
pub fn describe(dev: &Device, action: &Action) -> Result<String, String> {
    let v = value_of(action);
    let look = look_for(dev, &v).map_err(|e| format!("\"{v}\": {e}"))?;
    let mut chans: Vec<usize> = look.set.iter().map(|(c, _)| c + 1).collect();
    chans.sort_unstable();
    chans.dedup();
    Ok(format!("{} channel(s){}", chans.len(), if look.fade_s > 0.0 { format!(" over {} s", look.fade_s) } else { String::new() }))
}

/// An Art-Net ArtDmx packet: "Art-Net", opcode 0x5000, version 14, then the universe's port-address and the data.
pub fn artnet_packet(universe: u16, seq: u8, data: &[u8; CHANNELS]) -> Vec<u8> {
    let mut p = Vec::with_capacity(18 + CHANNELS);
    p.extend_from_slice(b"Art-Net\0");
    p.extend_from_slice(&[0x00, 0x50]); // OpCode ArtDmx, little endian
    p.extend_from_slice(&[0, 14]); // protocol version
    p.push(seq);
    p.push(0); // physical input port
    p.push((universe & 0xFF) as u8); // SubUni
    p.push(((universe >> 8) & 0x7F) as u8); // Net
    p.extend_from_slice(&(CHANNELS as u16).to_be_bytes());
    p.extend_from_slice(data);
    p
}

/// An sACN (ANSI E1.31) data packet for one universe: 638 bytes of root, framing and DMP layers.
pub fn sacn_packet(cid: &[u8; 16], source: &str, universe: u16, priority: u8, seq: u8, data: &[u8; CHANNELS]) -> Vec<u8> {
    let mut p = Vec::with_capacity(638);
    p.extend_from_slice(&[0x00, 0x10, 0x00, 0x00]); // preamble and postamble sizes
    p.extend_from_slice(b"ASC-E1.17\0\0\0");
    p.extend_from_slice(&[0x72, 0x6E]); // root PDU: flags and length 622
    p.extend_from_slice(&[0, 0, 0, 4]); // VECTOR_ROOT_E131_DATA
    p.extend_from_slice(cid);
    p.extend_from_slice(&[0x72, 0x58]); // framing PDU: flags and length 600
    p.extend_from_slice(&[0, 0, 0, 2]); // VECTOR_E131_DATA_PACKET
    let mut name = [0u8; 64];
    for (i, b) in source.bytes().take(63).enumerate() {
        name[i] = b;
    }
    p.extend_from_slice(&name);
    p.push(priority);
    p.extend_from_slice(&[0, 0]); // synchronisation address: none
    p.push(seq);
    p.push(0); // options
    p.extend_from_slice(&universe.to_be_bytes());
    p.extend_from_slice(&[0x72, 0x0B]); // DMP PDU: flags and length 523
    p.push(0x02); // VECTOR_DMP_SET_PROPERTY
    p.push(0xA1); // address and data type
    p.extend_from_slice(&[0, 0, 0, 1]); // first property address 0, increment 1
    p.extend_from_slice(&(CHANNELS as u16 + 1).to_be_bytes()); // property value count: start code and data
    p.push(0); // DMX start code
    p.extend_from_slice(data);
    p
}

struct State {
    base: [u8; CHANNELS],
    fade: Option<Fade>,
}

struct Fade {
    from: [u8; CHANNELS],
    to: [u8; CHANNELS],
    start: Instant,
    dur: Duration,
}

impl State {
    /// The levels right now, finishing a fade that has run its course.
    fn current(&mut self, now: Instant) -> [u8; CHANNELS] {
        if let Some(f) = &self.fade {
            let t = (now.saturating_duration_since(f.start).as_secs_f64() / f.dur.as_secs_f64()).min(1.0);
            if t >= 1.0 {
                self.base = f.to;
                self.fade = None;
            } else {
                let mut out = [0u8; CHANNELS];
                for i in 0..CHANNELS {
                    out[i] = (f.from[i] as f64 + (f.to[i] as f64 - f.from[i] as f64) * t).round() as u8;
                }
                return out;
            }
        }
        self.base
    }
}

/// One DMX device: its universe's levels and the task that keeps sending them.
struct Node {
    state: Arc<Mutex<State>>,
    notify: Arc<Notify>,
    key: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct Dmx {
    nodes: tokio::sync::Mutex<HashMap<String, Arc<Node>>>,
    cid: Mutex<Option<[u8; 16]>>,
}

fn random_cid() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let mut cid = [0u8; 16];
    for chunk in cid.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        chunk.copy_from_slice(&h.finish().to_be_bytes());
    }
    cid[6] = (cid[6] & 0x0F) | 0x40; // a version 4 UUID, as E1.31 asks of a CID
    cid[8] = (cid[8] & 0x3F) | 0x80;
    cid
}

impl Dmx {
    /// Where this device's frames go.
    async fn target(dev: &Device, proto: Proto) -> Result<SocketAddr, String> {
        let universe = dev.universe.unwrap_or(proto.default_universe());
        let port = dev.port.unwrap_or(proto.default_port());
        let host = dev.host.as_deref().map(str::trim).unwrap_or("");
        if host.is_empty() {
            return match proto {
                Proto::Sacn => Ok(SocketAddr::from((sacn_multicast(universe as u16), port))),
                Proto::ArtNet => Err(format!("{} has no host in the rig: give the node's address (or a broadcast address)", dev.label)),
            };
        }
        tokio::time::timeout(Duration::from_millis(1500), lookup_host((host, port)))
            .await
            .map_err(|_| format!("resolving {host} timed out"))?
            .map_err(|e| format!("cannot resolve {host}: {e}"))?
            .next()
            .ok_or_else(|| format!("cannot resolve {host}"))
    }

    /// Apply a lighting cue to a DMX device: change levels (fading if asked) and make sure the universe is being sent.
    /// A `test` action just sends the frame as it stands. Returns where it went.
    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<String, String> {
        let proto = proto_of(&dev.kind).ok_or("not a DMX device")?;
        let universe = dev.universe.unwrap_or(proto.default_universe());
        if !proto.universe_ok(universe) {
            return Err(format!("{}: universe {universe} is not valid for {} ({})", dev.label, proto.name(), if proto == Proto::ArtNet { "0 to 32767" } else { "1 to 63999" }));
        }
        let look = if action.kind == "test" {
            None
        } else if action.kind == "lighting.look" {
            let v = value_of(action);
            Some(look_for(dev, &v).map_err(|e| format!("\"{v}\": {e}"))?)
        } else {
            return Err(format!("{} is a DMX output: it takes lighting cues, not \"{}\"", dev.label, action.kind));
        };
        let target = Self::target(dev, proto).await?;
        let key = format!("{target}|{universe}|{proto:?}");
        let node = {
            let mut nodes = self.nodes.lock().await;
            match nodes.get(&dev.id).filter(|n| n.key == key) {
                Some(n) => Arc::clone(n),
                None => {
                    // the universe's levels survive a change of address: they are the show's, not the node's
                    let carry = nodes.get(&dev.id).map(|n| n.state.lock().unwrap().base).unwrap_or([0; CHANNELS]);
                    let n = Arc::new(self.spawn(dev, proto, universe as u16, target, key, carry).await?);
                    nodes.insert(dev.id.clone(), Arc::clone(&n));
                    n
                }
            }
        };
        if let Some(look) = look {
            let mut st = node.state.lock().unwrap();
            let now = Instant::now();
            let from = st.current(now);
            let mut to = from;
            for (c, v) in &look.set {
                to[*c] = *v;
            }
            if look.fade_s > 0.0 {
                st.fade = Some(Fade { from, to, start: now, dur: Duration::from_secs_f64(look.fade_s) });
            } else {
                st.base = to;
                st.fade = None;
            }
        }
        node.notify.notify_one();
        Ok(target.to_string())
    }

    async fn spawn(&self, dev: &Device, proto: Proto, universe: u16, target: SocketAddr, key: String, carry: [u8; CHANNELS]) -> Result<Node, String> {
        let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|e| format!("cannot open a socket for {}: {e}", dev.label))?;
        socket.set_broadcast(true).ok();
        if matches!(target, SocketAddr::V4(a) if a.ip().is_multicast()) {
            socket.set_multicast_ttl_v4(8).ok();
        }
        let cid = *self.cid.lock().unwrap().get_or_insert_with(random_cid);
        let state = Arc::new(Mutex::new(State { base: carry, fade: None }));
        let notify = Arc::new(Notify::new());
        let (st, nt, name) = (Arc::clone(&state), Arc::clone(&notify), format!("PCHI {}", dev.label));
        let task = tokio::spawn(async move {
            let mut seq: u8 = 0;
            loop {
                let (frame, fading) = {
                    let mut s = st.lock().unwrap();
                    let f = s.current(Instant::now());
                    (f, s.fade.is_some())
                };
                seq = if proto == Proto::ArtNet { if seq == 255 { 1 } else { seq + 1 } } else { seq.wrapping_add(1) };
                let packet = match proto {
                    Proto::ArtNet => artnet_packet(universe, seq, &frame),
                    Proto::Sacn => sacn_packet(&cid, &name, universe, 100, seq, &frame),
                };
                let _ = socket.send_to(&packet, target).await;
                tokio::select! {
                    _ = nt.notified() => {}
                    _ = tokio::time::sleep(if fading { FADE_FRAME } else { REFRESH }) => {}
                }
            }
        });
        Ok(Node { state, notify, key, task })
    }

    /// The levels a device is sending right now (for tests and the Doors check).
    pub async fn levels(&self, id: &str) -> Option<[u8; CHANNELS]> {
        let n = self.nodes.lock().await.get(id).cloned()?;
        let mut s = n.state.lock().unwrap();
        Some(s.current(Instant::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dev(kind: &str, host: Option<&str>, port: Option<u16>, universe: Option<u32>) -> Device {
        Device { id: "d1".into(), label: "Rack".into(), kind: kind.into(), dept: "lighting".into(), host: host.map(String::from), port, listen_port: None, profile: None, universe, patch: None }
    }

    fn look(v: &str) -> Action {
        Action::new("lighting.look", json!({ "look": v }))
    }

    #[test]
    fn a_cues_channels_are_read_in_plain_words() {
        let p = |t: &str| parse_look(t).unwrap();
        assert_eq!(p("1=255").set, vec![(0, 255)]);
        assert_eq!(p("1-3=50%  10=0").set, vec![(0, 128), (1, 128), (2, 128), (9, 0)], "50% is 128 of 255");
        assert_eq!(p("1=full,2=10;3=0").set, vec![(0, 255), (1, 10), (2, 0)], "commas, semicolons and spaces all separate terms");
        assert_eq!(p("5=out 6=off").set, vec![(4, 0), (5, 0)]);
        let fade = p("1-8=100% over 2");
        assert_eq!((fade.set.len(), fade.fade_s), (8, 2.0));
        assert_eq!(p("blackout over 3s").fade_s, 3.0);
        assert_eq!(p("blackout").set.len(), 512);
        assert_eq!(p("1=255 fade 1.5").fade_s, 1.5);
    }

    #[test]
    fn a_bad_cue_says_what_to_fix() {
        let e = |t: &str| parse_look(t).unwrap_err();
        assert!(e("").contains("no channels"));
        assert!(e("0=255").contains("channel 0 does not exist"));
        assert!(e("513=1").contains("channel 513 does not exist"));
        assert!(e("1=256").contains("0 to 255"));
        assert!(e("1=150%").contains("0 to 100"));
        assert!(e("1=bright").contains("not a level"));
        assert!(e("8-3=10").contains("lower channel first"));
        assert!(e("hello").contains("use channels like"));
        assert!(e("1=255 over soon").contains("not a number of seconds"));
        assert!(e("1=255 over 9999").contains("fade goes from 0 to 600"));
        assert!(describe(&dev("artnet", None, None, None), &look("1-3=10")).unwrap().starts_with("3 channel(s)"));
        assert!(describe(&dev("artnet", None, None, None), &look("x")).unwrap_err().contains("\"x\""));
    }

    #[test]
    fn an_artnet_packet_is_laid_out_as_the_standard_says() {
        let mut data = [0u8; CHANNELS];
        data[0] = 255;
        data[511] = 7;
        let p = artnet_packet(0x1234, 9, &data);
        assert_eq!(p.len(), 18 + 512);
        assert_eq!(&p[0..8], b"Art-Net\0");
        assert_eq!(&p[8..10], &[0x00, 0x50], "OpCode ArtDmx 0x5000, low byte first");
        assert_eq!(&p[10..12], &[0, 14], "protocol version 14");
        assert_eq!((p[12], p[13]), (9, 0), "sequence, physical port");
        assert_eq!((p[14], p[15]), (0x34, 0x12), "SubUni is the low byte of the port-address, Net the high");
        assert_eq!(&p[16..18], &[0x02, 0x00], "length 512, big endian");
        assert_eq!((p[18], p[18 + 511]), (255, 7));
    }

    #[test]
    fn an_sacn_packet_is_laid_out_as_e131_says() {
        let cid = [0xAB; 16];
        let mut data = [0u8; CHANNELS];
        data[0] = 200;
        let p = sacn_packet(&cid, "PCHI Rack", 0x0102, 100, 42, &data);
        assert_eq!(p.len(), 638, "a full universe is 638 bytes");
        assert_eq!(&p[0..4], &[0, 0x10, 0, 0]);
        assert_eq!(&p[4..16], b"ASC-E1.17\0\0\0");
        assert_eq!(&p[16..18], &[0x72, 0x6E], "root PDU flags and length 622");
        assert_eq!(&p[18..22], &[0, 0, 0, 4]);
        assert_eq!(&p[22..38], &cid);
        assert_eq!(&p[38..40], &[0x72, 0x58], "framing PDU flags and length 600");
        assert_eq!(&p[40..44], &[0, 0, 0, 2]);
        assert_eq!(&p[44..53], b"PCHI Rack");
        assert_eq!(p[53], 0, "the source name is padded with zeros to 64 bytes");
        assert_eq!(p[108], 100, "priority");
        assert_eq!(p[111], 42, "sequence");
        assert_eq!(&p[113..115], &[0x01, 0x02], "universe");
        assert_eq!(&p[115..117], &[0x72, 0x0B], "DMP PDU flags and length 523");
        assert_eq!((p[117], p[118]), (0x02, 0xA1));
        assert_eq!(&p[119..125], &[0, 0, 0, 1, 0x02, 0x01], "first address 0, increment 1, 513 values");
        assert_eq!((p[125], p[126]), (0, 200), "start code 0, then channel 1");
        assert_eq!(sacn_multicast(0x0102), Ipv4Addr::new(239, 255, 1, 2));
        // the CID the runtime makes is a version 4 UUID
        let c = random_cid();
        assert_eq!((c[6] >> 4, c[8] >> 6), (4, 2));
    }

    async fn listen() -> (UdpSocket, u16) {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = s.local_addr().unwrap().port();
        (s, port)
    }

    async fn frames(s: &UdpSocket, n: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 2048];
        for _ in 0..n {
            let (len, _) = tokio::time::timeout(Duration::from_secs(2), s.recv_from(&mut buf)).await.expect("a frame arrives").unwrap();
            out.push(buf[..len].to_vec());
        }
        out
    }

    #[tokio::test]
    async fn a_look_is_sent_at_once_and_kept_alive() {
        let (sock, port) = listen().await;
        let dmx = Dmx::default();
        let d = dev("artnet", Some("127.0.0.1"), Some(port), Some(3));
        dmx.apply(&d, &look("1=255 4-5=50%")).await.unwrap();
        let first = &frames(&sock, 1).await[0];
        assert_eq!((&first[0..8], first[14], first[18], first[21], first[22]), (&b"Art-Net\0"[..], 3, 255, 128, 128), "universe 3 with the levels set");
        // with nothing changing, the same frame keeps coming (the keepalive), with the sequence moving on
        let more = frames(&sock, 2).await;
        assert!(more.iter().all(|f| f[18] == 255 && f[21] == 128));
        assert!(more[1][12] != more[0][12], "the sequence advances");
        // a second cue changes only what it names
        dmx.apply(&d, &look("1=0")).await.unwrap();
        let lv = dmx.levels("d1").await.unwrap();
        assert_eq!((lv[0], lv[3], lv[4]), (0, 128, 128), "the other levels stay");
    }

    #[tokio::test]
    async fn a_fade_goes_through_the_levels_between() {
        let (sock, port) = listen().await;
        let dmx = Dmx::default();
        let d = dev("sacn", Some("127.0.0.1"), Some(port), Some(1));
        dmx.apply(&d, &look("1=0")).await.unwrap();
        let _ = frames(&sock, 1).await;
        dmx.apply(&d, &look("1=200 over 0.4")).await.unwrap();
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_millis(900);
        let mut buf = vec![0u8; 2048];
        while Instant::now() < end {
            if let Ok(Ok((len, _))) = tokio::time::timeout(Duration::from_millis(100), sock.recv_from(&mut buf)).await {
                assert_eq!(len, 638);
                seen.push(buf[126]);
            }
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "it only goes up: {seen:?}");
        assert!(seen.iter().any(|&v| v > 0 && v < 200), "it passes through the middle: {seen:?}");
        assert_eq!(*seen.last().unwrap(), 200, "and arrives");
        assert_eq!(dmx.levels("d1").await.unwrap()[0], 200);
    }

    #[tokio::test]
    async fn a_dmx_device_refuses_what_it_cannot_do() {
        let dmx = Dmx::default();
        let d = dev("artnet", Some("127.0.0.1"), Some(9), Some(0));
        assert!(dmx.apply(&d, &Action::new("scene.activate", json!({"scene": "1"}))).await.unwrap_err().contains("takes lighting cues"));
        assert!(dmx.apply(&d, &look("999=1")).await.unwrap_err().contains("does not exist"));
        assert!(dmx.apply(&dev("sacn", Some("127.0.0.1"), Some(9), Some(0)), &look("1=1")).await.unwrap_err().contains("not valid for sACN"));
        assert!(dmx.apply(&dev("artnet", Some("127.0.0.1"), Some(9), Some(40000)), &look("1=1")).await.unwrap_err().contains("not valid for Art-Net"));
        assert!(dmx.apply(&dev("artnet", None, None, None), &look("1=1")).await.unwrap_err().contains("no host"));
        // an sACN device with no host goes to the universe's multicast address
        assert_eq!(Dmx::target(&dev("sacn", None, None, Some(258)), Proto::Sacn).await.unwrap().to_string(), "239.255.1.2:5568");
    }

    #[test]
    fn a_cue_can_name_fixtures_when_the_device_has_a_patch() {
        let mut dv = dev("artnet", Some("127.0.0.1"), Some(9), Some(0));
        dv.patch = Some("wash1=rgbw@1, wash2=rgbw@5, lamp=dimmer@20, walls=wash1+wash2".into());
        let at = |l: &Look, ch: usize| l.set.iter().rev().find(|(c, _)| *c == ch - 1).map(|x| x.1); // the last term for a channel wins
        let l = look_for(&dv, "wash1=blue lamp=40% walls.white=100% 30=255 over 2").unwrap();
        assert_eq!((at(&l, 3), at(&l, 1), at(&l, 20), at(&l, 4), at(&l, 8), at(&l, 30), l.fade_s), (Some(255), Some(0), Some(102), Some(255), Some(255), Some(255), 2.0), "colors, a level, a group's attribute, a raw channel and a fade in one cue");
        // without a patch, fixture names mean nothing, and it says how to fix that
        let e = look_for(&dev("artnet", None, None, None), "wash1=blue").unwrap_err();
        assert!(e.contains("no patch") && e.contains("wash1=rgb@1"), "{e}");
        // a wrong patch is named as the device's, before any cue is sent
        let mut bad = dev("artnet", None, None, None);
        bad.label = "Rack".into();
        bad.patch = Some("a=rgbw@1, b=rgb@3".into());
        assert!(look_for(&bad, "1=1").unwrap_err().contains("Rack patch:") && look_for(&bad, "1=1").unwrap_err().contains("overlaps"));
        // blackout still works
        assert_eq!(look_for(&dv, "blackout").unwrap().set.len(), 512);
        assert!(describe(&dv, &look("wash2=red")).unwrap().starts_with("4 channel(s)"), "red sets red, green, blue and clears white: four channels");
    }
}
