//! OSC input: a Stream Deck, a tablet, a button box or another program presses things in the show.
//!
//! An input listens on a UDP port and turns the OSC messages it receives into the few things a button may do:
//! send an event (GO, NEXT, a scene's own event), set a context value (a fader, a sensor), or hit STOP LASERS.
//! It can never arm a laser, edit the show, or do anything else: it acts as the role **button**, which may only press.
//!
//! Safety, because a UDP port has no login:
//! - listening on anything but this machine **requires an allow list** of the sender addresses; datagrams from any
//!   other address are dropped and only counted;
//! - at most 100 messages a second are looked at, per input;
//! - a held press (a scene that has not run its minimum yet) is held exactly as for a person;
//! - every press that moves the show, every refusal, and STOP LASERS is written to the receipts under the input's name.

use crate::governance::{Actor, Perm};
use crate::{links, Applied, Show};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;

/// At most this many messages a second are looked at, per input; the rest are dropped and counted.
const MAX_PER_SEC: u32 = 100;

#[derive(Deserialize, Clone, Debug)]
pub struct Binding {
    pub address: String,
    /// Send this event to the show.
    #[serde(default)]
    pub event: Option<String>,
    /// Set this context key to the message's first argument.
    #[serde(default)]
    pub context: Option<String>,
    /// "laser.stop": the hard stop.
    #[serde(default)]
    pub action: Option<String>,
    /// Feed timecode: the message carries a position, as a string "HH:MM:SS:FF", four numbers (hours, minutes, seconds, frames,
    /// and optionally the frame rate), or a single number of seconds.
    #[serde(default)]
    pub timecode: bool,
    /// For a timecode binding: the frame rate when the message does not say (24, 25, 29.97, 30). Default 25.
    #[serde(default)]
    pub fps: Option<f64>,
    /// For a timecode binding at 29.97: drop-frame.
    #[serde(default)]
    pub drop: bool,
    /// For events and actions: ignore a message whose first argument is 0 or false (the button being released). Default true.
    #[serde(default = "yes")]
    pub press: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
pub struct InputSpec {
    pub name: String,
    /// `host:port` to listen on, for example `127.0.0.1:9000` or `0.0.0.0:9000`.
    pub listen: String,
    /// Sender addresses that may press. Required unless listening on this machine only.
    #[serde(default)]
    pub allow: Vec<String>,
    pub bindings: Vec<Binding>,
}

/// Anything that can show its counters in the Live tab: an OSC input or a MIDI input.
pub trait InputInfo: Send + Sync {
    fn json(&self) -> Value;
}

impl InputInfo for Input {
    fn json(&self) -> Value {
        Input::json(self)
    }
}

/// An inputs file: OSC inputs under `"inputs"`, MIDI under `"midi"`. (A bare list is OSC inputs.)
pub struct InputsFile {
    pub inputs: Vec<InputSpec>,
    pub midi: Vec<crate::midi::MidiSpec>,
}

pub fn parse_file(text: &str) -> Result<InputsFile, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("the inputs file is not valid JSON: {e}"))?;
    let (osc, midi) = if v.is_array() { (v.clone(), json!([])) } else { (v.get("inputs").cloned().unwrap_or(json!([])), v.get("midi").cloned().unwrap_or(json!([]))) };
    let inputs = parse_list(osc)?;
    let midi: Vec<crate::midi::MidiSpec> = serde_json::from_value(midi).map_err(|e| format!("the midi list is not in the expected shape: {e}"))?;
    crate::midi::check(&midi)?;
    if let Some(m) = midi.iter().find(|m| inputs.iter().any(|i| i.name.trim().eq_ignore_ascii_case(m.name.trim()))) {
        return Err(format!("two inputs are named {}", m.name));
    }
    Ok(InputsFile { inputs, midi })
}

/// Read and check the OSC inputs of a file: `{"inputs": [...]}` or a bare list.
pub fn parse(text: &str) -> Result<Vec<InputSpec>, String> {
    parse_file(text).map(|f| f.inputs)
}

fn parse_list(list: Value) -> Result<Vec<InputSpec>, String> {
    let specs: Vec<InputSpec> = serde_json::from_value(list).map_err(|e| format!("the inputs file is not in the expected shape: {e}"))?;
    let mut names: Vec<String> = Vec::new();
    for s in &specs {
        let name = s.name.trim();
        if name.is_empty() || name.len() > 40 || name.chars().any(|c| c.is_control()) {
            return Err("every input needs a short name (it appears in the record)".into());
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(format!("two inputs are named {name}"));
        }
        names.push(name.to_string());
        let addr: SocketAddr = s.listen.parse().map_err(|_| format!("{name}: listen '{}' is not host:port (for example 127.0.0.1:9000)", s.listen))?;
        let allow = s.allow.iter().map(|a| a.parse::<IpAddr>().map_err(|_| format!("{name}: allow '{a}' is not an IP address"))).collect::<Result<Vec<_>, _>>()?;
        if !addr.ip().is_loopback() && allow.is_empty() {
            return Err(format!("{name}: listening on {} lets anyone on the network press: add \"allow\": [the sender's address]", addr.ip()));
        }
        if s.bindings.is_empty() {
            return Err(format!("{name}: no bindings"));
        }
        let mut seen: Vec<&str> = Vec::new();
        for b in &s.bindings {
            if !b.address.starts_with('/') {
                return Err(format!("{name}: address '{}' must start with /", b.address));
            }
            if seen.contains(&b.address.as_str()) {
                return Err(format!("{name}: {} is bound twice", b.address));
            }
            seen.push(&b.address);
            let kinds = b.event.is_some() as u8 + b.context.is_some() as u8 + b.action.is_some() as u8 + b.timecode as u8;
            if kinds != 1 {
                return Err(format!("{name}: {} must have exactly one of event, context, action or timecode", b.address));
            }
            if let Some(f) = b.fps {
                if crate::timecode::Fps::from_number(f, b.drop).is_none() {
                    return Err(format!("{name}: fps {f} is not one of 23.976, 24, 25, 29.97, 30"));
                }
            }
            if let Some(a) = &b.action {
                if a != "laser.stop" {
                    return Err(format!("{name}: action '{a}' is not allowed: an input may only press an event, set a context value, or laser.stop"));
                }
            }
        }
    }
    Ok(specs)
}

#[derive(Default)]
pub struct Stats {
    received: AtomicU64,
    pressed: AtomicU64,
    unlisted: AtomicU64,
    unbound: AtomicU64,
    dropped: AtomicU64,
    refused: AtomicU64,
    last_ms: AtomicI64,
    last_address: Mutex<String>,
    refusal_logged: AtomicBool,
}

pub struct Input {
    pub spec: InputSpec,
    pub bound: Mutex<Option<SocketAddr>>,
    pub stats: Stats,
    last_context: Mutex<std::collections::HashMap<String, Value>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Input {
    pub fn new(spec: InputSpec) -> Arc<Self> {
        Arc::new(Input { spec, bound: Mutex::new(None), stats: Stats::default(), last_context: Mutex::default() })
    }

    /// Who an input is in the record: its name, in the role that may only press.
    pub fn actor(&self) -> Actor {
        Actor::Member { name: self.spec.name.clone(), role: "button".into() }
    }

    pub fn json(&self) -> Value {
        let last = self.stats.last_ms.load(Ordering::Relaxed);
        json!({
            "name": self.spec.name,
            "listen": self.bound.lock().unwrap().map(|a| a.to_string()).unwrap_or_else(|| self.spec.listen.clone()),
            "allow": self.spec.allow,
            "bindings": self.spec.bindings.len(),
            "received": self.stats.received.load(Ordering::Relaxed),
            "pressed": self.stats.pressed.load(Ordering::Relaxed),
            "unlisted_sender": self.stats.unlisted.load(Ordering::Relaxed),
            "unbound": self.stats.unbound.load(Ordering::Relaxed),
            "dropped": self.stats.dropped.load(Ordering::Relaxed),
            "refused": self.stats.refused.load(Ordering::Relaxed),
            "heard_ms_ago": (last > 0).then(|| (now_ms() - last).max(0)),
            "last_address": self.stats.last_address.lock().unwrap().clone(),
        })
    }
}

/// Start listening. The show keeps the input so its counters can be read; the task stops when the show is dropped.
pub async fn start(show: &Arc<Show>, spec: InputSpec) -> Result<Arc<Input>, String> {
    let socket = UdpSocket::bind(&spec.listen).await.map_err(|e| format!("{}: cannot listen on {}: {e} (is something else using that port?)", spec.name, spec.listen))?;
    let input = Input::new(spec);
    *input.bound.lock().unwrap() = socket.local_addr().ok();
    let info: Arc<dyn InputInfo> = Arc::clone(&input) as Arc<dyn InputInfo>;
    show.add_input(info);
    let weak = Arc::downgrade(show);
    let me = Arc::clone(&input);
    let allow: Vec<IpAddr> = me.spec.allow.iter().filter_map(|a| a.parse().ok()).collect();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        let (mut window, mut count) = (0i64, 0u32);
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
            let Some(show) = weak.upgrade() else { break };
            if !allow.is_empty() && !allow.contains(&from.ip()) {
                me.stats.unlisted.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let now = now_ms();
            if now - window >= 1000 {
                window = now;
                count = 0;
            }
            count += 1;
            if count > MAX_PER_SEC {
                me.stats.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            handle(&show, &me, &buf[..n]);
        }
    });
    Ok(input)
}

/// Act on one datagram.
pub fn handle(show: &Arc<Show>, input: &Input, bytes: &[u8]) {
    let actor = input.actor();
    let source = format!("osc:{}", input.spec.name);
    for (address, args) in links::flatten(bytes) {
        input.stats.received.fetch_add(1, Ordering::Relaxed);
        input.stats.last_ms.store(now_ms(), Ordering::Relaxed);
        *input.stats.last_address.lock().unwrap() = address.clone();
        let Some(b) = input.spec.bindings.iter().find(|b| b.address == address) else {
            input.stats.unbound.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let first = args.first().cloned();
        let released = matches!(&first, Some(Value::Number(n)) if n.as_f64() == Some(0.0)) || matches!(&first, Some(Value::Bool(false)));
        if b.action.is_some() {
            // the hard stop is the safe direction: always allowed, never held
            if b.press && released {
                continue;
            }
            input.stats.pressed.fetch_add(1, Ordering::Relaxed);
            let v = show.laser_stop();
            show.receipt("laser.stop", "ALLOW", &actor, &format!("hard stop from {address}: disarmed {} laser(s)", v["stopped"].as_array().map_or(0, |s| s.len())));
            continue;
        }
        if !show.allow(&actor, Perm::Go, None) {
            input.stats.refused.fetch_add(1, Ordering::Relaxed);
            if !input.stats.refusal_logged.swap(true, Ordering::Relaxed) {
                show.receipt("show.go", "DENY", &actor, &format!("refused: this input may not press ({address}); it needs a place on the roster"));
            }
            continue;
        }
        input.stats.refusal_logged.store(false, Ordering::Relaxed);
        if let Some(event) = &b.event {
            if b.press && released {
                continue;
            }
            input.stats.pressed.fetch_add(1, Ordering::Relaxed);
            match show.apply_checked(&source, event) {
                Applied::Moved => {
                    show.receipt("show.go", "ALLOW", &actor, &format!("{address} sent '{event}': moved the show to {}", show.current_state()));
                }
                held @ Applied::Held { .. } => {
                    show.receipt("show.go", "DENY", &actor, &format!("{address} sent '{event}' {}", held.held_text().unwrap_or_default()));
                }
                Applied::Ignored => {}
            }
        } else if b.timecode {
            if let Some((pos, fps)) = timecode_from_args(&args, b) {
                show.timecode_update(pos, fps, &source);
            }
        } else if let Some(key) = &b.context {
            let v = first.unwrap_or(Value::Bool(true));
            let mut last = input.last_context.lock().unwrap();
            if last.get(key) != Some(&v) {
                last.insert(key.clone(), v.clone());
                drop(last);
                input.stats.pressed.fetch_add(1, Ordering::Relaxed);
                show.set_context(key, v);
            }
        }
    }
}

/// A position in milliseconds and its frame rate, from the arguments of a timecode message.
fn timecode_from_args(args: &[Value], b: &Binding) -> Option<(f64, crate::timecode::Fps)> {
    use crate::timecode::{parse_fields, Fps, Tc};
    let default_fps = b.fps.unwrap_or(25.0);
    let num = |i: usize| args.get(i).and_then(Value::as_f64);
    if let Some(text) = args.first().and_then(Value::as_str) {
        let (h, m, s, f, drop) = parse_fields(text).ok()?;
        let fps = Fps::from_number(default_fps, drop || b.drop)?;
        return Tc::new(h, m, s, f, fps).ok().map(|t| (t.to_ms(), fps));
    }
    if args.len() >= 4 && (0..4).all(|i| num(i).is_some()) {
        let fps = Fps::from_number(num(4).unwrap_or(default_fps), b.drop)?;
        return Tc::new(num(0)? as u32, num(1)? as u32, num(2)? as u32, num(3)? as u32, fps).ok().map(|t| (t.to_ms(), fps));
    }
    if args.len() == 1 {
        let secs = num(0)?;
        return (secs >= 0.0).then(|| (secs * 1000.0, Fps::from_number(default_fps, b.drop).unwrap_or(Fps::F25)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(extra: &str) -> String {
        format!(r#"{{"inputs":[{{"name":"Stream Deck","listen":"127.0.0.1:9000","bindings":[{{"address":"/pchi/go","event":"NEXT"}}]{extra}}}]}}"#)
    }

    #[test]
    fn an_inputs_file_must_be_sound_and_safe() {
        assert_eq!(parse(&spec("")).unwrap().len(), 1);
        let bad = |t: &str| parse(t).err().unwrap_or_default();
        assert!(bad(r#"[{"name":"A","listen":"0.0.0.0:9000","bindings":[{"address":"/a","event":"GO"}]}]"#).contains("allow"), "a public port needs an allow list");
        assert!(parse(r#"[{"name":"A","listen":"0.0.0.0:9000","allow":["192.168.1.50"],"bindings":[{"address":"/a","event":"GO"}]}]"#).is_ok());
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","bindings":[{"address":"/a","action":"arm"}]}]"#).contains("not allowed"));
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","bindings":[{"address":"/a"}]}]"#).contains("exactly one"));
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","bindings":[{"address":"/a","event":"X","context":"y"}]}]"#).contains("exactly one"));
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","bindings":[{"address":"a","event":"X"}]}]"#).contains("start with /"));
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","bindings":[{"address":"/a","event":"X"},{"address":"/a","event":"Y"}]}]"#).contains("bound twice"));
        assert!(bad(r#"[{"name":"A","listen":"nonsense","bindings":[{"address":"/a","event":"X"}]}]"#).contains("host:port"));
        assert!(bad(r#"[{"name":"","listen":"127.0.0.1:9000","bindings":[{"address":"/a","event":"X"}]}]"#).contains("name"));
        assert!(bad(r#"[{"name":"A","listen":"127.0.0.1:9000","allow":["nope"],"bindings":[{"address":"/a","event":"X"}]}]"#).contains("not an IP"));
        assert!(bad("{").contains("not valid JSON"));
    }
}
