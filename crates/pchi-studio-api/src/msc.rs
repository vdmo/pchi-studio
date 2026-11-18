//! MIDI Show Control (MSC): the standard system-exclusive message a lighting or sound desk takes to GO, STOP or load a cue.
//!
//! The layout (MIDI Show Control 1.0): `F0 7F <device> 02 <command format> <command> [cue [00 list [00 path]]] F7`, where the cue
//! number, list and path are ASCII digits and dots. This file builds and reads that layout and nothing else.
//!
//! **A closed set.** Only these can be built:
//!
//! | what | allowed |
//! |---|---|
//! | command | `go`, `stop`, `resume`, `load`, `all-off`, `restore`, `reset` |
//! | format | `lighting` (01), `moving-lights` (02), `sound` (10), `video` (30) |
//! | device | 0 to 111, one device. Not a group (112 to 126) and not all-call (127) |
//! | cue, list | digits and dots, up to 16 characters |
//!
//! Not built, on purpose: timed go, set, fire, go-off; pyrotechnics, machinery, rigging, lasers and every other command format. A cue
//! cannot put its own bytes on the wire. MSC has no reply, so "sent" means handed to the port.
//!
//! The layout comes from memory of the MSC specification, with no copy of the document to hand, so it is marked as such wherever it is
//! described. Nothing here has been seen to move a real desk.

/// What the message asks the desk to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Go,
    Stop,
    Resume,
    Load,
    AllOff,
    Restore,
    Reset,
}

impl Command {
    pub const ALL: [Command; 7] = [Command::Go, Command::Stop, Command::Resume, Command::Load, Command::AllOff, Command::Restore, Command::Reset];

    pub fn code(self) -> u8 {
        match self {
            Command::Go => 0x01,
            Command::Stop => 0x02,
            Command::Resume => 0x03,
            Command::Load => 0x05,
            Command::AllOff => 0x08,
            Command::Restore => 0x09,
            Command::Reset => 0x0A,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Command::Go => "go",
            Command::Stop => "stop",
            Command::Resume => "resume",
            Command::Load => "load",
            Command::AllOff => "all-off",
            Command::Restore => "restore",
            Command::Reset => "reset",
        }
    }

    pub fn from_name(w: &str) -> Option<Command> {
        let w = w.to_ascii_lowercase().replace('_', "-");
        Command::ALL.into_iter().find(|c| c.name() == w || (*c == Command::AllOff && w == "alloff"))
    }

    pub fn from_code(b: u8) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.code() == b)
    }

    /// Whether the command carries a cue number. LOAD needs one; GO, STOP and RESUME may have one; the rest take none.
    fn cue_use(self) -> Cue {
        match self {
            Command::Load => Cue::Needed,
            Command::Go | Command::Stop | Command::Resume => Cue::Optional,
            _ => Cue::Never,
        }
    }
}

enum Cue {
    Needed,
    Optional,
    Never,
}

/// The kind of equipment the message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Lighting,
    MovingLights,
    Sound,
    Video,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Lighting, Format::MovingLights, Format::Sound, Format::Video];

    pub fn code(self) -> u8 {
        match self {
            Format::Lighting => 0x01,
            Format::MovingLights => 0x02,
            Format::Sound => 0x10,
            Format::Video => 0x30,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Lighting => "lighting",
            Format::MovingLights => "moving-lights",
            Format::Sound => "sound",
            Format::Video => "video",
        }
    }

    pub fn from_name(w: &str) -> Option<Format> {
        let w = w.to_ascii_lowercase().replace('_', "-");
        Format::ALL.into_iter().find(|f| f.name() == w)
    }

    pub fn from_code(b: u8) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.code() == b)
    }
}

/// One MSC message.
#[derive(Debug, Clone, PartialEq)]
pub struct Msc {
    pub device: u8,
    pub format: Format,
    pub command: Command,
    pub cue: Option<String>,
    pub list: Option<String>,
}

/// A cue number or list: digits and dots, starting and ending with a digit, no empty parts, up to 16 characters.
pub fn valid_number(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

impl Msc {
    /// Check the message is one this module may build.
    pub fn check(&self) -> Result<(), String> {
        if self.device > 111 {
            return Err(format!("device {} is not 0 to 111: a group or all-call message could move every desk at once", self.device));
        }
        for (what, v) in [("cue", &self.cue), ("list", &self.list)] {
            if let Some(v) = v {
                if !valid_number(v) {
                    return Err(format!("{what} '{v}' is not a number such as 5 or 12.5 (digits and dots, up to 16 characters)"));
                }
            }
        }
        if self.list.is_some() && self.cue.is_none() {
            return Err("a cue list needs a cue number".into());
        }
        match (self.command.cue_use(), &self.cue) {
            (Cue::Needed, None) => Err(format!("{} needs a cue number", self.command.name())),
            (Cue::Never, Some(_)) => Err(format!("{} takes no cue number", self.command.name())),
            _ => Ok(()),
        }
    }

    /// The bytes to send, as a whole system-exclusive message.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.check()?;
        let mut b = vec![0xF0, 0x7F, self.device, 0x02, self.format.code(), self.command.code()];
        if let Some(c) = &self.cue {
            b.extend(c.bytes());
            if let Some(l) = &self.list {
                b.push(0x00);
                b.extend(l.bytes());
            }
        }
        b.push(0xF7);
        Ok(b)
    }

    /// In words, for a person.
    pub fn words(&self) -> String {
        let mut s = format!("MSC {} {}", self.format.name(), self.command.name());
        if let Some(c) = &self.cue {
            s += &format!(" cue {c}");
        }
        if let Some(l) = &self.list {
            s += &format!(" list {l}");
        }
        s + &format!(" device {}", self.device)
    }
}

/// Read a message heard on a port. `None` for anything that is not a plain MSC message of a kind this module knows (a different
/// sysex, a command or format it does not build, a malformed cue number).
pub fn decode(b: &[u8]) -> Option<Msc> {
    if b.len() < 7 || b[0] != 0xF0 || b[1] != 0x7F || b[3] != 0x02 || *b.last()? != 0xF7 {
        return None;
    }
    let (device, format, command) = (b[2], Format::from_code(b[4])?, Command::from_code(b[5])?);
    let data = &b[6..b.len() - 1];
    if data.iter().any(|x| *x >= 0x80) {
        return None;
    }
    let mut parts = data.split(|x| *x == 0x00).map(|p| String::from_utf8_lossy(p).to_string());
    let cue = parts.next().filter(|s| !s.is_empty());
    let list = parts.next().filter(|s| !s.is_empty());
    let m = Msc { device, format, command, cue, list };
    // a message heard is held to the same rules as one sent, except for the device: a group or all-call reaches us too
    if m.cue.as_deref().is_some_and(|c| !valid_number(c)) || m.list.as_deref().is_some_and(|l| !valid_number(l)) {
        return None;
    }
    Some(m)
}

/// Read the words of a cue's value after `msc`: `go 5`, `load 12.5 list 2 dev 3 fmt sound`, `all-off`.
pub fn from_words(words: &[String]) -> Result<Msc, String> {
    let hint = || "write msc <go|stop|resume|load|all-off|restore|reset> [cue] [list N] [dev N] [fmt lighting|moving-lights|sound|video]".to_string();
    let mut it = words.iter();
    let cmd = it.next().ok_or_else(hint)?;
    let command = Command::from_name(cmd).ok_or_else(|| format!("'{cmd}' is not an MSC command this build sends. {}", hint()))?;
    let (mut cue, mut list, mut device, mut format) = (None, None, 0u8, Format::Lighting);
    let mut rest: Vec<&String> = it.collect();
    rest.reverse();
    while let Some(w) = rest.pop() {
        match w.as_str() {
            "list" | "dev" | "device" | "fmt" | "format" => {
                let v = rest.pop().ok_or_else(|| format!("'{w}' needs a value. {}", hint()))?;
                match w.as_str() {
                    "list" => list = Some(v.clone()),
                    "dev" | "device" => device = v.parse().map_err(|_| format!("device '{v}' is not a whole number 0 to 111"))?,
                    _ => format = Format::from_name(v).ok_or_else(|| format!("format '{v}' is not one of lighting, moving-lights, sound, video"))?,
                }
            }
            _ if cue.is_none() => cue = Some(w.clone()),
            _ => return Err(format!("'{w}' is not understood. {}", hint())),
        }
    }
    let m = Msc { device, format, command, cue, list };
    m.check()?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn go_with_a_cue_is_the_documented_layout() {
        let m = from_words(&w("go 5")).unwrap();
        // F0 7F dev 02 fmt cmd "5" F7
        assert_eq!(m.encode().unwrap(), [0xF0, 0x7F, 0x00, 0x02, 0x01, 0x01, 0x35, 0xF7]);
        let m = from_words(&w("go 12.5 list 3 dev 7 fmt sound")).unwrap();
        assert_eq!(m.encode().unwrap(), [0xF0, 0x7F, 0x07, 0x02, 0x10, 0x01, b'1', b'2', b'.', b'5', 0x00, b'3', 0xF7]);
    }

    #[test]
    fn each_command_has_its_code_and_cue_rules() {
        let code = |s: &str| from_words(&w(s)).unwrap().encode().unwrap()[5];
        assert_eq!(code("go"), 0x01);
        assert_eq!(code("stop"), 0x02);
        assert_eq!(code("resume 4"), 0x03);
        assert_eq!(code("load 4"), 0x05);
        assert_eq!(code("all-off"), 0x08);
        assert_eq!(code("ALL_OFF"), 0x08);
        assert_eq!(code("restore"), 0x09);
        assert_eq!(code("reset"), 0x0A);
        assert_eq!(from_words(&w("go")).unwrap().encode().unwrap(), [0xF0, 0x7F, 0, 2, 1, 1, 0xF7], "GO with no cue goes the next one");
        assert!(from_words(&w("load")).unwrap_err().contains("needs a cue"));
        assert!(from_words(&w("all-off 3")).unwrap_err().contains("takes no cue"));
        assert!(from_words(&w("reset 1")).is_err());
    }

    #[test]
    fn formats_are_a_short_closed_list() {
        let fmt = |s: &str| from_words(&w(&format!("go fmt {s}"))).unwrap().encode().unwrap()[4];
        assert_eq!((fmt("lighting"), fmt("moving-lights"), fmt("sound"), fmt("video")), (0x01, 0x02, 0x10, 0x30));
        for bad in ["pyro", "fireworks", "machinery", "rigging", "lasers", "all", "7f", "60"] {
            assert!(from_words(&w(&format!("go fmt {bad}"))).is_err(), "{bad}");
        }
    }

    #[test]
    fn groups_and_all_call_are_refused() {
        assert!(from_words(&w("go dev 111")).is_ok());
        for d in ["112", "126", "127", "128", "255", "-1", "x"] {
            assert!(from_words(&w(&format!("go dev {d}"))).is_err(), "dev {d}");
        }
    }

    #[test]
    fn cue_numbers_are_plain_numbers() {
        for ok in ["1", "12.5", "0.5", "100.25.3"] {
            assert!(valid_number(ok), "{ok}");
        }
        for bad in ["", ".", "1.", ".5", "1..2", "a", "1,5", "-1", "1 2", &"1".repeat(17)] {
            assert!(!valid_number(bad), "{bad}");
        }
        assert!(from_words(&w("go list 2")).unwrap_err().contains("needs a cue"));
        assert!(from_words(&w("go 1 2")).is_err());
        assert!(from_words(&w("timed-go 1")).is_err());
        assert!(from_words(&w("fire 1")).is_err());
        assert!(from_words(&w("set 1")).is_err());
        assert!(from_words(&w("")).is_err());
    }

    #[test]
    fn what_is_built_is_read_back_the_same() {
        for v in ["go", "go 5", "stop", "resume 2.5", "load 7 list 2", "all-off", "restore dev 3 fmt video", "reset fmt moving-lights"] {
            let m = from_words(&w(v)).unwrap();
            assert_eq!(decode(&m.encode().unwrap()), Some(m.clone()), "{v}");
        }
    }

    #[test]
    fn decode_refuses_what_is_not_plain_msc() {
        assert_eq!(decode(&[0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7]), None, "general MIDI on is not MSC");
        assert_eq!(decode(&[0xF0, 0x7F, 0, 0x02, 0x01, 0x01, 0x35]), None, "no end byte");
        assert_eq!(decode(&[0xF0, 0x7F, 0, 0x02, 0x60, 0x01, 0xF7]), None, "a pyro format is not read");
        assert_eq!(decode(&[0xF0, 0x7F, 0, 0x02, 0x01, 0x04, 0xF7]), None, "timed go is not read");
        assert_eq!(decode(&[0xF0, 0x7F, 0, 0x02, 0x01, 0x01, b'a', 0xF7]), None, "a cue that is not a number");
        assert_eq!(decode(&[0xF0, 0x7F, 0, 0x01, 0x01, 0x01, 0xF7]), None, "sub-id 01 is a different message");
        assert_eq!(decode(&[0xF0, 0x7F, 0x7F, 0x02, 0x01, 0x01, 0xF7]).map(|m| m.device), Some(0x7F), "all-call is heard");
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn it_reads_in_words() {
        assert_eq!(from_words(&w("load 7 list 2 dev 3 fmt sound")).unwrap().words(), "MSC sound load cue 7 list 2 device 3");
    }
}
