//! MIDI input: a foot pedal, a keyboard, a controller or a playback machine sending MIDI Time Code.
//!
//! What a message may do is the same short list as for OSC inputs (`inputs.rs`): send an event (a note, a program change or a
//! controller used as a button), set a context value (a controller), hit STOP LASERS, or feed timecode (MIDI Time Code). It acts
//! as the role **button**: it can press, nothing else.
//!
//! Reading MIDI from a port needs the system's MIDI library, so it is built only with `--features midi`. Without it the runtime
//! says so and the rest still works. Everything below the transport (parsing, bindings, timecode) is plain code and tested;
//! the transport itself (`start`) is not tested here against real MIDI hardware.

use crate::governance::{Actor, Perm};
use crate::inputs::InputInfo;
use crate::timecode::MtcDecoder;
use crate::{Applied, Show};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Deserialize, Clone, Debug)]
pub struct MidiBinding {
    /// 1 to 16. Without it, any channel.
    #[serde(default)]
    pub channel: Option<u8>,
    /// Trigger: a note (0 to 127), a controller number, a program number, or `"timecode": "mtc"`.
    #[serde(default)]
    pub note: Option<u8>,
    #[serde(default)]
    pub cc: Option<u8>,
    #[serde(default)]
    pub pc: Option<u8>,
    #[serde(default)]
    pub timecode: Option<String>,
    /// Effect: send this event, set this context key (from a controller), or "laser.stop".
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    /// For a context value: `[low, high]` that 0 to 127 is scaled to. Default: the raw 0 to 127.
    #[serde(default)]
    pub scale: Option<[f64; 2]>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MidiSpec {
    pub name: String,
    /// Part of the MIDI port's name (`"nanoKONTROL"`, `"loopMIDI"`). Without it, the first port.
    #[serde(default)]
    pub port: Option<String>,
    pub bindings: Vec<MidiBinding>,
}

/// Check the `midi` list of an inputs file.
pub fn check(specs: &[MidiSpec]) -> Result<(), String> {
    let mut names: Vec<String> = Vec::new();
    for s in specs {
        let name = s.name.trim();
        if name.is_empty() || name.len() > 40 || name.chars().any(|c| c.is_control()) {
            return Err("every MIDI input needs a short name (it appears in the record)".into());
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(format!("two inputs are named {name}"));
        }
        names.push(name.to_string());
        if s.bindings.is_empty() {
            return Err(format!("{name}: no bindings"));
        }
        for b in &s.bindings {
            let triggers = b.note.is_some() as u8 + b.cc.is_some() as u8 + b.pc.is_some() as u8 + b.timecode.is_some() as u8;
            if triggers != 1 {
                return Err(format!("{name}: each binding needs exactly one of note, cc, pc or timecode"));
            }
            if let Some(c) = b.channel {
                if !(1..=16).contains(&c) {
                    return Err(format!("{name}: channel {c} is not 1 to 16"));
                }
            }
            for n in [b.note, b.cc, b.pc].into_iter().flatten() {
                if n > 127 {
                    return Err(format!("{name}: {n} is not 0 to 127"));
                }
            }
            if let Some(t) = &b.timecode {
                if t != "mtc" {
                    return Err(format!("{name}: timecode '{t}' is not supported: use \"mtc\" (LTC is audio, not MIDI)"));
                }
                if b.event.is_some() || b.context.is_some() || b.action.is_some() {
                    return Err(format!("{name}: a timecode binding has no event, context or action: the timecode itself drives the show"));
                }
                continue;
            }
            let effects = b.event.is_some() as u8 + b.context.is_some() as u8 + b.action.is_some() as u8;
            if effects != 1 {
                return Err(format!("{name}: each binding needs exactly one of event, context or action"));
            }
            if b.context.is_some() && b.cc.is_none() {
                return Err(format!("{name}: a context value needs a controller (cc): notes and programs are presses"));
            }
            if let Some(a) = &b.action {
                if a != "laser.stop" {
                    return Err(format!("{name}: action '{a}' is not allowed: only laser.stop"));
                }
            }
        }
    }
    Ok(())
}

/// A MIDI message, the parts we use.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    NoteOn { ch: u8, note: u8, vel: u8 },
    NoteOff { ch: u8, note: u8 },
    Cc { ch: u8, cc: u8, val: u8 },
    Pc { ch: u8, prog: u8 },
    /// The data byte of an MTC quarter-frame message.
    QuarterFrame(u8),
    SysEx(Vec<u8>),
    Other,
}

/// Read one message as the system delivers it (one complete message per call). Channels are 1 to 16.
pub fn parse(b: &[u8]) -> Msg {
    let Some(&status) = b.first() else { return Msg::Other };
    let ch = (status & 0x0F) + 1;
    match (status & 0xF0, b.len()) {
        (0x90, 3) if b[2] > 0 => Msg::NoteOn { ch, note: b[1] & 0x7F, vel: b[2] & 0x7F },
        (0x90, 3) | (0x80, 3) => Msg::NoteOff { ch, note: b[1] & 0x7F },
        (0xB0, 3) => Msg::Cc { ch, cc: b[1] & 0x7F, val: b[2] & 0x7F },
        (0xC0, 2) => Msg::Pc { ch, prog: b[1] & 0x7F },
        _ if status == 0xF1 && b.len() == 2 => Msg::QuarterFrame(b[1] & 0x7F),
        _ if status == 0xF0 => Msg::SysEx(b.to_vec()),
        _ => Msg::Other,
    }
}

pub struct MidiInput {
    pub spec: MidiSpec,
    port: Mutex<String>,
    received: AtomicU64,
    pressed: AtomicU64,
    unbound: AtomicU64,
    last_ms: AtomicI64,
    last: Mutex<String>,
    mtc: Mutex<MtcDecoder>,
    last_context: Mutex<std::collections::HashMap<String, Value>>,
    /// Keeps the system connection open; dropped with the input.
    #[allow(dead_code)]
    connection: Mutex<Option<Box<dyn std::any::Any + Send>>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl MidiInput {
    pub fn new(spec: MidiSpec) -> Arc<Self> {
        Arc::new(MidiInput {
            port: Mutex::new(spec.port.clone().unwrap_or_else(|| "first port".into())),
            spec,
            received: AtomicU64::new(0),
            pressed: AtomicU64::new(0),
            unbound: AtomicU64::new(0),
            last_ms: AtomicI64::new(0),
            last: Mutex::new(String::new()),
            mtc: Mutex::new(MtcDecoder::default()),
            last_context: Mutex::default(),
            connection: Mutex::new(None),
        })
    }

    pub fn actor(&self) -> Actor {
        Actor::Member { name: self.spec.name.clone(), role: "button".into() }
    }
}

impl InputInfo for MidiInput {
    fn json(&self) -> Value {
        let last = self.last_ms.load(Ordering::Relaxed);
        json!({
            "name": self.spec.name,
            "listen": format!("MIDI {}", self.port.lock().unwrap()),
            "bindings": self.spec.bindings.len(),
            "received": self.received.load(Ordering::Relaxed),
            "pressed": self.pressed.load(Ordering::Relaxed),
            "unlisted_sender": 0, "unbound": self.unbound.load(Ordering::Relaxed), "dropped": 0, "refused": 0,
            "heard_ms_ago": (last > 0).then(|| (now_ms() - last).max(0)),
            "last_address": self.last.lock().unwrap().clone(),
        })
    }
}

fn describe(m: &Msg) -> String {
    match m {
        Msg::NoteOn { ch, note, .. } | Msg::NoteOff { ch, note } => format!("note {note} ch{ch}"),
        Msg::Cc { ch, cc, .. } => format!("cc {cc} ch{ch}"),
        Msg::Pc { ch, prog } => format!("pc {prog} ch{ch}"),
        Msg::QuarterFrame(_) => "mtc".into(),
        Msg::SysEx(_) => "sysex".into(),
        Msg::Other => "other".into(),
    }
}

/// Act on one MIDI message.
pub fn handle(show: &Arc<Show>, input: &MidiInput, bytes: &[u8]) {
    let msg = parse(bytes);
    if msg == Msg::Other {
        return;
    }
    // timecode first: quarter frames come four a frame and must not be counted as presses
    let tc = match &msg {
        Msg::QuarterFrame(d) => input.mtc.lock().unwrap().quarter_frame(*d),
        Msg::SysEx(b) => MtcDecoder::full_frame(b),
        _ => None,
    };
    if matches!(msg, Msg::QuarterFrame(_) | Msg::SysEx(_)) {
        if input.spec.bindings.iter().any(|b| b.timecode.is_some()) {
            if let Some(tc) = tc {
                let actor = input.actor();
                if show.allow(&actor, Perm::Go, None) {
                    show.timecode_update(tc.to_ms(), tc.fps, "mtc");
                }
            }
            input.last_ms.store(now_ms(), Ordering::Relaxed);
            *input.last.lock().unwrap() = describe(&msg);
        }
        return;
    }
    input.received.fetch_add(1, Ordering::Relaxed);
    input.last_ms.store(now_ms(), Ordering::Relaxed);
    *input.last.lock().unwrap() = describe(&msg);
    let (ch, kind, number, value) = match msg {
        Msg::NoteOn { ch, note, vel } => (ch, 'n', note, vel),
        Msg::NoteOff { ch, note } => (ch, 'n', note, 0),
        Msg::Cc { ch, cc, val } => (ch, 'c', cc, val),
        Msg::Pc { ch, prog } => (ch, 'p', prog, 127),
        _ => return,
    };
    let Some(b) = input.spec.bindings.iter().find(|b| {
        b.channel.is_none_or(|c| c == ch)
            && match kind {
                'n' => b.note == Some(number),
                'c' => b.cc == Some(number),
                _ => b.pc == Some(number),
            }
    }) else {
        input.unbound.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let actor = input.actor();
    let source = format!("midi:{}", input.spec.name);
    let what = describe(&parse(bytes));
    // a controller used as a button counts as pressed from the middle of its range up
    let pressed = if kind == 'c' { value >= 64 } else { value > 0 };
    if b.action.is_some() {
        if !pressed {
            return;
        }
        input.pressed.fetch_add(1, Ordering::Relaxed);
        let v = show.laser_stop();
        show.receipt("laser.stop", "ALLOW", &actor, &format!("hard stop from MIDI {what}: disarmed {} laser(s)", v["stopped"].as_array().map_or(0, |s| s.len())));
        return;
    }
    if !show.allow(&actor, Perm::Go, None) {
        show.receipt("show.go", "DENY", &actor, &format!("refused: this input may not press ({what}); it needs a place on the roster"));
        return;
    }
    if let Some(event) = &b.event {
        if !pressed {
            return;
        }
        input.pressed.fetch_add(1, Ordering::Relaxed);
        match show.apply_checked(&source, event) {
            Applied::Moved => {
                show.receipt("show.go", "ALLOW", &actor, &format!("MIDI {what} sent '{event}': moved the show to {}", show.current_state()));
            }
            held @ Applied::Held { .. } => {
                show.receipt("show.go", "DENY", &actor, &format!("MIDI {what} sent '{event}' {}", held.held_text().unwrap_or_default()));
            }
            Applied::Ignored => {}
        }
    } else if let Some(key) = &b.context {
        let [lo, hi] = b.scale.unwrap_or([0.0, 127.0]);
        let v = json!(lo + (hi - lo) * (value as f64 / 127.0));
        let mut last = input.last_context.lock().unwrap();
        if last.get(key) != Some(&v) {
            last.insert(key.clone(), v.clone());
            drop(last);
            input.pressed.fetch_add(1, Ordering::Relaxed);
            show.set_context(key, v);
        }
    }
}

/// Open the MIDI port and listen. Built only with `--features midi`.
#[cfg(feature = "midi")]
pub fn start(show: &Arc<Show>, spec: MidiSpec) -> Result<Arc<MidiInput>, String> {
    use midir::{Ignore, MidiInput as Sys};
    let mut sys = Sys::new("pchi-studio").map_err(|e| format!("{}: cannot start MIDI: {e}", spec.name))?;
    // the default drops timecode, sysex and clock messages, which is exactly what MTC is
    sys.ignore(Ignore::ActiveSense);
    let ports = sys.ports();
    let names: Vec<String> = ports.iter().map(|p| sys.port_name(p).unwrap_or_default()).collect();
    let pick = match &spec.port {
        Some(want) => names.iter().position(|n| n.to_lowercase().contains(&want.to_lowercase())),
        None => (!names.is_empty()).then_some(0),
    };
    let Some(i) = pick else {
        return Err(format!("{}: no MIDI port matches {:?}. Ports found: {}", spec.name, spec.port.as_deref().unwrap_or("(first)"), if names.is_empty() { "none".to_string() } else { names.join(", ") }));
    };
    let input = MidiInput::new(spec);
    *input.port.lock().unwrap() = names[i].clone();
    show.add_input(Arc::clone(&input) as Arc<dyn InputInfo>);
    let (weak, me, rt) = (Arc::downgrade(show), Arc::clone(&input), tokio::runtime::Handle::current());
    let conn = sys
        .connect(
            &ports[i],
            "pchi-in",
            move |_ts, bytes, _| {
                if let Some(show) = weak.upgrade() {
                    let _guard = rt.enter(); // the show spawns tasks: give this thread the runtime
                    handle(&show, &me, bytes);
                }
            },
            (),
        )
        .map_err(|e| format!("{}: cannot open MIDI port {}: {e}", input.spec.name, names[i]))?;
    *input.connection.lock().unwrap() = Some(Box::new(conn));
    Ok(input)
}

#[cfg(not(feature = "midi"))]
pub fn start(_show: &Arc<Show>, spec: MidiSpec) -> Result<Arc<MidiInput>, String> {
    Err(format!("{}: this build cannot read MIDI ports. Build with: cargo build --release -p pchi-studio-api --features midi (it needs the system MIDI library; on Linux, libasound2-dev)", spec.name))
}

/// The MIDI input ports this machine has (empty without `--features midi`).
pub fn list_ports() -> Vec<String> {
    #[cfg(feature = "midi")]
    {
        if let Ok(sys) = midir::MidiInput::new("pchi-studio") {
            return sys.ports().iter().map(|p| sys.port_name(p).unwrap_or_default()).collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_messages_are_read_as_the_system_delivers_them() {
        assert_eq!(parse(&[0x90, 60, 100]), Msg::NoteOn { ch: 1, note: 60, vel: 100 });
        assert_eq!(parse(&[0x93, 60, 0]), Msg::NoteOff { ch: 4, note: 60 }, "a note-on with velocity 0 is a release");
        assert_eq!(parse(&[0x80, 60, 64]), Msg::NoteOff { ch: 1, note: 60 });
        assert_eq!(parse(&[0xB0, 7, 127]), Msg::Cc { ch: 1, cc: 7, val: 127 });
        assert_eq!(parse(&[0xCF, 3]), Msg::Pc { ch: 16, prog: 3 });
        assert_eq!(parse(&[0xF1, 0x23]), Msg::QuarterFrame(0x23));
        assert!(matches!(parse(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0, 0, 0, 0, 0xF7]), Msg::SysEx(_)));
        assert_eq!(parse(&[0xF8]), Msg::Other, "clock ticks are ignored");
        assert_eq!(parse(&[]), Msg::Other);
    }

    #[test]
    fn a_midi_binding_must_be_sound_and_safe() {
        let ok = |t: &str| check(&serde_json::from_str::<Vec<MidiSpec>>(t).unwrap());
        let base = |b: &str| format!(r#"[{{"name":"Pedal","bindings":[{b}]}}]"#);
        assert!(ok(&base(r#"{"note":60,"event":"NEXT"}"#)).is_ok());
        assert!(ok(&base(r#"{"timecode":"mtc"}"#)).is_ok());
        assert!(ok(&base(r#"{"cc":7,"context":"volume","scale":[0,1]}"#)).is_ok());
        for (b, why) in [
            (r#"{"event":"X"}"#, "exactly one of note"),
            (r#"{"note":60,"cc":7,"event":"X"}"#, "exactly one of note"),
            (r#"{"note":60}"#, "exactly one of event"),
            (r#"{"note":60,"event":"X","context":"y"}"#, "exactly one of event"),
            (r#"{"note":200,"event":"X"}"#, "not 0 to 127"),
            (r#"{"note":60,"channel":17,"event":"X"}"#, "not 1 to 16"),
            (r#"{"note":60,"context":"y"}"#, "needs a controller"),
            (r#"{"note":60,"action":"laser.arm"}"#, "not allowed"),
            (r#"{"timecode":"ltc"}"#, "not supported"),
            (r#"{"timecode":"mtc","event":"X"}"#, "no event"),
        ] {
            let e = ok(&base(b)).err().unwrap_or_default();
            assert!(e.contains(why), "{b}: {e}");
        }
    }
}
