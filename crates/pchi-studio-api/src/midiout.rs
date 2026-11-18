//! MIDI output: a cue becomes a short MIDI message to a port, for a lighting desk, a sampler, a playback machine or a controller's lights.
//!
//! A rig device of type `midi-out` names a MIDI output port in its `host` (part of the port's name, any case; blank means the first
//! port). A cue's value says what to send, in a closed set of words:
//!
//! | value | sends |
//! |---|---|
//! | `note 60` or `note 60 100 ch 2` | note on (velocity 100 unless given), then note off 100 ms later |
//! | `pc 12 ch 3` | program change |
//! | `cc 7 64 ch 1` | controller 7 set to 64 |
//! | `panic` | controller 123 (all notes off) on all 16 channels |
//!
//! Numbers are sent as written, 0 to 127 (a device that shows programs as 1 to 128 will show one more than written). The channel is 1 to
//! 16 and is 1 if not said. Nothing else can be sent: no system-exclusive, no clock, no start or stop, no free bytes. A cue cannot put
//! its own bytes on the wire, whatever it says.
//!
//! Writing to a port needs the system's MIDI library, so the transport is built only with `--features midi` (as MIDI input is). Without
//! it a send fails and says so, and the doors check says so before the show. Everything below the transport (the words, the bytes,
//! the checks) is plain code and is tested; the transport is **not** tested here against real MIDI hardware.

use crate::rig::Device;
use pchi_showchart::Action;
use serde_json::Value;

/// The rig device type this module drives.
pub const KIND: &str = "midi-out";

/// How long a note stays on before the note off is sent.
pub const NOTE_MS: u64 = 100;

/// Whether this build can write to a MIDI port.
pub const BUILT: bool = cfg!(feature = "midi");

/// One message, and how long to wait before sending it.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub wait_ms: u64,
    pub bytes: Vec<u8>,
}

/// What a cue asks for: the first value in its payload, trimmed.
pub fn value_of(action: &Action) -> String {
    match &action.payload {
        Value::Object(p) => p.values().next().and_then(|v| v.as_str().map(|s| s.trim().to_string()).or_else(|| v.as_i64().map(|n| n.to_string()))).unwrap_or_default(),
        _ => String::new(),
    }
}

fn number(word: &str, what: &str, lo: u8, hi: u8) -> Result<u8, String> {
    let n: u8 = word.parse().map_err(|_| format!("{what} '{word}' is not a whole number {lo} to {hi}"))?;
    if n < lo || n > hi {
        return Err(format!("{what} {n} is not {lo} to {hi}"));
    }
    Ok(n)
}

/// Split off a trailing channel (`ch 2` or `ch2`), leaving the other words. Channels are 1 to 16; the default is 1.
fn channel(words: &mut Vec<String>) -> Result<u8, String> {
    if let Some(w) = words.last().filter(|w| w.starts_with("ch")).cloned() {
        words.pop();
        let digits = w.trim_start_matches("ch");
        if digits.is_empty() {
            let n = words.pop().ok_or("'ch' needs a channel number")?;
            return number(&n, "channel", 1, 16);
        }
        return number(digits, "channel", 1, 16);
    }
    if words.len() >= 2 && words[words.len() - 2] == "ch" {
        let n = words.pop().unwrap_or_default();
        words.pop();
        return number(&n, "channel", 1, 16);
    }
    Ok(1)
}

/// Turn a cue's value into the messages it sends, or say what is wrong with it.
pub fn parse(value: &str) -> Result<Vec<Step>, String> {
    if value.chars().any(|c| c.is_control()) || value.len() > 70 {
        return Err("a MIDI value is a few plain words, such as note 60 or pc 12 ch 2".into());
    }
    let mut words: Vec<String> = value.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    let verb = if words.is_empty() { return Err("no MIDI value: write note 60, pc 12, cc 7 64 or panic".into()) } else { words.remove(0) };
    let step = |wait_ms: u64, bytes: Vec<u8>| Step { wait_ms, bytes };
    let hint = || format!("'{value}' is not one of: note N [VEL], pc N, cc N V, panic (each optionally followed by ch C), or msc go|stop|resume|load|all-off|restore|reset [cue]");
    match verb.as_str() {
        "msc" => Ok(vec![step(0, crate::msc::from_words(&words)?.encode()?)]),
        "note" => {
            let ch = channel(&mut words)? - 1;
            if words.is_empty() || words.len() > 2 {
                return Err(hint());
            }
            let n = number(&words[0], "note", 0, 127)?;
            let vel = words.get(1).map(|v| number(v, "velocity", 1, 127)).transpose()?.unwrap_or(100);
            Ok(vec![step(0, vec![0x90 | ch, n, vel]), step(NOTE_MS, vec![0x80 | ch, n, 0])])
        }
        "pc" => {
            let ch = channel(&mut words)? - 1;
            if words.len() != 1 {
                return Err(hint());
            }
            Ok(vec![step(0, vec![0xC0 | ch, number(&words[0], "program", 0, 127)?])])
        }
        "cc" => {
            let ch = channel(&mut words)? - 1;
            if words.len() != 2 {
                return Err(hint());
            }
            Ok(vec![step(0, vec![0xB0 | ch, number(&words[0], "controller", 0, 127)?, number(&words[1], "value", 0, 127)?])])
        }
        "panic" => {
            if !words.is_empty() {
                return Err(hint());
            }
            Ok((0..16u8).map(|ch| step(0, vec![0xB0 | ch, 123, 0])).collect())
        }
        _ => Err(hint()),
    }
}

/// Check a cue for a `midi-out` device. Sends nothing.
pub fn check(action: &Action) -> Result<(), String> {
    parse(&value_of(action)).map(|_| ())
}

/// What a message is, in words (for a person reading what would be sent).
pub fn words(bytes: &[u8]) -> String {
    use crate::midi::{parse, Msg};
    if let Some(m) = crate::msc::decode(bytes) {
        return m.words();
    }
    match parse(bytes) {
        Msg::NoteOn { ch, note, vel } => format!("note on {note} velocity {vel} channel {ch}"),
        Msg::NoteOff { ch, note } => format!("note off {note} channel {ch}"),
        Msg::Cc { ch, cc, val } => format!("controller {cc} = {val} channel {ch}"),
        Msg::Pc { ch, prog } => format!("program {prog} channel {ch}"),
        _ => format!("{bytes:02X?}"),
    }
}

/// What sending the cue to the device puts on the wire, for a person to read.
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let steps = parse(&value_of(action))?;
    let port = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or("the first MIDI output port");
    // sixteen all-notes-off messages are one idea: say it once
    if steps.len() == 16 && steps.iter().all(|s| s.bytes.get(1) == Some(&123)) {
        return Ok(vec![(format!("MIDI out to {port}: all notes off on all 16 channels"), vec![])]);
    }
    Ok(steps.iter().map(|s| (format!("MIDI out to {port}: {}{}", words(&s.bytes), if s.wait_ms > 0 { format!(" (after {} ms)", s.wait_ms) } else { String::new() }), vec![Value::String(s.bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" "))])).collect())
}

/// The MIDI output ports this machine has (empty without `--features midi`).
pub fn list_ports() -> Vec<String> {
    #[cfg(feature = "midi")]
    {
        if let Ok(sys) = midir::MidiOutput::new("pchi-studio") {
            return sys.ports().iter().map(|p| sys.port_name(p).unwrap_or_default()).collect();
        }
    }
    Vec::new()
}

/// Open connections to output ports, kept between cues.
#[derive(Default)]
pub struct MidiOut {
    #[cfg(feature = "midi")]
    conns: std::sync::Mutex<std::collections::HashMap<String, (String, String, midir::MidiOutputConnection)>>,
}

impl MidiOut {
    /// Send a cue's messages to the device's port. Returns where they went.
    #[cfg(not(feature = "midi"))]
    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<String, String> {
        parse(&value_of(action))?;
        Err(format!("{} is a MIDI output, and this build cannot write to MIDI ports. Build with: cargo build --release -p pchi-studio-api --features midi (it needs the system MIDI library; on Linux, libasound2-dev)", dev.label))
    }

    #[cfg(feature = "midi")]
    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<String, String> {
        let steps = parse(&value_of(action))?;
        let query = dev.host.as_deref().map(str::trim).unwrap_or("").to_string();
        let mut name = String::new();
        for s in &steps {
            if s.wait_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(s.wait_ms)).await;
            }
            let mut conns = self.conns.lock().unwrap();
            // a device whose port setting changed gets a new connection
            if conns.get(&dev.id).is_some_and(|(q, _, _)| *q != query) {
                conns.remove(&dev.id);
            }
            if !conns.contains_key(&dev.id) {
                let out = midir::MidiOutput::new("pchi-studio").map_err(|e| format!("cannot use MIDI: {e}"))?;
                let ports = out.ports();
                let found = ports.iter().find(|p| query.is_empty() || out.port_name(p).is_ok_and(|n| n.to_lowercase().contains(&query.to_lowercase())));
                let port = found.ok_or_else(|| format!("{}: no MIDI output port matches '{query}' (run pchi-studio-api --midi-ports to see them)", dev.label))?;
                let port_name = out.port_name(port).unwrap_or_default();
                let conn = out.connect(port, "pchi-out").map_err(|e| format!("{}: cannot open MIDI port {port_name}: {e}", dev.label))?;
                conns.insert(dev.id.clone(), (query.clone(), port_name, conn));
            }
            let entry = conns.get_mut(&dev.id).ok_or("no connection")?;
            name = entry.1.clone();
            if let Err(e) = entry.2.send(&s.bytes) {
                conns.remove(&dev.id);
                return Err(format!("{}: sending to the MIDI port failed: {e}", dev.label));
            }
        }
        Ok(format!("MIDI out: {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bytes(v: &str) -> Vec<(u64, Vec<u8>)> {
        parse(v).unwrap().into_iter().map(|s| (s.wait_ms, s.bytes)).collect()
    }

    #[test]
    fn a_note_is_on_then_off_a_moment_later_on_the_right_channel() {
        assert_eq!(bytes("note 60"), [(0, vec![0x90, 60, 100]), (100, vec![0x80, 60, 0])]);
        assert_eq!(bytes("NOTE 36 127 ch 10"), [(0, vec![0x99, 36, 127]), (100, vec![0x89, 36, 0])]);
        assert_eq!(bytes("note 0 1 ch16"), [(0, vec![0x9F, 0, 1]), (100, vec![0x8F, 0, 0])]);
    }

    #[test]
    fn programs_and_controllers_are_sent_as_written() {
        assert_eq!(bytes("pc 12"), [(0, vec![0xC0, 12])]);
        assert_eq!(bytes("pc 127 ch 3"), [(0, vec![0xC2, 127])]);
        assert_eq!(bytes("cc 7 64"), [(0, vec![0xB0, 7, 64])]);
        assert_eq!(bytes("cc 64 0 ch2"), [(0, vec![0xB1, 64, 0])]);
    }

    #[test]
    fn panic_is_all_notes_off_on_every_channel_and_nothing_else() {
        let p = bytes("panic");
        assert_eq!(p.len(), 16);
        for (ch, (wait, b)) in p.iter().enumerate() {
            assert_eq!((*wait, b.clone()), (0, vec![0xB0 | ch as u8, 123, 0]));
        }
        assert!(parse("panic now").is_err());
    }

    #[test]
    fn anything_else_is_refused_and_says_what_is_allowed() {
        for bad in ["", "   ", "note", "note 128", "note 60 0", "note 60 128", "note 60 100 ch 17", "note 60 100 ch 0", "note 60 100 200", "pc", "pc 128", "pc 1 2", "cc 7", "cc 7 64 65", "cc 200 1", "start", "stop", "clock", "sysex f0 7e f7", "f0 7e 7f 09 01 f7", "note 6x", "note -1", "note 60 ch", "note 60\nstop", "note 60 ch 1 ch 2"] {
            assert!(parse(bad).is_err(), "'{bad}' must be refused");
        }
        assert!(parse("start").unwrap_err().contains("is not one of"));
        assert!(parse("note 128").unwrap_err().contains("0 to 127"));
        assert!(parse(&"note 60 ".repeat(10)).is_err(), "too long");
    }

    #[test]
    fn show_control_is_one_sysex_message_and_nothing_else_sysex_gets_through() {
        let one = |v: &str| { let s = parse(v).unwrap(); assert_eq!(s.len(), 1); s[0].bytes.clone() };
        assert_eq!(one("msc go 5"), [0xF0, 0x7F, 0, 2, 1, 1, 0x35, 0xF7]);
        assert_eq!(one("MSC LOAD 2.5 list 1 dev 4 fmt video"), [0xF0, 0x7F, 4, 2, 0x30, 5, b'2', b'.', b'5', 0, b'1', 0xF7]);
        for bad in ["msc", "msc fire 1", "msc go dev 127", "msc go fmt pyro", "msc go 1' or", "sysex f0 7f 7f 02 01 01 f7", "f0 7f 7f 02 01 01 f7", "msc timed-go 1"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        // whatever "msc" accepts is an MSC message to one device: F0 7F dev 02 fmt cmd .. F7, data bytes 7-bit
        for cmd in ["go", "stop", "resume 3", "load 3", "all-off", "restore", "reset"] {
            for dev in [0, 50, 111] {
                let b = one(&format!("msc {cmd} dev {dev}"));
                assert_eq!((b[0], b[1], b[3], *b.last().unwrap()), (0xF0, 0x7F, 0x02, 0xF7), "{cmd}");
                assert!(b[2] <= 111 && b[1..b.len() - 1].iter().all(|x| *x < 0x80));
            }
        }
    }

    #[test]
    fn no_value_can_produce_a_message_that_is_not_a_note_program_or_controller() {
        // every accepted value, tried on a grid, gives only note on/off, controller or program change messages
        for n in [0, 1, 60, 127] {
            for ch in [1, 9, 16] {
                for v in [format!("note {n} ch {ch}"), format!("pc {n} ch {ch}"), format!("cc {n} {n} ch {ch}")] {
                    for s in parse(&v).unwrap() {
                        assert!(matches!(s.bytes[0] & 0xF0, 0x80 | 0x90 | 0xB0 | 0xC0), "{v}: {:02X?}", s.bytes);
                        assert!(s.bytes[1..].iter().all(|b| *b < 0x80), "data bytes stay 7-bit: {v}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_cue_is_checked_by_its_first_value_and_described_in_words() {
        let a = |v: &str| Action::new("lighting.look", json!({"look": v}));
        assert!(check(&a("pc 4")).is_ok());
        assert!(check(&a("pc 400")).is_err());
        assert!(check(&Action::new("lighting.look", json!({}))).unwrap_err().contains("no MIDI value"));
        let dev = Device { id: "m1".into(), label: "Desk MIDI".into(), kind: KIND.into(), dept: "lighting".into(), host: Some("loopMIDI".into()), port: None, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None };
        assert_eq!(dev.address().as_deref(), Some("MIDI port 'loopMIDI'"), "a MIDI port has an address of its own, so the Doors check does not call it a missing host and port");
        let mut blank = dev.clone();
        blank.host = None;
        assert_eq!(blank.address().as_deref(), Some("the first MIDI output port"));
        let lines = describe(&dev, &a("note 60 90 ch 2")).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, "MIDI out to loopMIDI: note on 60 velocity 90 channel 2");
        assert_eq!(lines[0].1, vec![json!("91 3C 5A")]);
        assert!(lines[1].0.contains("note off 60 channel 2 (after 100 ms)"), "{}", lines[1].0);
        assert_eq!(describe(&dev, &a("panic")).unwrap().len(), 1, "sixteen messages are described as one");
        assert!(describe(&dev, &a("stop")).is_err());
    }

    #[cfg(not(feature = "midi"))]
    #[tokio::test]
    async fn a_build_without_midi_says_so_and_still_checks_the_value_first() {
        let m = MidiOut::default();
        let dev = Device { id: "m1".into(), label: "Desk MIDI".into(), kind: KIND.into(), dept: "lighting".into(), host: None, port: None, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None };
        let e = m.apply(&dev, &Action::new("lighting.look", json!({"look": "pc 4"}))).await.unwrap_err();
        assert!(e.contains("--features midi"), "{e}");
        let e = m.apply(&dev, &Action::new("lighting.look", json!({"look": "start"}))).await.unwrap_err();
        assert!(e.contains("is not one of"), "a bad value is reported as a bad value, not as a missing library: {e}");
        assert!(!BUILT);
    }
}
