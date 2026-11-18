//! Serial output (RS-232, and RS-485 through an adapter): a cue becomes one line of text written to a serial port, for projectors, video
//! switchers, relay boards and other gear that takes commands that way.
//!
//! The commands come from a **closed list in a profile file** (`"transport": "serial"`): the line settings (baud rate, data bits, parity,
//! stop bits), and per cue kind a text template, a line ending and optionally the reply to expect. A cue supplies only a value, checked
//! against the action's pattern; a value cannot contain a control character, so it cannot end the line early or add a second command.
//! **A cue cannot choose the port or the line settings.** The port is the device's `host` (`COM3`, `/dev/ttyUSB0`), and only names
//! that look like a serial port are accepted, so the field cannot point at some other file.
//!
//! - Text lines only: no binary bytes (no STX/ETX framing, no checksums). A device that needs them is not covered.
//! - RS-485 is a serial port to this code: use a USB adapter that switches direction by itself. Nothing here drives a direction pin.
//! - With no `expect`, `sent` means the bytes were handed to the port: serial gives no acknowledgement. With an `expect`, the first reply
//!   line must match it for `accepted`; a different line is `refused` with the device's words, and no line is `failed`.
//! - Needs a build with `--features serial`. Without it a cue says so, and the Doors check fails the device before the show.
//! - Laser and motion profiles are refused, as for the other profile-driven outputs.

use crate::httpout::{placeholders, render, Enc};
use crate::rig::Device;
use crate::textlink::{LinkError, Outcome};
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Whether this build can write to serial ports.
pub const BUILT: bool = cfg!(feature = "serial");

const VALUE_MAX: usize = 64;
const LINE_MAX: usize = 200;
const BAUDS: [u32; 9] = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    value: Option<String>,
    send: String,
    /// `cr` (default), `lf`, `crlf` or `none`.
    #[serde(default)]
    end: Option<String>,
    /// A pattern the first reply line must match, whole.
    #[serde(default)]
    expect: Option<String>,
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
    baud: u32,
    #[serde(default)]
    data_bits: Option<u8>,
    /// `none` (default), `even` or `odd`.
    #[serde(default)]
    parity: Option<String>,
    #[serde(default)]
    stop_bits: Option<u8>,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    notes: String,
    kinds: Vec<String>,
    actions: HashMap<String, RawAction>,
}

#[derive(Debug)]
pub struct SerialAction {
    pub value: Option<Regex>,
    pub send: String,
    pub end: &'static [u8],
    pub expect: Option<Regex>,
    pub basis: String,
    pub source: String,
}

#[derive(Debug)]
pub struct SerialProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: &'static str,
    pub stop_bits: u8,
    pub hint: String,
    pub notes: String,
    pub kinds: Vec<String>,
    pub actions: HashMap<String, SerialAction>,
    pub built_in: bool,
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

impl SerialProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<SerialProfile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a valid serial profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("profile id '{id}' must be letters, digits, - or _ (up to 40)"));
        }
        if !matches!(raw.dept.as_str(), "visual" | "lighting" | "audio") {
            return Err(format!("{id}: dept '{}' is not one of visual, lighting, audio (a laser or motion device needs an acknowledgement and a stop, which a serial line does not reliably give)", raw.dept));
        }
        if !BAUDS.contains(&raw.baud) {
            return Err(format!("{id}: baud {} is not one of {}", raw.baud, BAUDS.map(|b| b.to_string()).join(", ")));
        }
        let data_bits = raw.data_bits.unwrap_or(8);
        if !(data_bits == 7 || data_bits == 8) {
            return Err(format!("{id}: data_bits is 7 or 8"));
        }
        let parity: &'static str = match raw.parity.as_deref() {
            None | Some("none") => "none",
            Some("even") => "even",
            Some("odd") => "odd",
            Some(p) => return Err(format!("{id}: parity '{p}' is none, even or odd")),
        };
        let stop_bits = raw.stop_bits.unwrap_or(1);
        if !(stop_bits == 1 || stop_bits == 2) {
            return Err(format!("{id}: stop_bits is 1 or 2"));
        }
        if raw.actions.is_empty() {
            return Err(format!("{id}: no actions"));
        }
        let mut actions = HashMap::new();
        for (kind, a) in raw.actions {
            let what = format!("{id} {kind}");
            if a.send.is_empty() || a.send.len() > LINE_MAX || a.send.chars().any(|c| !(c.is_ascii() && !c.is_control())) {
                return Err(format!("{what}: a command is up to {LINE_MAX} printable ASCII characters (the line ending is set by `end`)"));
            }
            let end: &'static [u8] = match a.end.as_deref() {
                None | Some("cr") => b"\r",
                Some("lf") => b"\n",
                Some("crlf") => b"\r\n",
                Some("none") => b"",
                Some(e) => return Err(format!("{what}: end '{e}' is cr, lf, crlf or none")),
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
            let expect = a.expect.as_deref().map(full).transpose().map_err(|e| format!("{what}: {e}"))?;
            if !matches!(a.basis.as_str(), "vendor" | "community" | "memory") {
                return Err(format!("{what}: basis '{}' is not vendor, community or memory (nothing here may call itself proven)", a.basis));
            }
            actions.insert(kind, SerialAction { value, send: a.send, end, expect, basis: a.basis, source: a.source });
        }
        for k in &raw.kinds {
            if !actions.contains_key(k) {
                return Err(format!("{id}: kind '{k}' has no action"));
            }
        }
        Ok(SerialProfile { id, name: raw.name, dept: raw.dept, baud: raw.baud, data_bits, parity, stop_bits, hint: raw.hint, notes: raw.notes, kinds: raw.kinds, actions, built_in })
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<SerialProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<SerialProfile>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn get(id: &str) -> Option<Arc<SerialProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

/// Load the serial profiles among a folder's files; `profiles::load_dir` leaves those to this. There is no built-in one: the commands
/// are the device's own.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(SerialProfile::from_json(&item.to_string(), false).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.contains_key(&p.id);
        lines.push(format!("{} ({}), a serial device{}", p.name, p.id, if replaced { ", replaces an earlier one" } else { "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

pub fn is_serial_json(v: &Value) -> bool {
    v.get("transport").and_then(Value::as_str) == Some("serial")
}

/// A port name that looks like a serial port: `COM3`, `/dev/ttyUSB0`, `/dev/ttyACM0`, `/dev/cu.usbserial-1410`, or a pseudo-terminal
/// (`/dev/pts/3`, which virtual serial pairs use). Nothing else is opened.
pub fn port_ok(name: &str) -> bool {
    let n = name.trim();
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = n.strip_prefix("COM") {
        return digits(rest);
    }
    if let Some(rest) = n.strip_prefix("/dev/pts/") {
        return digits(rest);
    }
    let tail = n.strip_prefix("/dev/tty").or_else(|| n.strip_prefix("/dev/cu."));
    tail.is_some_and(|t| !t.is_empty() && t.len() <= 40 && t.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)))
}

/// What goes on the line for a cue, and the reply to expect.
#[derive(Debug)]
pub struct Line {
    pub text: String,
    pub end: &'static [u8],
    pub expect: Option<Regex>,
}

impl Line {
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = self.text.clone().into_bytes();
        b.extend_from_slice(self.end);
        b
    }
}

/// Turn a cue into the line the profile makes of it, or say what is wrong with the cue.
pub fn line_for(p: &SerialProfile, action: &Action) -> Result<Line, String> {
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
    Ok(Line { text, end: a.end, expect: a.expect.clone() })
}

fn profile_of(dev: &Device) -> Result<Arc<SerialProfile>, String> {
    get(dev.profile_key()).ok_or_else(|| format!("{} has no serial profile", dev.label))
}

fn port_of(dev: &Device) -> Result<String, String> {
    let name = dev.host.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(format!("{} has no serial port in the rig (put the port in host, for example COM3 or /dev/ttyUSB0)", dev.label));
    }
    if !port_ok(name) {
        return Err(format!("{}: '{name}' is not a serial port name (COM1 to COM999, /dev/ttyUSB0, /dev/ttyACM0, /dev/cu.usbserial-..., /dev/pts/N)", dev.label));
    }
    Ok(name.to_string())
}

/// Check a cue for a serial device. Sends nothing.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    line_for(&*profile_of(dev)?, action).map(|_| ())
}

fn show(b: &[u8]) -> String {
    b.iter().map(|c| match c {
        b'\r' => "\\r".to_string(),
        b'\n' => "\\n".to_string(),
        c => (*c as char).to_string(),
    }).collect()
}

/// What sending the cue puts on the line, in words.
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let p = profile_of(dev)?;
    let l = line_for(&p, action)?;
    let port = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or("(no port)");
    let par = match p.parity { "even" => "E", "odd" => "O", _ => "N" };
    Ok(vec![(format!("{} serial {port} {} {}{par}{}: {}", p.name, p.baud, p.data_bits, p.stop_bits, show(&l.bytes())), vec![])])
}

/// What the Doors check says about a profile whose rules do not rest on the vendor's document.
pub fn weak_note(dev: &Device) -> Option<String> {
    let p = get(dev.profile_key())?;
    crate::httpout::weak_note(&dev.label, &p.name, p.actions.values().map(|a| a.basis.as_str()))
}

#[cfg(feature = "serial")]
mod port {
    use super::*;
    use serialport::{DataBits, Parity, StopBits};
    use std::io::{Read, Write};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    type Conns = Arc<Mutex<HashMap<String, (String, Box<dyn serialport::SerialPort>)>>>;

    #[derive(Default)]
    pub struct Serial {
        conns: Conns,
    }

    fn open(path: &str, p: &SerialProfile) -> Result<Box<dyn serialport::SerialPort>, String> {
        serialport::new(path, p.baud)
            .data_bits(if p.data_bits == 7 { DataBits::Seven } else { DataBits::Eight })
            .parity(match p.parity { "even" => Parity::Even, "odd" => Parity::Odd, _ => Parity::None })
            .stop_bits(if p.stop_bits == 2 { StopBits::Two } else { StopBits::One })
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| format!("cannot open {path}: {e}"))
    }

    /// Read one line (up to a CR or LF after some text), for at most `wait`.
    fn read_line(port: &mut Box<dyn serialport::SerialPort>, wait: Duration) -> String {
        let (start, mut got) = (Instant::now(), Vec::new());
        let mut buf = [0u8; 128];
        while start.elapsed() < wait {
            match port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    got.extend_from_slice(&buf[..n]);
                    if got.iter().any(|b| *b == b'\r' || *b == b'\n') && got.iter().any(|b| !b.is_ascii_whitespace()) {
                        break;
                    }
                }
                _ => {}
            }
        }
        String::from_utf8_lossy(&got).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
    }

    impl Serial {
        pub async fn apply(&self, dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
            let p = profile_of(dev).map_err(LinkError::Failed)?;
            let line = line_for(&p, action).map_err(LinkError::Failed)?;
            let path = port_of(dev).map_err(LinkError::Failed)?;
            let (conns, id, label) = (Arc::clone(&self.conns), dev.id.clone(), dev.label.clone());
            let key = format!("{path}|{}|{}|{}|{}", p.baud, p.data_bits, p.parity, p.stop_bits);
            let bytes = line.bytes();
            let (expect, text, port_name) = (line.expect.clone(), line.text.clone(), path.clone());
            let p2 = Arc::clone(&p);
            tokio::task::spawn_blocking(move || {
                let mut map = conns.lock().unwrap();
                if map.get(&id).is_some_and(|(k, _)| *k != key) {
                    map.remove(&id);
                }
                if !map.contains_key(&id) {
                    map.insert(id.clone(), (key, open(&path, &p2).map_err(LinkError::Failed)?));
                }
                let port = &mut map.get_mut(&id).ok_or_else(|| LinkError::Failed("no port".into()))?.1;
                if expect.is_some() {
                    let _ = port.clear(serialport::ClearBuffer::Input);
                }
                if let Err(e) = port.write_all(&bytes).and_then(|_| port.flush()) {
                    map.remove(&id);
                    return Err(LinkError::Failed(format!("{label}: writing to {path} failed: {e}")));
                }
                match &expect {
                    None => Ok(("sent", format!("wrote {text:?} to {path}: serial gives no acknowledgement"))),
                    Some(re) => {
                        let reply = read_line(port, Duration::from_millis(1000));
                        if reply.is_empty() {
                            Err(LinkError::Failed(format!("{label} wrote nothing back within a second of {text:?}")))
                        } else if re.is_match(&reply) {
                            Ok(("accepted", format!("wrote {text:?}; it answered {reply:?}")))
                        } else {
                            Err(LinkError::Refused(format!("{label} answered {reply:?} to {text:?}")))
                        }
                    }
                }
            })
            .await
            .map_err(|e| LinkError::Failed(format!("serial task failed: {e}")))?
            .map(|(status, note)| Outcome { status, note, target: port_name })
        }
    }
}

#[cfg(feature = "serial")]
pub use port::Serial;

/// Open serial ports, kept between cues. This build cannot write to them: build with `--features serial`.
#[cfg(not(feature = "serial"))]
#[derive(Default)]
pub struct Serial;

#[cfg(not(feature = "serial"))]
impl Serial {
    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
        let p = profile_of(dev).map_err(LinkError::Failed)?;
        line_for(&p, action).map_err(LinkError::Failed)?;
        port_of(dev).map_err(LinkError::Failed)?;
        Err(LinkError::Failed(format!("{} is a serial device, and this build cannot write to serial ports. Build with: cargo build --release -p pchi-studio-api --features serial", dev.label)))
    }
}

/// The serial ports this machine reports (empty without `--features serial`).
pub fn list_ports() -> Vec<String> {
    #[cfg(feature = "serial")]
    {
        if let Ok(ports) = serialport::available_ports() {
            return ports.into_iter().map(|p| p.port_name).collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(actions: &str) -> String {
        format!(r#"{{"id":"x","name":"X","transport":"serial","dept":"visual","baud":9600,"kinds":[],"actions":{actions}}}"#)
    }
    fn bad(actions: &str) -> String {
        SerialProfile::from_json(&profile(actions), false).err().unwrap_or_default()
    }
    fn act(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"v": v}))
    }
    const PROJECTOR: &str = r#"{
      "id": "test-proj", "name": "Test projector", "transport": "serial", "dept": "visual",
      "baud": 9600, "data_bits": 8, "parity": "none", "stop_bits": 1,
      "kinds": ["visual.power_on", "visual.input"],
      "hint": "an input number, 1 to 9",
      "notes": "test",
      "actions": {
        "visual.power_on": {"send": "PON", "expect": "PON|ACK", "basis": "vendor", "source": "test"},
        "visual.input": {"value": "([1-9])", "send": "IN{1}", "end": "crlf", "basis": "vendor", "source": "test"}
      }
    }"#;
    fn dev(port: &str) -> Device {
        Device { id: "s1".into(), label: "Proj".into(), kind: "test-proj".into(), dept: "visual".into(), host: Some(port.into()), port: None, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }
    fn load() {
        let _ = load_values(vec![serde_json::from_str(PROJECTOR).unwrap()], "test").unwrap();
    }

    #[test]
    fn only_serial_port_names_are_accepted() {
        for ok in ["COM1", "COM12", "COM999", "/dev/ttyUSB0", "/dev/ttyACM1", "/dev/ttyS0", "/dev/cu.usbserial-1410", "/dev/pts/3"] {
            assert!(port_ok(ok), "{ok}");
        }
        for bad in ["", "COM", "COM1000", "COMx", "com1", "/etc/passwd", "/dev/sda", "/dev/null", "/dev/tty", "/dev/ttyUSB0/../sda", "/dev/pts/", "/dev/pts/x", "../COM1", "COM1 ", "tty0", "/dev/tty USB", "\\\\.\\COM1"] {
            assert!(!port_ok(bad) || bad == "COM1 ", "{bad:?}");
        }
        assert!(port_ok(" COM3 "), "surrounding spaces are trimmed");
        assert!(port_of(&dev("/etc/passwd")).unwrap_err().contains("not a serial port name"));
        assert!(port_of(&dev("")).unwrap_err().contains("no serial port"));
    }

    #[test]
    fn a_cue_makes_the_line_the_profile_says() {
        load();
        let p = get("test-proj").unwrap();
        let l = line_for(&p, &act("visual.power_on", "")).unwrap();
        assert_eq!(l.bytes(), b"PON\r");
        assert_eq!(line_for(&p, &act("visual.input", "3")).unwrap().bytes(), b"IN3\r\n");
        for v in ["0", "10", "a", "3\rPOFF", "3\nPOFF", ""] {
            assert!(line_for(&p, &act("visual.input", v)).is_err(), "{v:?}");
        }
        assert!(line_for(&p, &act("visual.power_on", "x")).unwrap_err().contains("takes no value"));
        assert!(line_for(&p, &act("visual.other", "")).unwrap_err().contains("does not take"));
    }

    #[test]
    fn a_value_cannot_end_the_line_or_add_a_command() {
        let t = r#"{"k":{"value":"(.{1,30})","send":"SAY {1}"}}"#;
        let p = SerialProfile::from_json(&profile(t), false).unwrap();
        for v in ["a\rPOFF", "a\nb", "a\u{0}b", "é"] {
            assert!(line_for(&p, &act("k", v)).is_err(), "{v:?}");
        }
        assert_eq!(line_for(&p, &act("k", "hello")).unwrap().bytes(), b"SAY hello\r");
    }

    #[test]
    fn a_profile_can_only_say_what_is_safe() {
        assert!(bad(r#"{"k":{"send":""}}"#).contains("up to"));
        assert!(bad(r#"{"k":{"send":"A\rB"}}"#).contains("printable"));
        assert!(bad(r#"{"k":{"send":"é"}}"#).contains("printable"));
        assert!(bad(r#"{"k":{"send":"A","end":"tab"}}"#).contains("cr, lf, crlf or none"));
        assert!(bad(r#"{"k":{"send":"A{0}"}}"#).contains("need a value pattern"));
        assert!(bad(r#"{"k":{"send":"A{2}","value":"(.)"}}"#).contains("no bracketed part 2"));
        assert!(bad(r#"{"k":{"send":"A","expect":"("}}"#).contains("pattern"));
        assert!(bad(r#"{"k":{"send":"A","basis":"proven"}}"#).contains("proven"));
        assert!(bad(r#"{"k":{"send":"A","extra":1}}"#).contains("unknown field"));
        assert!(bad("{}").contains("no actions"));
        for (from, to, why) in [("9600", "9601", "baud"), (r#""baud":9600"#, r#""baud":9600,"data_bits":9"#, "data_bits"), (r#""baud":9600"#, r#""baud":9600,"parity":"mark""#, "parity"), (r#""baud":9600"#, r#""baud":9600,"stop_bits":3"#, "stop_bits"), ("visual", "laser", "laser or motion")] {
            let t = profile(r#"{"k":{"send":"A"}}"#).replace(from, to);
            assert!(SerialProfile::from_json(&t, false).unwrap_err().contains(why), "{why}");
        }
    }

    #[test]
    fn a_cue_is_checked_and_described_without_sending() {
        load();
        assert!(is_serial_json(&serde_json::from_str(PROJECTOR).unwrap()));
        assert!(check_action(&dev("COM3"), &act("visual.input", "2")).is_ok());
        assert!(check_action(&dev("COM3"), &act("visual.input", "x")).is_err());
        let l = describe(&dev("/dev/ttyUSB0"), &act("visual.input", "2")).unwrap();
        assert_eq!(l[0].0, "Test projector serial /dev/ttyUSB0 9600 8N1: IN2\\r\\n");
        assert_eq!(dept_of("test-proj").as_deref(), Some("visual"));
        assert!(weak_note(&dev("COM3")).is_none(), "vendor-based rules are not flagged");
    }

    #[cfg(not(feature = "serial"))]
    #[tokio::test]
    async fn a_build_without_serial_says_so_and_checks_the_cue_first() {
        load();
        let s = Serial::default();
        let e = s.apply(&dev("COM3"), &act("visual.power_on", "")).await.unwrap_err();
        assert!(e.text().contains("--features serial"), "{}", e.text());
        assert!(s.apply(&dev("COM3"), &act("visual.input", "x")).await.unwrap_err().text().contains("not a value"));
        assert!(s.apply(&dev("/etc/passwd"), &act("visual.power_on", "")).await.unwrap_err().text().contains("not a serial port"));
        assert!(!BUILT);
    }

    /// With `--features serial` on Linux: a pseudo-terminal pair stands in for a cable, so the real port code writes to and reads from a
    /// real (virtual) serial device. It proves the code path, not any hardware's behaviour.
    #[cfg(all(feature = "serial", target_os = "linux"))]
    #[tokio::test]
    async fn the_port_code_writes_to_and_reads_from_a_pseudo_terminal() {
        use std::io::{Read, Write};
        use std::os::fd::FromRawFd;
        load();
        let (mut master, slave_path) = unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(m >= 0, "posix_openpt");
            assert_eq!(libc::grantpt(m), 0);
            assert_eq!(libc::unlockpt(m), 0);
            let mut buf = [0 as libc::c_char; 64];
            assert_eq!(libc::ptsname_r(m, buf.as_mut_ptr(), buf.len()), 0);
            let path = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().to_string();
            (std::fs::File::from_raw_fd(m), path)
        };
        assert!(port_ok(&slave_path), "{slave_path}");
        let s = Serial::default();
        // no reply expected: sent, and the exact bytes arrive on the other end
        let o = s.apply(&dev(&slave_path), &act("visual.input", "4")).await.unwrap();
        assert_eq!(o.status, "sent");
        let mut got = [0u8; 5];
        master.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"IN4\r\n");
        // a reply expected: the other end answers, and the answer decides
        let m2 = master.try_clone().unwrap();
        let answer = |reply: &'static [u8]| {
            let mut m = m2.try_clone().unwrap();
            std::thread::spawn(move || {
                let mut b = [0u8; 4];
                m.read_exact(&mut b).unwrap(); // PON\r
                m.write_all(reply).unwrap();
            })
        };
        let t = answer(b"ACK\r\n");
        assert_eq!(s.apply(&dev(&slave_path), &act("visual.power_on", "")).await.unwrap().status, "accepted");
        t.join().unwrap();
        let t = answer(b"ERR\r\n");
        let e = s.apply(&dev(&slave_path), &act("visual.power_on", "")).await.unwrap_err();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("ERR"), "{}", e.text());
        t.join().unwrap();
        // silence: failed
        let t = answer(b"");
        assert_eq!(s.apply(&dev(&slave_path), &act("visual.power_on", "")).await.unwrap_err().status(), "failed");
        t.join().unwrap();
        // a port that is not there
        assert_eq!(s.apply(&dev("/dev/ttyNOPE9"), &act("visual.input", "1")).await.unwrap_err().status(), "failed");
    }
}
