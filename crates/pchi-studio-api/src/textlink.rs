//! Devices that are driven by a line of text over TCP, described by a profile file: how a cue becomes commands, what reply means yes, how to
//! read the device's state, and what must never be sent. The first is the Universal Robots Dashboard Server (`profiles/link/ur.json`).
//!
//! Why a profile and not code: a robot's command language is the robot's, and a site may need its own (a KUKA controller running a KRL
//! receiver the integrator wrote has a command format only that site knows). The profile is a file a person can read and review; the
//! runtime only ever sends commands that appear in it.
//!
//! The safety design, in short:
//! * **A closed set of commands.** Every command that can ever be sent is written in the profile (actions, status reads, stop). A cue
//!   only chooses *which program name* goes into a template; it never supplies a command.
//! * **A `never` list.** A profile names commands it must never contain ("power on", "brake release", ...); a profile with a step that
//!   starts with one is refused when it is loaded.
//! * **Program names are an allow-list.** With `needs_programs`, a cue's value must be on the device's own list (`programs` in the rig
//!   node): the default is that nothing may be run. The name is also checked against a strict pattern, so it cannot carry a command.
//! * **Honest results.** `accepted` = the device answered the way the profile says means yes. `running` = a read-back shows it is doing
//!   it. `refused` = the device answered and said no. `failed` = no answer, or the connection broke. Never "done".
//! * **State is read, and a fault holds the show.** The device's state is polled. While it is not ready, cues to it are held, and an
//!   armed device is disarmed, so recovery needs a person to arm it again.
//! * **A late reply cannot be mistaken for the next one:** after a timeout the connection is dropped, not reused.
//!
//! What this is not: a safety system. It cannot know what a program does. See docs/motion.md.

use crate::rig::Device;
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const CONNECT: Duration = Duration::from_secs(2);
const GREETING: Duration = Duration::from_millis(400);
const REPLY: Duration = Duration::from_secs(3);
const BUSY: Duration = Duration::from_secs(4);

// ---- the profile, as written ----

#[derive(Deserialize)]
struct RawSkip {
    var: String,
    regex: String,
}

#[derive(Deserialize)]
struct RawStep {
    send: String,
    #[serde(default)]
    expect: Option<String>,
    #[serde(default)]
    fail: Option<String>,
    #[serde(default)]
    store: Option<String>,
    #[serde(default)]
    skip_if: Option<RawSkip>,
    #[serde(default)]
    within_ms: Option<u64>,
    #[serde(default)]
    every_ms: Option<u64>,
    /// Wait this long after the step succeeds (a PLC needs a moment to see a pulse before it is released).
    #[serde(default)]
    wait_ms: Option<u64>,
    /// This step changes something the action's `cleanup` must put back. (For Modbus every write counts, without saying so.)
    #[serde(default)]
    changes: bool,
}

#[derive(Deserialize)]
struct RawAction {
    steps: Vec<RawStep>,
    #[serde(default)]
    confirm: Option<RawStep>,
    /// Run, best effort, if any step fails: put back what the steps changed (release a start pulse). Never skipped by an error.
    #[serde(default)]
    cleanup: Vec<RawStep>,
    /// What this action's value may be (a whole-value pattern), instead of the profile's. Its bracketed parts are `{1}`, `{2}`... in the commands.
    #[serde(default)]
    value: Option<String>,
}

/// A greeting the device sends on connect that this profile cannot work with (a password challenge), and what to tell the person.
#[derive(Deserialize)]
struct RawRefuse {
    #[serde(rename = "match")]
    pattern: String,
    why: String,
}

#[derive(Deserialize)]
struct RawRead {
    /// Take the first group of `expect` (or the whole reply) as this variable.
    #[serde(default)]
    var: Option<String>,
    send: String,
    expect: String,
    /// Take several values out of the one reply: variable name -> a pattern whose first group is the value (searched anywhere in the reply).
    #[serde(default)]
    extract: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawReady {
    var: String,
    #[serde(default)]
    equals: Option<String>,
    #[serde(default)]
    one_of: Option<Vec<String>>,
    why: String,
}

#[derive(Deserialize)]
struct RawStatus {
    #[serde(default)]
    poll_ms: Option<u64>,
    reads: Vec<RawRead>,
    #[serde(default)]
    ready_when: Vec<RawReady>,
}

#[derive(Deserialize)]
struct RawLink {
    id: String,
    name: String,
    transport: String,
    dept: String,
    #[serde(default)]
    port: Option<u16>,
    kinds: Vec<String>,
    value: String,
    #[serde(default)]
    needs_programs: bool,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    never: Vec<String>,
    /// For "modbus-tcp": the unit (slave) id; 1 if not said.
    #[serde(default)]
    unit: Option<u8>,
    /// For "modbus-tcp": addresses this profile must never write, such as "coil:30-39" or "reg:100". A profile with a step that writes one is refused.
    #[serde(default)]
    never_addresses: Vec<String>,
    /// What ends a request on the wire: "\n" (the default), "\r\n", or "" for a device that frames its own messages (XML).
    #[serde(default)]
    request_end: Option<String>,
    /// For a device whose replies are not one line (XML): the text that ends a reply, for example "</Reply>". Without it a reply is one line.
    #[serde(default)]
    reply_until: Option<String>,
    #[serde(default)]
    greeting_refuse: Option<RawRefuse>,
    actions: BTreeMap<String, RawAction>,
    #[serde(default)]
    status: Option<RawStatus>,
    #[serde(default)]
    stop: Vec<RawStep>,
    #[serde(default)]
    basis: Option<String>,
    #[serde(default)]
    source: String,
    #[serde(default)]
    notes: String,
}

// ---- the profile, checked ----

struct Step {
    send: String,
    expect: Option<Regex>,
    fail: String,
    store: Option<String>,
    skip_if: Option<(String, String)>,
    within: Duration,
    every: Duration,
    wait: Duration,
    changes: bool,
}

struct Action2 {
    steps: Vec<Step>,
    confirm: Option<Step>,
    cleanup: Vec<Step>,
    /// This action's own value pattern, if it has one.
    value: Option<Regex>,
}

/// What a Modbus profile is told on the wire: the unit, and the addresses it can never write.
struct ModbusCfg {
    unit: u8,
    never: Vec<crate::modbus::Forbidden>,
}

/// What goes into a command template: the program's name or value (`{0}`), the bracketed parts of the value's pattern (`{1}` to `{9}`)
/// and, for a PLC that picks programs by number, its number (`{n}`).
struct Subst<'a> {
    name: &'a str,
    num: Option<u16>,
    caps: Vec<String>,
}

impl Subst<'_> {
    /// One pass over the template, so text that came out of a value is never read as a placeholder itself.
    fn render(&self, t: &str) -> String {
        let mut out = String::with_capacity(t.len() + 16);
        let mut it = t.chars().peekable();
        while let Some(c) = it.next() {
            if c == '{' {
                let rest: String = it.clone().take(2).collect();
                let mut cs = rest.chars();
                if let (Some(k), Some('}')) = (cs.next(), cs.next()) {
                    let put = match k {
                        '0' => Some(self.name.to_string()),
                        'n' => self.num.map(|n| n.to_string()),
                        '1'..='9' => Some(self.caps.get(k as usize - '1' as usize).cloned().unwrap_or_default()),
                        _ => None,
                    };
                    if let Some(p) = put {
                        out.push_str(&p);
                        it.next();
                        it.next();
                        continue;
                    }
                }
            }
            out.push(c);
        }
        out
    }
}

/// Whether a command takes the cue's value (or a part of it, or its program number).
fn uses_value(send: &str) -> bool {
    send.contains("{0}") || send.contains("{n}") || (1..=9).any(|i| send.contains(&format!("{{{i}}}")))
}

/// The bracketed parts of a value that fits the pattern.
fn caps_of(re: &Regex, v: &str) -> Vec<String> {
    re.captures(v).map(|c| (1..c.len()).map(|i| c.get(i).map_or(String::new(), |m| m.as_str().to_string())).collect()).unwrap_or_default()
}

struct Read {
    var: Option<String>,
    send: String,
    expect: Regex,
    extract: Vec<(String, Regex)>,
}

impl Read {
    /// The variables this reply gives, each `? (reply)` when it is not in the expected form.
    fn values(&self, reply: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if let Some(var) = &self.var {
            let v = self.expect.captures(reply).map(|c| c.get(1).map_or(reply.to_string(), |g| g.as_str().to_string()));
            out.insert(var.clone(), v.unwrap_or_else(|| format!("? ({reply})")));
        }
        for (name, re) in &self.extract {
            let v = re.captures(reply).and_then(|c| c.get(1)).map(|g| g.as_str().trim().to_string());
            out.insert(name.clone(), v.unwrap_or_else(|| format!("? ({reply})")));
        }
        out
    }

    /// Whether the reply is in the form expected: the whole-reply pattern matches, and every extraction found its value.
    fn matches(&self, reply: &str) -> bool {
        self.expect.is_match(reply) && self.extract.iter().all(|(_, re)| re.captures(reply).is_some_and(|c| c.get(1).is_some()))
    }
}

struct Ready {
    var: String,
    equals: Option<String>,
    one_of: Vec<String>,
    why: String,
}

pub struct LinkProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub port: Option<u16>,
    pub hint: String,
    pub basis: String,
    pub source: String,
    pub notes: String,
    pub built_in: bool,
    kinds: Vec<String>,
    value: Regex,
    needs_programs: bool,
    actions: BTreeMap<String, Action2>,
    reads: Vec<Read>,
    ready_when: Vec<Ready>,
    poll: Duration,
    stop: Vec<Step>,
    request_end: String,
    reply_until: Option<String>,
    greeting_refuse: Option<(Regex, String)>,
    modbus: Option<ModbusCfg>,
}

impl LinkProfile {
    /// The byte that ends a line the device sends on connect: a carriage return for a device whose replies end in one, else a line feed.
    fn line_byte(&self) -> u8 {
        if self.reply_until.as_deref() == Some("\r") {
            b'\r'
        } else {
            b'\n'
        }
    }

    /// Why this profile cannot work with the greeting the device sent, if it cannot.
    fn refuses_greeting(&self, greeting: &str) -> Option<String> {
        self.greeting_refuse.as_ref().filter(|(re, _)| re.is_match(greeting.trim())).map(|(_, why)| why.clone())
    }
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).dot_matches_new_line(true).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

/// A command template: printable text, with `{0}` (the program's name or the value), `{1}` to `{9}` (the bracketed parts of the value's
/// pattern) and `{n}` (the program's number) as the only placeholders.
fn template_ok(t: &str) -> Result<(), String> {
    if t.trim().is_empty() {
        return Err("an empty command".into());
    }
    if t.chars().any(|c| c.is_control()) {
        return Err(format!("'{t}' has a control character: one command is one line"));
    }
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        let after = &rest[i..];
        let ok = after.len() >= 3 && after.as_bytes()[2] == b'}' && matches!(after.as_bytes()[1], b'0'..=b'9' | b'n');
        if !ok {
            return Err(format!("'{t}': the only placeholders are {{0}} (the value), {{1}} to {{9}} (its bracketed parts) and {{n}} (the program number)"));
        }
        rest = &after[3..];
    }
    if rest.contains('}') {
        return Err(format!("'{t}': stray }}"));
    }
    Ok(())
}

fn step_of(r: &RawStep, what: &str) -> Result<Step, String> {
    template_ok(&r.send).map_err(|e| format!("{what}: {e}"))?;
    let expect = r.expect.as_deref().map(full).transpose().map_err(|e| format!("{what}: {e}"))?;
    if let Some(s) = &r.skip_if {
        // the pattern is built per cue (the program name goes in, escaped): check it compiles with a stand-in
        full(&s.regex.replace("{0}", "X")).map_err(|e| format!("{what} skip_if: {e}"))?;
    }
    Ok(Step {
        send: r.send.clone(),
        expect,
        fail: r.fail.clone().unwrap_or_else(|| format!("the device did not accept '{}' ({{reply}})", r.send)),
        store: r.store.clone(),
        skip_if: r.skip_if.as_ref().map(|s| (s.var.clone(), s.regex.clone())),
        within: Duration::from_millis(r.within_ms.unwrap_or(0).min(30_000)),
        every: Duration::from_millis(r.every_ms.unwrap_or(250).clamp(50, 5_000)),
        wait: Duration::from_millis(r.wait_ms.unwrap_or(0).min(5_000)),
        changes: r.changes,
    })
}

impl LinkProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<LinkProfile, String> {
        let raw: RawLink = serde_json::from_str(text).map_err(|e| format!("not a link profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("id '{id}' must be 1 to 32 characters: a to z, 0 to 9 and -"));
        }
        let is_modbus = match raw.transport.as_str() {
            "tcp-text" => false,
            "modbus-tcp" => true,
            _ => return Err(format!("{id}: transport must be \"tcp-text\" or \"modbus-tcp\"")),
        };
        if is_modbus && (raw.request_end.is_some() || raw.reply_until.is_some()) {
            return Err(format!("{id}: request_end and reply_until are for text devices: Modbus frames its own messages"));
        }
        if !is_modbus && (raw.unit.is_some() || !raw.never_addresses.is_empty()) {
            return Err(format!("{id}: unit and never_addresses are for \"modbus-tcp\""));
        }
        if !["visual", "lighting", "laser", "motion", "audio"].contains(&raw.dept.as_str()) {
            return Err(format!("{id}: dept must be visual, lighting, laser, motion or audio"));
        }
        if raw.kinds.is_empty() || raw.actions.is_empty() {
            return Err(format!("{id}: kinds and actions must not be empty"));
        }
        for k in raw.actions.keys() {
            if !raw.kinds.iter().any(|x| x == k) {
                return Err(format!("{id}: action '{k}' is not in kinds"));
            }
        }
        let value = full(&raw.value).map_err(|e| format!("{id} value: {e}"))?;
        let request_end = match raw.request_end.as_deref() {
            None | Some("\n") => "\n".to_string(),
            Some("\r\n") => "\r\n".to_string(),
            Some("\r") => "\r".to_string(),
            Some("") => String::new(),
            Some(other) => return Err(format!("{id}: request_end must be \"\\n\", \"\\r\\n\", \"\\r\" or \"\", not {other:?}")),
        };
        // a reply ends at a line feed, at text such as "</Reply>", or, for a device that ends its lines with a carriage return alone, at that
        let reply_until = match raw.reply_until.as_deref() {
            None => None,
            Some("\r") => Some("\r".to_string()),
            Some(u) if !u.is_empty() && u.len() <= 64 && !u.chars().any(|c| c.is_control()) => Some(u.to_string()),
            Some(u) => return Err(format!("{id}: reply_until {u:?} must be 1 to 64 printable characters, or a single carriage return")),
        };
        let greeting_refuse = match &raw.greeting_refuse {
            None => None,
            Some(g) if g.why.trim().is_empty() => return Err(format!("{id}: greeting_refuse needs a why")),
            Some(g) => Some((regex::RegexBuilder::new(&g.pattern).size_limit(1 << 20).build().map_err(|e| format!("{id} greeting_refuse: {e}"))?, g.why.clone())),
        };
        let never: Vec<String> = raw.never.iter().map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty()).collect();
        let mut sent: Vec<(String, String)> = Vec::new(); // (where, command)
        let mut actions = BTreeMap::new();
        for (kind, a) in &raw.actions {
            if a.steps.is_empty() {
                return Err(format!("{id} {kind}: no steps"));
            }
            let mut steps = Vec::new();
            for (n, s) in a.steps.iter().enumerate() {
                let at = format!("{id} {kind} step {}", n + 1);
                sent.push((at.clone(), s.send.clone()));
                steps.push(step_of(s, &at)?);
            }
            let confirm = match &a.confirm {
                Some(c) => {
                    let at = format!("{id} {kind} confirm");
                    sent.push((at.clone(), c.send.clone()));
                    let mut s = step_of(c, &at)?;
                    if s.expect.is_none() {
                        return Err(format!("{at}: a confirm needs an expect"));
                    }
                    if s.within.is_zero() {
                        s.within = Duration::from_secs(3);
                    }
                    Some(s)
                }
                None => None,
            };
            let mut cleanup = Vec::new();
            for (n, c) in a.cleanup.iter().enumerate() {
                let at = format!("{id} {kind} cleanup {}", n + 1);
                sent.push((at.clone(), c.send.clone()));
                cleanup.push(step_of(c, &at)?);
            }
            let own = a.value.as_deref().map(full).transpose().map_err(|e| format!("{id} {kind} value: {e}"))?;
            // every {1}..{9} in a command must be a bracketed part of the pattern that value is checked against
            let groups = own.as_ref().unwrap_or(&value).captures_len() - 1;
            for s in a.steps.iter().chain(a.confirm.iter()).chain(a.cleanup.iter()) {
                if let Some(k) = (1..=9).find(|k| s.send.contains(&format!("{{{k}}}")) && *k > groups) {
                    return Err(format!("{id} {kind}: '{}' uses {{{k}}} but the value pattern has {groups} bracketed part(s)", s.send));
                }
            }
            actions.insert(kind.clone(), Action2 { steps, confirm, cleanup, value: own });
        }
        let mut stop = Vec::new();
        for (n, s) in raw.stop.iter().enumerate() {
            let at = format!("{id} stop step {}", n + 1);
            if uses_value(&s.send) {
                return Err(format!("{at}: a stop has no program to put in"));
            }
            sent.push((at.clone(), s.send.clone()));
            stop.push(step_of(s, &at)?);
        }
        let (mut reads, mut ready_when, mut poll) = (Vec::new(), Vec::new(), Duration::from_secs(1));
        if let Some(st) = &raw.status {
            poll = Duration::from_millis(st.poll_ms.unwrap_or(1000).clamp(200, 30_000));
            for (n, r) in st.reads.iter().enumerate() {
                let at = format!("{id} status read {}", n + 1);
                template_ok(&r.send).map_err(|e| format!("{at}: {e}"))?;
                if uses_value(&r.send) {
                    return Err(format!("{at}: a status read has no program to put in"));
                }
                sent.push((at.clone(), r.send.clone()));
                if r.var.is_none() && r.extract.is_empty() {
                    return Err(format!("{at}: give a var, or extract"));
                }
                let mut extract = Vec::new();
                for (name, pat) in &r.extract {
                    let re = regex::RegexBuilder::new(pat).dot_matches_new_line(true).size_limit(1 << 20).build().map_err(|e| format!("{at} extract '{name}': {e}"))?;
                    extract.push((name.clone(), re));
                }
                reads.push(Read { var: r.var.clone(), send: r.send.clone(), expect: full(&r.expect).map_err(|e| format!("{at}: {e}"))?, extract });
            }
            for r in &st.ready_when {
                if !st.reads.iter().any(|x| x.var.as_deref() == Some(r.var.as_str()) || x.extract.contains_key(&r.var)) {
                    return Err(format!("{id} ready_when: '{}' is not read by any status read", r.var));
                }
                if r.equals.is_none() && r.one_of.is_none() {
                    return Err(format!("{id} ready_when '{}': give equals or one_of", r.var));
                }
                ready_when.push(Ready { var: r.var.clone(), equals: r.equals.clone(), one_of: r.one_of.clone().unwrap_or_default(), why: r.why.clone() });
            }
        }
        // a Modbus profile: every command must be a Modbus command, and none may write an address the profile forbids
        let modbus = if is_modbus {
            let never_addr: Vec<crate::modbus::Forbidden> = raw.never_addresses.iter().map(|a| crate::modbus::parse_forbidden(a)).collect::<Result<_, _>>().map_err(|e| format!("{id} never_addresses: {e}"))?;
            for (at, cmd) in &sent {
                let c = crate::modbus::parse(&cmd.replace("{0}", "A").replace("{n}", "1")).map_err(|e| format!("{at}: {e}"))?;
                if let Some(f) = never_addr.iter().find(|f| f.covers(&c)) {
                    return Err(format!("{at}: '{cmd}' writes an address this profile says it must never write ({:?} {} to {})", f.space, f.lo, f.hi));
                }
            }
            Some(ModbusCfg { unit: raw.unit.unwrap_or(1), never: never_addr })
        } else {
            None
        };
        // the never list: nothing the profile can send may begin with a forbidden command
        for (at, cmd) in &sent {
            let c = cmd.trim().to_ascii_lowercase();
            // a command is forbidden if it IS the phrase, begins with it, or (for XML) has it as an element: <power> or <power ...>
            if let Some(bad) = never.iter().find(|n| c == **n || c.starts_with(&format!("{n} ")) || c.contains(&format!("<{n}>")) || c.contains(&format!("<{n} ")) || c.contains(&format!("<{n}/"))) {
                return Err(format!("{at}: '{cmd}' is a command this profile says it must never send ('{bad}')"));
            }
        }
        Ok(LinkProfile {
            id,
            name: raw.name,
            dept: raw.dept,
            port: raw.port,
            hint: raw.hint,
            basis: raw.basis.unwrap_or_else(|| "memory".into()),
            source: raw.source,
            notes: raw.notes,
            built_in,
            kinds: raw.kinds,
            value,
            needs_programs: raw.needs_programs,
            actions,
            reads,
            ready_when,
            poll,
            stop,
            request_end,
            reply_until,
            greeting_refuse,
            modbus,
        })
    }

    /// Every command this profile can ever send, in the order a person would read them. This is the whole of what the device is ever told.
    pub fn commands(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for a in self.actions.values() {
            for s in a.steps.iter().chain(a.confirm.iter()).chain(a.cleanup.iter()) {
                v.push(s.send.clone());
            }
        }
        v.extend(self.stop.iter().map(|s| s.send.clone()));
        v.extend(self.reads.iter().map(|r| r.send.clone()));
        v.sort();
        v.dedup();
        v
    }

    pub fn handles(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind) && self.actions.contains_key(kind)
    }
}

// ---- the registry ----

const BUILT_IN: [&str; 2] = [include_str!("../profiles/link/ur.json"), include_str!("../profiles/link/pjlink.json")];

fn registry() -> &'static RwLock<HashMap<String, Arc<LinkProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<LinkProfile>>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut m = HashMap::new();
        for text in BUILT_IN {
            let p = LinkProfile::from_json(text, true).expect("a built-in link profile is valid (a test checks them all)");
            m.insert(p.id.clone(), Arc::new(p));
        }
        RwLock::new(m)
    })
}

pub fn get(id: &str) -> Option<Arc<LinkProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

pub fn list() -> Vec<Arc<LinkProfile>> {
    let mut v: Vec<_> = registry().read().unwrap().values().cloned().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Load the link profiles among a folder's files (a file whose `transport` is `tcp-text`); `profiles::load_dir` leaves those to this.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(LinkProfile::from_json(&item.to_string(), false).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.get(&p.id).map(|o| o.built_in);
        lines.push(format!("{} ({}), a text link{}", p.name, p.id, match replaced { Some(true) => ", replaces the built-in profile", Some(false) => ", replaces an earlier one", None => "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

/// Whether a file's JSON is a link profile (so the OSC loader leaves it alone).
pub fn is_link_json(v: &Value) -> bool {
    matches!(v.get("transport").and_then(Value::as_str), Some("tcp-text") | Some("modbus-tcp"))
}

// ---- what a cue would do, checked without sending anything ----

/// The program name a cue carries: the first payload value, as text.
pub fn value_of(action: &Action) -> String {
    match &action.payload {
        Value::Object(p) => match p.values().next() {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The entries of a device's allow-list (the rig node's `programs`: separated by commas, spaces or semicolons): a name, or for a PLC that
/// picks programs by number, `NAME=3`.
fn program_entries(dev: &Device) -> Vec<(String, Option<u16>)> {
    dev.programs.as_deref().unwrap_or("").split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(str::trim).filter(|s| !s.is_empty()).map(|e| match e.split_once('=') {
        Some((n, num)) => (n.trim().to_string(), num.trim().parse::<u16>().ok()),
        None => (e.to_string(), None),
    }).collect()
}

/// The names on a device's allow-list.
pub fn allowed_programs(dev: &Device) -> Vec<String> {
    program_entries(dev).into_iter().map(|(n, _)| n).collect()
}

/// The number a device's list gives a program (`OPEN_ARM=3`), if it gives one.
pub fn program_number(dev: &Device, name: &str) -> Option<u16> {
    program_entries(dev).into_iter().find(|(n, _)| n == name).and_then(|(_, num)| num)
}

fn uses_number(a: &Action2) -> bool {
    a.steps.iter().chain(a.confirm.iter()).chain(a.cleanup.iter()).any(|s| s.send.contains("{n}"))
}

/// Check a cue against the profile and the device's allow-list. Sends nothing. The error says what to fix.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    let p = get(dev.profile_key()).ok_or_else(|| format!("{} has no link profile", dev.label))?;
    let a = p.actions.get(&action.kind);
    if a.is_none() {
        return Err(format!("{} has no mapping for \"{}\"", p.name, action.kind));
    }
    // pause and stop take no program name; run does. A step uses {0} only if it needs one.
    let needs_value = a.is_some_and(|a| a.steps.iter().chain(a.confirm.iter()).chain(a.cleanup.iter()).any(|s| uses_value(&s.send) || s.skip_if.is_some()));
    let v = value_of(action);
    if !needs_value {
        return Ok(());
    }
    if v.is_empty() {
        return Err(format!("{} has no value: write it in the Cue sheet", action.kind));
    }
    let pattern = a.and_then(|a| a.value.as_ref()).unwrap_or(&p.value);
    if !pattern.is_match(&v) {
        return Err(format!("\"{v}\" is not a value {} accepts for {}: use {}", p.name, action.kind, if p.hint.is_empty() { "letters, digits and _ . -" } else { &p.hint }));
    }
    if p.needs_programs {
        let allowed = allowed_programs(dev);
        if allowed.is_empty() {
            return Err(format!("{} has no list of allowed programs, so it will not run anything: write the names its programmer validated in the rig node's \"programs\"", dev.label));
        }
        if !allowed.iter().any(|x| x == &v) {
            return Err(format!("\"{v}\" is not on {}'s list of allowed programs ({}): add it in the rig node's \"programs\" only if its programmer validated it", dev.label, allowed.join(", ")));
        }
    }
    if a.is_some_and(uses_number) && program_number(dev, &v).is_none() {
        return Err(format!("\"{v}\" has no program number on {}'s list: this device picks programs by number, so write it as {v}=<number> in the rig node's \"programs\" (the number its PLC program expects)", dev.label));
    }
    Ok(())
}

/// The commands a cue would send, in order, for a person to read (a step that is skipped when something is already true says so).
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    check_action(dev, action)?;
    let p = get(dev.profile_key()).ok_or("no link profile")?;
    let a = p.actions.get(&action.kind).ok_or("no mapping")?;
    let v = value_of(action);
    let sub = Subst { name: &v, num: program_number(dev, &v), caps: caps_of(a.value.as_ref().unwrap_or(&p.value), &v) };
    let mut out = Vec::new();
    for s in &a.steps {
        let line = sub.render(&s.send);
        let note = if s.skip_if.is_some() { " (skipped if it is already loaded)" } else { "" };
        let kind = if p.modbus.is_some() { "Modbus TCP" } else { "text" };
        out.push((format!("{} {kind}: {}{}", p.name, line, note), vec![]));
    }
    if let Some(c) = &a.confirm {
        out.push((format!("{} text: {} (read back, to see it running)", p.name, sub.render(&c.send)), vec![]));
    }
    Ok(out)
}

// ---- the result of asking a device to do something ----

/// What came of one request: the device said no, or could not be asked.
#[derive(Debug, PartialEq)]
pub enum LinkError {
    /// The device answered, and the answer was a no.
    Refused(String),
    /// No usable answer: not reachable, the connection broke, a check of ours failed.
    Failed(String),
}

impl LinkError {
    pub fn status(&self) -> &'static str {
        match self {
            LinkError::Refused(_) => "refused",
            LinkError::Failed(_) => "failed",
        }
    }
    pub fn text(&self) -> &str {
        match self {
            LinkError::Refused(t) | LinkError::Failed(t) => t,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.text())
    }
}

#[derive(Debug)]
pub struct Outcome {
    /// `accepted` (it answered yes) or `running` (a read-back shows it is doing it).
    pub status: &'static str,
    pub note: String,
    pub target: String,
}

/// Send one request: the command, then the profile's terminator (a line end, or nothing for a device that frames its own messages).
async fn write_request(r: &mut BufReader<TcpStream>, cmd: &str, end: &str) -> Result<(), String> {
    r.get_mut().write_all(format!("{cmd}{end}").as_bytes()).await.map_err(|e| format!("could not send: {e}"))
}

/// Read one reply: a line, or (with `until`) everything up to and including that text. Bounded, so a device that never stops talking
/// cannot fill memory. An empty connection close is an error.
async fn read_reply(r: &mut BufReader<TcpStream>, until: Option<&str>) -> Result<String, String> {
    match until {
        None => {
            let mut line = String::new();
            match r.read_line(&mut line).await {
                Ok(0) => Err("the device closed the connection".into()),
                Ok(_) => Ok(line.trim().to_string()),
                Err(e) => Err(format!("could not read: {e}")),
            }
        }
        Some(u) => {
            let last = *u.as_bytes().last().ok_or("empty reply_until")?;
            let mut buf: Vec<u8> = Vec::new();
            loop {
                match r.read_until(last, &mut buf).await {
                    Ok(0) => return Err("the device closed the connection".into()),
                    Ok(_) => {}
                    Err(e) => return Err(format!("could not read: {e}")),
                }
                if buf.ends_with(u.as_bytes()) {
                    return Ok(String::from_utf8_lossy(&buf).trim().to_string());
                }
                if buf.len() > 64 * 1024 {
                    return Err("the reply was longer than 64 KB without ending: not a reply this profile expects".into());
                }
            }
        }
    }
}

// ---- a read-only look at a real device, to compare what it says with what the profile assumes ----

/// One status read, as the device answered it.
#[derive(Debug, Clone)]
pub struct ProbeRead {
    pub var: String,
    pub send: String,
    /// What the device said, exactly.
    pub reply: Option<String>,
    /// Whether the reply matches what the profile expects.
    pub matched: bool,
    /// The value the profile would take from it.
    pub value: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub addr: String,
    /// A line the device sent on connect, before any command (some do, some do not).
    pub greeting: Option<String>,
    pub reads: Vec<ProbeRead>,
    /// What the profile's `ready_when` rules make of those replies.
    pub ready: Option<bool>,
    pub note: String,
}

impl Probe {
    /// Every read answered, in the form the profile expects.
    pub fn all_matched(&self) -> bool {
        !self.reads.is_empty() && self.reads.iter().all(|r| r.matched)
    }
}

/// Connect to `addr` and send ONLY the profile's status reads: nothing that could start, stop or change anything. Says what the device
/// actually replied, so a profile written from a manual can be checked against the real thing (or a simulator). Never sends an action.
pub async fn probe(profile: &LinkProfile, addr: &str) -> Result<Probe, String> {
    let s = tokio::time::timeout(CONNECT, TcpStream::connect(addr)).await.map_err(|_| format!("{addr} did not answer within {} s", CONNECT.as_secs()))?.map_err(|e| format!("cannot reach {addr}: {e}"))?;
    let _ = s.set_nodelay(true);
    let mut r = BufReader::new(s);
    // wait a little longer than a normal connect for a greeting, so "none" really means none
    // a Modbus device does not greet: do not make a probe wait to find that out
    let greeting = if profile.modbus.is_some() {
        None
    } else {
        let mut line: Vec<u8> = Vec::new();
        match tokio::time::timeout(Duration::from_millis(1500), r.read_until(profile.line_byte(), &mut line)).await {
            Ok(Ok(n)) if n > 0 => Some(String::from_utf8_lossy(&line).trim().to_string()),
            _ => None,
        }
    };
    if let Some(why) = greeting.as_deref().and_then(|g| profile.refuses_greeting(g)) {
        return Err(why);
    }
    let mut txid: u16 = 0;
    let mut reads = Vec::new();
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for rd in &profile.reads {
        let mut item = ProbeRead { var: rd.var.clone().unwrap_or_else(|| rd.extract.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>().join(",")), send: rd.send.clone(), reply: None, matched: false, value: None, error: None };
        let go = async {
            if let Some(m) = &profile.modbus {
                txid = txid.wrapping_add(1);
                return modbus_text(&mut r, m, txid, &rd.send).await;
            }
            write_request(&mut r, &rd.send, &profile.request_end).await?;
            read_reply(&mut r, profile.reply_until.as_deref()).await
        };
        match tokio::time::timeout(REPLY, go).await {
            Ok(Ok(reply)) => {
                if rd.matches(&reply) {
                    item.matched = true;
                    let got = rd.values(&reply);
                    item.value = Some(if got.len() == 1 { got.values().next().cloned().unwrap_or_default() } else { got.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ") });
                    vars.extend(got);
                }
                item.reply = Some(reply);
                reads.push(item);
            }
            Ok(Err(e)) => {
                item.error = Some(e);
                reads.push(item);
                break; // a connection that failed once is not asked again
            }
            Err(_) => {
                item.error = Some(format!("no answer within {} s", REPLY.as_secs()));
                reads.push(item);
                break;
            }
        }
    }
    let (mut ready, mut note) = (None, String::new());
    if !profile.ready_when.is_empty() && reads.iter().all(|r| r.matched) && reads.len() == profile.reads.len() {
        let mut ok = true;
        for rule in &profile.ready_when {
            let v = vars.get(&rule.var).cloned().unwrap_or_default();
            if !(rule.equals.as_ref().map_or(true, |e| *e == v) && (rule.one_of.is_empty() || rule.one_of.contains(&v))) {
                ok = false;
                note = rule.why.replace("{value}", &v);
                break;
            }
        }
        ready = Some(ok);
    }
    Ok(Probe { addr: addr.to_string(), greeting, reads, ready, note })
}

// ---- the live link to one device ----

/// What is known about a device from reading it: whether it answers, its values, whether it is ready, and why not.
#[derive(Clone, Debug, Default)]
pub struct LinkState {
    /// `none` (never tried), `silent` (not answered yet), `linked`, `lost` (answered before, not now).
    pub state: &'static str,
    pub vars: BTreeMap<String, String>,
    pub ready: Option<bool>,
    pub note: String,
}

struct Conn {
    stream: Option<BufReader<TcpStream>>,
}

struct Link {
    profile: Arc<LinkProfile>,
    addr: String,
    key: String,
    conn: tokio::sync::Mutex<Conn>,
    state: Mutex<LinkState>,
    was_linked: std::sync::atomic::AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    txid: std::sync::atomic::AtomicU16,
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
    }
}

#[derive(Default)]
pub struct TextLinks {
    links: tokio::sync::Mutex<HashMap<String, Arc<Link>>>,
}

/// One Modbus command on an open connection, for a profile: parsed, checked against the addresses the profile forbids (again, at the
/// moment of sending), sent, and answered as text.
async fn modbus_text(r: &mut BufReader<TcpStream>, m: &ModbusCfg, txid: u16, cmd: &str) -> Result<String, String> {
    let c = crate::modbus::parse(cmd)?;
    if let Some(f) = m.never.iter().find(|f| f.covers(&c)) {
        return Err(format!("refused before sending: '{cmd}' writes an address this profile forbids ({:?} {} to {})", f.space, f.lo, f.hi));
    }
    crate::modbus::do_command(r, m.unit, txid, &c).await
}

impl Link {
    async fn connect(&self, conn: &mut Conn) -> Result<(), String> {
        if conn.stream.is_some() {
            return Ok(());
        }
        let s = tokio::time::timeout(CONNECT, TcpStream::connect(&self.addr)).await.map_err(|_| format!("{} did not answer within {} s", self.addr, CONNECT.as_secs()))?.map_err(|e| format!("cannot reach {}: {e}", self.addr))?;
        let _ = s.set_nodelay(true);
        let mut r = BufReader::new(s);
        if self.profile.modbus.is_none() {
            // some text devices greet on connect: take that line if it comes, so it is not read as the answer to the first command
            let mut greeting: Vec<u8> = Vec::new();
            let _ = tokio::time::timeout(GREETING, r.read_until(self.profile.line_byte(), &mut greeting)).await;
            // a greeting this profile cannot work with (a password challenge): say so, and send nothing
            if let Some(why) = self.profile.refuses_greeting(&String::from_utf8_lossy(&greeting)) {
                return Err(why);
            }
        }
        conn.stream = Some(r);
        Ok(())
    }

    /// One command, one reply line. On any trouble the connection is dropped, so a late reply can never be read as the next one's.
    async fn exchange(&self, conn: &mut Conn, cmd: &str) -> Result<String, String> {
        self.connect(conn).await?;
        let r = conn.stream.as_mut().ok_or("not connected")?;
        let go = async {
            if let Some(m) = &self.profile.modbus {
                let tx = self.txid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return modbus_text(r, m, tx, cmd).await.map_err(|e| format!("{e} ({})", self.addr));
            }
            write_request(r, cmd, &self.profile.request_end).await.map_err(|e| format!("{e} ({})", self.addr))?;
            read_reply(r, self.profile.reply_until.as_deref()).await.map_err(|e| format!("{e} ({})", self.addr))
        };
        match tokio::time::timeout(REPLY, go).await {
            Ok(Ok(l)) => Ok(l),
            Ok(Err(e)) => {
                conn.stream = None;
                Err(e)
            }
            Err(_) => {
                conn.stream = None;
                Err(format!("{} did not answer \"{cmd}\" within {} s", self.addr, REPLY.as_secs()))
            }
        }
    }

    /// Run a list of steps. `vars` carries what earlier steps stored.
    /// Whether a step would change something (so a failure after it needs the cleanup): a Modbus write, or a step that says so.
    fn changes(&self, s: &Step, sub: &Subst<'_>) -> bool {
        s.changes || (self.profile.modbus.is_some() && crate::modbus::parse(&sub.render(&s.send)).is_ok_and(|c| c.is_write()))
    }

    /// Run a list of steps. `changed` is set as soon as a step that changes something is ATTEMPTED (even one whose answer is lost: it may
    /// have landed), so the caller knows whether a cleanup is needed.
    async fn run_steps(&self, conn: &mut Conn, steps: &[Step], sub: &Subst<'_>, vars: &mut HashMap<String, String>, changed: &mut bool) -> Result<(), LinkError> {
        for s in steps {
            if let Some((var, pat)) = &s.skip_if {
                let re = full(&pat.replace("{0}", &regex::escape(sub.name))).map_err(LinkError::Failed)?;
                if vars.get(var).is_some_and(|x| re.is_match(x)) {
                    continue;
                }
            }
            if self.changes(s, sub) {
                *changed = true;
            }
            let reply = self.exchange(conn, &sub.render(&s.send)).await.map_err(LinkError::Failed)?;
            if !s.wait.is_zero() {
                tokio::time::sleep(s.wait).await;
            }
            if let Some(re) = &s.expect {
                match re.captures(&reply) {
                    None => return Err(LinkError::Refused(sub.render(&s.fail).replace("{reply}", &reply))),
                    Some(c) => {
                        if let Some(var) = &s.store {
                            vars.insert(var.clone(), c.get(1).map_or(reply.clone(), |m| m.as_str().to_string()));
                        }
                    }
                }
            } else if let Some(var) = &s.store {
                vars.insert(var.clone(), reply);
            }
        }
        Ok(())
    }

    async fn confirm(&self, conn: &mut Conn, c: &Step, sub: &Subst<'_>) -> Result<bool, LinkError> {
        let until = tokio::time::Instant::now() + c.within;
        loop {
            let reply = self.exchange(conn, &sub.render(&c.send)).await.map_err(LinkError::Failed)?;
            if c.expect.as_ref().is_some_and(|re| re.is_match(&reply)) {
                return Ok(true);
            }
            if tokio::time::Instant::now() + c.every > until {
                return Ok(false);
            }
            tokio::time::sleep(c.every).await;
        }
    }

    async fn lock(&self) -> Result<tokio::sync::MutexGuard<'_, Conn>, LinkError> {
        tokio::time::timeout(BUSY, self.conn.lock()).await.map_err(|_| LinkError::Failed(format!("{} is busy with another request", self.addr)))
    }

    async fn apply(&self, dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
        check_action(dev, action).map_err(LinkError::Failed)?;
        let a = self.profile.actions.get(&action.kind).ok_or_else(|| LinkError::Failed("no mapping".into()))?;
        let value = value_of(action);
        let sub = Subst { name: &value, num: program_number(dev, &value), caps: caps_of(a.value.as_ref().unwrap_or(&self.profile.value), &value) };
        let mut conn = self.lock().await?;
        let mut vars = HashMap::new();
        let mut changed = false;
        let result = self.run_steps(&mut conn, &a.steps, &sub, &mut vars, &mut changed).await;
        if let Err(e) = &result {
            self.note_failure(e);
            // put back what the steps changed (a start pulse that was raised must be lowered), whatever went wrong: best effort, and the
            // original error is what is reported. Only if a step that changes something was actually attempted: a PLC that said no at the
            // first check is not written to at all.
            if changed && !a.cleanup.is_empty() {
                let (mut ignore, mut also) = (HashMap::new(), false);
                let _ = self.run_steps(&mut conn, &a.cleanup, &sub, &mut ignore, &mut also).await;
            }
        }
        result?;
        let target = self.addr.clone();
        match &a.confirm {
            None => Ok(Outcome { status: "accepted", note: "the device said yes".into(), target }),
            Some(c) => match self.confirm(&mut conn, c, &sub).await {
                Ok(true) => Ok(Outcome { status: "running", note: "read back: it is doing it".into(), target }),
                Ok(false) => Ok(Outcome { status: "accepted", note: format!("the device said yes, but a read-back did not show it within {} s (a short program may already have finished)", c.within.as_secs_f32()), target }),
                Err(e) => Ok(Outcome { status: "accepted", note: format!("the device said yes, but the read-back failed: {e}"), target }),
            },
        }
    }

    async fn stop(&self) -> Result<Outcome, LinkError> {
        if self.profile.stop.is_empty() {
            return Err(LinkError::Failed("this profile has no stop steps".into()));
        }
        let mut conn = self.lock().await?;
        // a stop is the one request that must try twice: a dropped connection is retried once on a fresh one
        let mut vars = HashMap::new();
        let none = Subst { name: "", num: None, caps: Vec::new() };
        let mut changed = false;
        let mut r = self.run_steps(&mut conn, &self.profile.stop, &none, &mut vars, &mut changed).await;
        if matches!(r, Err(LinkError::Failed(_))) {
            conn.stream = None;
            r = self.run_steps(&mut conn, &self.profile.stop, &none, &mut vars, &mut changed).await;
        }
        r.map(|_| Outcome { status: "accepted", note: "the device acknowledged the stop request".into(), target: self.addr.clone() })
    }

    fn note_failure(&self, e: &LinkError) {
        if let LinkError::Failed(t) = e {
            let mut s = self.state.lock().unwrap();
            s.state = if self.was_linked.load(std::sync::atomic::Ordering::Relaxed) { "lost" } else { "silent" };
            s.ready = Some(false);
            s.note = t.clone();
        }
    }

    /// Read the device's state once. Skipped if a request is running (it will be read next time).
    async fn poll_once(&self) {
        let Ok(mut conn) = self.conn.try_lock() else { return };
        let mut vars = BTreeMap::new();
        for r in &self.profile.reads {
            match self.exchange(&mut conn, &r.send).await {
                Ok(reply) => vars.extend(r.values(&reply)),
                Err(e) => {
                    let mut s = self.state.lock().unwrap();
                    s.state = if self.was_linked.load(std::sync::atomic::Ordering::Relaxed) { "lost" } else { "silent" };
                    s.ready = Some(false);
                    s.note = e;
                    return;
                }
            }
        }
        self.was_linked.store(true, std::sync::atomic::Ordering::Relaxed);
        let (mut ready, mut note) = (true, String::new());
        for rule in &self.profile.ready_when {
            let v = vars.get(&rule.var).cloned().unwrap_or_default();
            let ok = rule.equals.as_ref().map_or(true, |e| *e == v) && (rule.one_of.is_empty() || rule.one_of.contains(&v));
            if !ok {
                ready = false;
                note = rule.why.replace("{value}", &v);
                break;
            }
        }
        let mut s = self.state.lock().unwrap();
        s.state = "linked";
        s.vars = vars;
        s.ready = if self.profile.ready_when.is_empty() { None } else { Some(ready) };
        s.note = note;
    }
}

impl TextLinks {
    async fn link(&self, dev: &Device) -> Result<Arc<Link>, String> {
        let profile = get(dev.profile_key()).ok_or_else(|| format!("{} has no link profile", dev.label))?;
        let host = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).ok_or_else(|| format!("{} has no host in the rig", dev.label))?;
        let port = dev.port.or(profile.port).ok_or_else(|| format!("{} has no port in the rig", dev.label))?;
        let addr = format!("{host}:{port}");
        let key = format!("{addr}|{}", profile.id);
        let mut links = self.links.lock().await;
        if let Some(l) = links.get(&dev.id).filter(|l| l.key == key) {
            return Ok(Arc::clone(l));
        }
        let link = Arc::new(Link { profile: Arc::clone(&profile), addr, key, conn: tokio::sync::Mutex::new(Conn { stream: None }), state: Mutex::new(LinkState { state: "silent", ..Default::default() }), was_linked: Default::default(), task: Mutex::new(None), txid: Default::default() });
        links.insert(dev.id.clone(), Arc::clone(&link));
        Ok(link)
    }

    /// Make sure the device is being watched: its state read every second or so (called while the runtime is sending). A request alone
    /// does not start the watching, so a test of one request sees only that request's commands.
    pub async fn ensure(&self, dev: &Device) -> Result<(), String> {
        let link = self.link(dev).await?;
        let mut task = link.task.lock().unwrap();
        if task.is_none() && !link.profile.reads.is_empty() {
            let weak: Weak<Link> = Arc::downgrade(&link);
            let every = link.profile.poll;
            *task = Some(tokio::spawn(async move {
                loop {
                    let Some(l) = weak.upgrade() else { break };
                    l.poll_once().await;
                    drop(l);
                    tokio::time::sleep(every).await;
                }
            }));
        }
        Ok(())
    }

    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
        let l = self.link(dev).await.map_err(LinkError::Failed)?;
        l.apply(dev, action).await
    }

    /// The profile's stop steps.
    pub async fn stop(&self, dev: &Device) -> Result<Outcome, LinkError> {
        let l = self.link(dev).await.map_err(LinkError::Failed)?;
        l.stop().await
    }

    /// Read the device's state now and say what it is (for a test of the link: it only reads).
    pub async fn check(&self, dev: &Device) -> Result<Value, LinkError> {
        let l = self.link(dev).await.map_err(LinkError::Failed)?;
        {
            let mut conn = l.lock().await?;
            // a fresh read, even though the watcher also reads: a test says what the device says now
            for r in &l.profile.reads {
                let reply = l.exchange(&mut conn, &r.send).await.map_err(LinkError::Failed)?;
                if !r.matches(&reply) {
                    return Err(LinkError::Refused(format!("the device answered \"{}\" with \"{reply}\", which is not what {} expects", r.send, l.profile.name)));
                }
            }
        }
        l.poll_once().await;
        Ok(self.state_json(&dev.id))
    }

    /// What is known about a device, or None if it has never been looked at.
    pub fn state(&self, id: &str) -> Option<LinkState> {
        let links = self.links.try_lock().ok()?;
        links.get(id).map(|l| l.state.lock().unwrap().clone())
    }

    pub fn state_json(&self, id: &str) -> Value {
        match self.state(id) {
            Some(s) => json!({"state": s.state, "ready": s.ready, "note": s.note, "values": s.vars}),
            None => json!({"state": "none"}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ursim::UrSim;

    fn dev(port: u16, programs: &str) -> Device {
        Device { id: "ur1".into(), label: "UR".into(), kind: "ur".into(), dept: "motion".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: None, programs: Some(programs.into()) }
    }

    fn run(p: &str) -> Action {
        Action::new("motion.run", json!({"program": p}))
    }

    /// The commands PCHI must never send, whatever a cue says.
    const NEVER: [&str; 9] = ["power on", "power off", "brake release", "unlock protective stop", "close safety popup", "restart safety", "load installation", "shutdown", "set operational mode"];

    #[test]
    fn the_built_in_ur_profile_is_valid_and_its_whole_command_set_is_small_and_known() {
        let p = get("ur").expect("ur profile");
        assert_eq!((p.dept.as_str(), p.port, p.built_in), ("motion", Some(29999), true));
        let cmds = p.commands();
        for c in &cmds {
            assert!(!NEVER.iter().any(|n| c.to_ascii_lowercase().starts_with(n)), "the UR profile can send '{c}'");
        }
        for expected in ["is in remote control", "safetystatus", "get loaded program", "load {0}.urp", "play", "pause", "stop", "programState", "robotmode"] {
            assert!(cmds.iter().any(|c| c == expected), "missing {expected}: {cmds:?}");
        }
        assert_eq!(cmds.len(), 9, "the profile sends exactly these and nothing else: {cmds:?}");
    }

    #[test]
    fn a_profile_that_could_send_a_forbidden_command_or_has_a_loose_template_is_refused() {
        let mk = |send: &str| format!(r#"{{"id":"t","name":"T","transport":"tcp-text","dept":"motion","kinds":["motion.run"],"value":"[A-Z]+","never":["power on","brake release"],"actions":{{"motion.run":{{"steps":[{{"send":"{send}"}}]}}}}}}"#);
        assert!(LinkProfile::from_json(&mk("play {0}"), false).is_ok());
        let e = LinkProfile::from_json(&mk("power on"), false).err().unwrap();
        assert!(e.contains("must never send"), "{e}");
        assert!(LinkProfile::from_json(&mk("Brake release now"), false).err().unwrap().contains("must never send"), "case and trailing words do not get around it");
        assert!(LinkProfile::from_json(&mk("play {1}"), false).err().unwrap().contains("bracketed part"), "a part the value pattern does not have");
        assert!(LinkProfile::from_json(&mk("play {x}"), false).err().unwrap().contains("only placeholder"));
        assert!(LinkProfile::from_json(&mk("play\\nstop"), false).err().unwrap().contains("control character"), "one command is one line");
        assert!(LinkProfile::from_json(&mk("play").replace("tcp-text", "udp"), false).is_err());
    }

    #[tokio::test]
    async fn a_cue_not_on_the_allow_list_sends_nothing_at_all() {
        let sim = UrSim::start(&["OPEN_ARM", "PARK"]).await.unwrap();
        let links = TextLinks::default();
        for (prog, list, why) in [("OPEN_ARM", "", "no list of allowed programs"), ("PARK", "OPEN_ARM", "not on UR's list"), ("OPEN ARM", "OPEN_ARM", "not a value"), ("a;power on", "a;power on", "not a value"), ("", "OPEN_ARM", "no value")] {
            let e = links.apply(&dev(sim.port, list), &run(prog)).await.err().unwrap_or_else(|| panic!("{prog} should be refused"));
            assert!(e.text().contains(why), "{prog}: {}", e.text());
            assert_eq!(e.status(), "failed");
        }
        assert!(sim.seen().is_empty(), "not one byte reached the robot: {:?}", sim.seen());
    }

    #[tokio::test]
    async fn a_run_sends_the_documented_commands_in_order_and_reads_back_that_it_is_running() {
        let sim = UrSim::start(&["OPEN_ARM", "PARK"]).await.unwrap();
        let links = TextLinks::default();
        let d = dev(sim.port, "OPEN_ARM, PARK");
        let o = links.apply(&d, &run("OPEN_ARM")).await.unwrap();
        assert_eq!(o.status, "running", "{}", o.note);
        assert_eq!(sim.seen(), ["is in remote control", "safetystatus", "get loaded program", "load OPEN_ARM.urp", "play", "programState"]);
        // the same program again: it is already loaded, so it is not loaded again
        sim.with(|s| s.program = "STOPPED");
        sim.with(|s| s.seen.clear());
        assert_eq!(links.apply(&d, &run("OPEN_ARM")).await.unwrap().status, "running");
        assert_eq!(sim.seen(), ["is in remote control", "safetystatus", "get loaded program", "play", "programState"]);
        // a different one is loaded
        sim.with(|s| s.seen.clear());
        links.apply(&d, &run("PARK")).await.unwrap();
        assert!(sim.seen().contains(&"load PARK.urp".to_string()));
        for c in sim.seen() {
            assert!(!NEVER.iter().any(|n| c.to_ascii_lowercase().starts_with(n)), "PCHI sent '{c}'");
        }
    }

    #[tokio::test]
    async fn when_the_robot_says_no_the_result_is_refused_in_its_words_and_nothing_else_is_tried() {
        let sim = UrSim::start(&["A"]).await.unwrap();
        let links = TextLinks::default();
        let d = dev(sim.port, "A");
        sim.fault("local-control").unwrap();
        let e = links.apply(&d, &run("A")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("Remote Control"), "{}", e.text());
        assert_eq!(sim.seen(), ["is in remote control"], "it stopped at the first no");
        sim.fault("healthy").unwrap();
        sim.fault("protective-stop").unwrap();
        sim.with(|s| s.seen.clear());
        let e = links.apply(&d, &run("A")).await.err().unwrap();
        assert!(e.text().contains("PROTECTIVE_STOP"), "{}", e.text());
        assert!(!sim.seen().iter().any(|c| c == "play"), "it never asked a stopped robot to play");
        sim.fault("healthy").unwrap();
        sim.fault("refuse-play").unwrap();
        let e = links.apply(&d, &run("A")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("Failed to execute: play"), "{}", e.text());
    }

    #[tokio::test]
    async fn an_unreachable_robot_is_failed_not_refused() {
        // nothing is listening on this port
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        let links = TextLinks::default();
        let e = links.apply(&dev(port, "A"), &run("A")).await.err().unwrap();
        assert_eq!(e.status(), "failed");
        assert!(e.text().contains("cannot reach"), "{}", e.text());
    }

    #[tokio::test]
    async fn a_late_reply_is_never_read_as_the_answer_to_the_next_question() {
        // a controller that is slow once: its first connection answers the first question 3.6 s late (after the 3 s the link waits), with
        // a line that would fail every check. The next connection answers properly.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut n = 0;
            loop {
                let Ok((sock, _)) = l.accept().await else { continue };
                n += 1;
                let slow = n == 1;
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let mut lines = tokio::io::BufReader::new(rd).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if slow {
                            tokio::time::sleep(Duration::from_millis(3_600)).await;
                            let _ = wr.write_all(b"STALE ANSWER
").await;
                            continue;
                        }
                        let r = match line.as_str() {
                            "is in remote control" => "true",
                            "safetystatus" => "Safetystatus: NORMAL",
                            "robotmode" => "Robotmode: RUNNING",
                            _ => "STOPPED",
                        };
                        let _ = wr.write_all(format!("{r}
").as_bytes()).await;
                    }
                });
            }
        });
        let links = TextLinks::default();
        let d = dev(port, "A");
        let e = links.check(&d).await.err().unwrap();
        assert_eq!(e.status(), "failed");
        assert!(e.text().contains("did not answer"), "{}", e.text());
        tokio::time::sleep(Duration::from_millis(900)).await; // the stale line is now on the old connection, which was dropped
        let v = links.check(&d).await.expect("a fresh connection reads cleanly");
        assert_eq!(v["values"]["remote"], "true");
        assert_eq!(v["ready"], true);
    }

    #[tokio::test]
    async fn the_state_is_read_and_a_fault_makes_the_device_not_ready_with_the_robots_own_reason() {
        let sim = UrSim::start(&["A"]).await.unwrap();
        let links = TextLinks::default();
        let d = dev(sim.port, "A");
        links.ensure(&d).await.unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(4);
        let wait = |want: &'static str| {
            let links = &links;
            async move {
                while std::time::Instant::now() < until {
                    if links.state("ur1").is_some_and(|s| s.state == want) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                panic!("never became {want}: {:?}", links.state("ur1"));
            }
        };
        wait("linked").await;
        let s = links.state("ur1").unwrap();
        assert_eq!((s.ready, s.vars.get("mode").map(String::as_str), s.vars.get("safety").map(String::as_str)), (Some(true), Some("RUNNING"), Some("NORMAL")));
        sim.fault("protective-stop").unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            if links.state("ur1").is_some_and(|s| s.ready == Some(false)) {
                break;
            }
            assert!(std::time::Instant::now() < end, "never noticed the protective stop");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(links.state("ur1").unwrap().note.contains("PROTECTIVE_STOP"), "{:?}", links.state("ur1"));
        sim.fault("healthy").unwrap();
        sim.fault("local-control").unwrap();
        tokio::time::sleep(Duration::from_millis(1400)).await;
        assert!(links.state("ur1").unwrap().note.contains("Remote Control"), "{:?}", links.state("ur1"));
        sim.fault("healthy").unwrap();
        sim.fault("down").unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            if links.state("ur1").is_some_and(|s| s.state == "lost") {
                break;
            }
            assert!(std::time::Instant::now() < end, "never noticed the robot went away: {:?}", links.state("ur1"));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(links.state("ur1").unwrap().ready, Some(false));
    }

    #[tokio::test]
    async fn stop_sends_only_stop_and_a_robot_with_nothing_running_is_not_an_error() {
        let sim = UrSim::start(&["A"]).await.unwrap();
        let links = TextLinks::default();
        let d = dev(sim.port, "A");
        let o = links.stop(&d).await.unwrap();
        assert_eq!(o.status, "accepted");
        assert_eq!(sim.seen(), ["stop"], "a stop is one command");
        links.apply(&d, &run("A")).await.unwrap();
        assert!(links.stop(&d).await.is_ok());
        assert_eq!(sim.snapshot().program, "STOPPED");
    }

    #[test]
    fn a_link_profile_in_a_folder_is_loaded_beside_osc_profiles_and_is_checked_like_the_built_in() {
        let dir = std::env::temp_dir().join(format!("pchi-linkprof-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-robot.json"), r#"{"id":"acme-arm","name":"Acme Arm","transport":"tcp-text","dept":"motion","port":7000,"kinds":["motion.run"],"value":"[A-Z]+","needs_programs":true,"never":["power on"],"actions":{"motion.run":{"steps":[{"send":"RUN {0}","expect":"OK"}]}}}"#).unwrap();
        std::fs::write(dir.join("osc.json"), r#"{"id":"acme-desk","name":"Acme Desk","dept":"lighting","kinds":["lighting.look"],"rules":[{"match":"[0-9]+","send":"/go/{0}"}]}"#).unwrap();
        let lines = crate::profiles::load_dir(&dir).unwrap();
        assert!(lines.iter().any(|l| l.contains("acme-arm") && l.contains("text link")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("acme-desk")), "{lines:?}");
        assert!(get("acme-arm").is_some_and(|p| p.handles("motion.run") && p.port == Some(7000)));
        assert_eq!(crate::profiles::dept_of("acme-arm").as_deref(), Some("motion"), "a device naming this profile is a motion device");
        // a custom profile that could send a forbidden command is refused when the folder is loaded
        std::fs::write(dir.join("a-robot.json"), r#"{"id":"acme-arm","name":"Acme Arm","transport":"tcp-text","dept":"motion","kinds":["motion.run"],"value":"[A-Z]+","never":["power on"],"actions":{"motion.run":{"steps":[{"send":"power on"}]}}}"#).unwrap();
        let e = crate::profiles::load_dir(&dir).unwrap_err();
        assert!(e.contains("must never send"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_probe_sends_only_reads_and_reports_what_the_device_really_said() {
        let sim = UrSim::start(&["A"]).await.unwrap();
        let p = get("ur").unwrap();
        let addr = format!("127.0.0.1:{}", sim.port);
        let r = probe(&p, &addr).await.unwrap();
        assert_eq!(r.greeting.as_deref(), Some("Connected: Universal Robots Dashboard Server"));
        assert!(r.all_matched(), "{r:?}");
        assert_eq!(r.ready, Some(true));
        let seen = sim.seen();
        assert_eq!(seen, ["is in remote control", "safetystatus", "robotmode", "programState"], "a probe sends only the profile's status reads");
        assert!(!seen.iter().any(|c| matches!(c.as_str(), "play" | "pause" | "stop") || c.starts_with("load")));
        // a faulted robot is read as not ready, with its reason
        sim.fault("protective-stop").unwrap();
        let r = probe(&p, &addr).await.unwrap();
        assert_eq!((r.ready, r.note.as_str()), (Some(false), "its safety status is PROTECTIVE_STOP"));
        // a device that answers in a form the profile does not expect is flagged, with its exact words
        let odd = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = odd.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = odd.accept().await else { continue };
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let mut lines = tokio::io::BufReader::new(rd).lines();
                    while let Ok(Some(_)) = lines.next_line().await {
                        let _ = wr.write_all(b"unexpected words\n").await;
                    }
                });
            }
        });
        let r = probe(&p, &format!("127.0.0.1:{port}")).await.unwrap();
        assert_eq!(r.greeting, None, "no greeting is reported as none");
        assert!(!r.all_matched() && r.reads[0].reply.as_deref() == Some("unexpected words") && r.ready.is_none(), "{r:?}");
        assert!(probe(&p, "127.0.0.1:1").await.is_err());
    }

    /// A pretend robot that frames its own messages: XML, no line ends, replies written in two pieces.
    async fn xml_robot(programs: &'static [&'static str]) -> (u16, Arc<Mutex<Vec<String>>>, Arc<Mutex<(String, String)>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let state: Arc<Mutex<(String, String)>> = Arc::new(Mutex::new(("OK".into(), "IDLE".into()))); // (safety, state)
        let (s2, st2) = (Arc::clone(&seen), Arc::clone(&state));
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = l.accept().await else { continue };
                let (seen, state) = (Arc::clone(&s2), Arc::clone(&st2));
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let mut rd = tokio::io::BufReader::new(rd);
                    loop {
                        let mut buf = Vec::new();
                        // a request ends at its closing </Cmd>
                        loop {
                            let mut b = Vec::new();
                            match rd.read_until(b'>', &mut b).await {
                                Ok(0) | Err(_) => return,
                                Ok(_) => buf.extend(b),
                            }
                            if buf.ends_with(b"</Cmd>") {
                                break;
                            }
                        }
                        let req = String::from_utf8_lossy(&buf).to_string();
                        seen.lock().unwrap().push(req.clone());
                        let (safety, st) = state.lock().unwrap().clone();
                        let reply = if req.contains("<Status/>") {
                            format!("<Reply><Remote>true</Remote><Safety>{safety}</Safety><State>{st}</State></Reply>")
                        } else if let Some(name) = req.split("<Run>").nth(1).and_then(|r| r.split("</Run>").next()) {
                            if programs.contains(&name) && safety == "OK" {
                                state.lock().unwrap().1 = "RUNNING".into();
                                "<Reply><Ack>OK</Ack></Reply>".to_string()
                            } else {
                                format!("<Reply><Nack>cannot run {name}</Nack></Reply>")
                            }
                        } else if req.contains("<Stop/>") {
                            state.lock().unwrap().1 = "IDLE".into();
                            "<Reply><Ack>OK</Ack></Reply>".to_string()
                        } else {
                            "<Reply><Nack>unknown</Nack></Reply>".to_string()
                        };
                        // two pieces, with no newline anywhere: a reader that waits for a line would hang
                        let (a, b) = reply.split_at(reply.len() / 2);
                        let _ = wr.write_all(a.as_bytes()).await;
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        let _ = wr.write_all(b.as_bytes()).await;
                    }
                });
            }
        });
        (port, seen, state)
    }

    const XML_PROFILE: &str = r#"{"id":"xml-arm","name":"XML Arm","transport":"tcp-text","dept":"motion","kinds":["motion.run","motion.stop"],
        "value":"[A-Z_]{1,32}","needs_programs":true,"request_end":"","reply_until":"</Reply>",
        "never":["power","brake","reset","mode"],
        "actions":{
          "motion.run":{"steps":[{"send":"<Cmd><Run>{0}</Run></Cmd>","expect":"<Reply><Ack>OK</Ack></Reply>","fail":"the arm said {reply}"}],
                        "confirm":{"send":"<Cmd><Status/></Cmd>","expect":".*<State>RUNNING</State>.*","within_ms":1500,"every_ms":100}},
          "motion.stop":{"steps":[{"send":"<Cmd><Stop/></Cmd>","expect":"<Reply><Ack>OK</Ack></Reply>"}]}},
        "status":{"poll_ms":500,"reads":[{"send":"<Cmd><Status/></Cmd>","expect":"<Reply>.*</Reply>","extract":{"safety":"<Safety>(\\w+)</Safety>","state":"<State>(\\w+)</State>","remote":"<Remote>(\\w+)</Remote>"}}],
                  "ready_when":[{"var":"remote","equals":"true","why":"not remote"},{"var":"safety","equals":"OK","why":"safety is {value}"}]},
        "stop":[{"send":"<Cmd><Stop/></Cmd>","expect":"<Reply><Ack>OK</Ack></Reply>"}]}"#;

    #[tokio::test]
    async fn a_device_that_frames_its_own_xml_messages_is_driven_with_no_line_ends_at_all() {
        let p = Arc::new(LinkProfile::from_json(XML_PROFILE, false).unwrap());
        registry().write().unwrap().insert("xml-arm".into(), Arc::clone(&p));
        let (port, seen, state) = xml_robot(&["OPEN_ARM"]).await;
        let mut d = dev(port, "OPEN_ARM");
        d.kind = "xml-arm".into();
        let links = TextLinks::default();
        // a run: a request is sent with no newline, the reply (in two pieces, no newline) is read to its end, and it is read back
        let o = links.apply(&d, &run("OPEN_ARM")).await.unwrap();
        assert_eq!(o.status, "running", "{}", o.note);
        assert_eq!(seen.lock().unwrap()[0], "<Cmd><Run>OPEN_ARM</Run></Cmd>", "no line end was added");
        // a no is a no, in the device's words
        let e = links.apply(&d, &run("OPEN_ARM")).await;
        assert!(e.is_ok());
        state.lock().unwrap().0 = "FAULT".into();
        let e = links.apply(&d, &run("OPEN_ARM")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("<Nack>cannot run OPEN_ARM</Nack>"), "{}", e.text());
        // several values out of one reply, and the readiness rule reads them
        let r = probe(&p, &format!("127.0.0.1:{port}")).await.unwrap();
        assert!(r.all_matched(), "{r:?}");
        assert_eq!(r.ready, Some(false));
        assert_eq!(r.note, "safety is FAULT");
        assert!(r.reads[0].value.as_deref().unwrap().contains("state=IDLE") || r.reads[0].value.as_deref().unwrap().contains("state=RUNNING"), "{:?}", r.reads[0].value);
        state.lock().unwrap().0 = "OK".into();
        let o = links.stop(&d).await.unwrap();
        assert_eq!(o.status, "accepted");
        registry().write().unwrap().remove("xml-arm");
    }

    #[test]
    fn the_kuka_example_profile_loads_and_cannot_send_the_things_it_forbids() {
        let p = LinkProfile::from_json(include_str!("../../../tools/kuka/kuka-eki-example.json"), false).expect("the example profile is valid");
        assert_eq!((p.id.as_str(), p.dept.as_str(), p.basis.as_str(), p.port), ("kuka-eki-example", "motion", "memory", None));
        let cmds = p.commands();
        assert_eq!(cmds, ["<Cmd><Pause/></Cmd>", "<Cmd><Run>{0}</Run></Cmd>", "<Cmd><Status/></Cmd>", "<Cmd><Stop/></Cmd>"], "the example can send exactly four commands");
        for c in &cmds {
            let l = c.to_ascii_lowercase();
            assert!(!["power", "brake", "reset", "mode", "override"].iter().any(|n| l.contains(&format!("<{n}"))), "{c}");
        }
        assert!(p.notes.contains("NOT an emergency stop") && p.source.contains("NOT a KUKA format"));
    }

    #[test]
    fn the_never_list_also_catches_a_forbidden_command_written_as_an_xml_element() {
        let mk = |cmd: &str| XML_PROFILE.replace("<Cmd><Stop/></Cmd>\",\"expect\":\"<Reply><Ack>OK</Ack></Reply>\"}]}", &format!("{cmd}\",\"expect\":\"<Reply><Ack>OK</Ack></Reply>\"}}]}}"));
        assert!(LinkProfile::from_json(XML_PROFILE, false).is_ok());
        let _ = mk;
        for bad in ["<Cmd><Power>on</Power></Cmd>", "<Cmd><Brake release=\"1\"/></Cmd>", "<Cmd><Mode>T1</Mode></Cmd>", "<Cmd><RESET/></Cmd>"] {
            let text = XML_PROFILE.replace("\"stop\":[{\"send\":\"<Cmd><Stop/></Cmd>\"", &format!("\"stop\":[{{\"send\":{}", serde_json::to_string(bad).unwrap()));
            let e = LinkProfile::from_json(&text, false).err().unwrap_or_else(|| panic!("{bad} should be refused"));
            assert!(e.contains("must never send"), "{bad}: {e}");
        }
        // request_end and reply_until are checked
        assert!(LinkProfile::from_json(&XML_PROFILE.replace("\"request_end\":\"\"", "\"request_end\":\"x\""), false).err().unwrap().contains("request_end"));
        assert!(LinkProfile::from_json(&XML_PROFILE.replace("\"reply_until\":\"</Reply>\"", "\"reply_until\":\"\""), false).err().unwrap().contains("reply_until"));
    }

    // ---- a PLC over Modbus TCP, against a pretend PLC ----

    const PLC_EXAMPLE: &str = include_str!("../../../tools/kuka/kuka-plc-example.json");

    fn plc_dev(port: u16, programs: &str) -> Device {
        let mut d = dev(port, programs);
        d.kind = "kuka-plc-example".into();
        d
    }

    fn with_plc_profile() {
        let p = LinkProfile::from_json(PLC_EXAMPLE, false).expect("the example PLC profile is valid");
        registry().write().unwrap().insert("kuka-plc-example".into(), Arc::new(p));
    }

    #[test]
    fn the_plc_example_profile_loads_and_cannot_write_what_it_forbids() {
        let p = LinkProfile::from_json(PLC_EXAMPLE, false).expect("valid");
        assert_eq!((p.id.as_str(), p.dept.as_str(), p.basis.as_str(), p.port), ("kuka-plc-example", "motion", "memory", Some(502)));
        let cmds = p.commands();
        assert!(cmds.iter().all(|c| !c.starts_with("write_coil 3")), "reserved coils 30 to 39 are never written: {cmds:?}");
        assert!(cmds.contains(&"write_reg 100 {n}".to_string()) && cmds.contains(&"write_coil 10 1".to_string()));
        assert!(p.notes.contains("NOT an emergency stop") && p.source.contains("INVENTED"));
    }

    #[test]
    fn a_modbus_profile_that_could_write_a_forbidden_address_or_is_not_modbus_is_refused() {
        let mk = |step: &str| format!(r#"{{"id":"m","name":"M","transport":"modbus-tcp","dept":"motion","kinds":["motion.run"],"value":"[A-Z]+","never_addresses":["coil:30-39","reg:100"],"actions":{{"motion.run":{{"steps":[{{"send":"{step}"}}]}}}}}}"#);
        assert!(LinkProfile::from_json(&mk("write_coil 10 1"), false).is_ok());
        for (bad, why) in [("write_coil 35 1", "must never write"), ("write_coil 30 0", "must never write"), ("write_reg 100 {n}", "must never write"), ("write_reg 101 {n}", ""), ("write_reg 7 {0}", "not a value"), ("play", "not a Modbus command")] {
            let r = LinkProfile::from_json(&mk(bad), false);
            if why.is_empty() {
                assert!(r.is_ok(), "{bad}: {:?}", r.err());
            } else {
                let e = r.err().unwrap_or_else(|| panic!("{bad} should be refused"));
                assert!(e.contains(why), "{bad}: {e}");
            }
        }
        // text-only settings do not belong on a Modbus profile
        assert!(LinkProfile::from_json(&mk("write_coil 10 1").replace("\"dept\"", "\"request_end\":\"\",\"dept\""), false).err().unwrap().contains("Modbus frames its own messages"));
    }

    #[tokio::test]
    async fn a_run_writes_the_number_pulses_the_start_reads_back_and_never_touches_the_reserved_coils() {
        with_plc_profile();
        let sim = crate::plcsim::PlcSim::start(1, &[3, 5]).await.unwrap();
        let links = TextLinks::default();
        let d = plc_dev(sim.port, "OPEN_ARM=3, PARK=5");
        let o = links.apply(&d, &run("OPEN_ARM")).await.unwrap();
        assert_eq!(o.status, "running", "{}", o.note);
        assert_eq!(sim.seen(), ["read_input 20", "read_input 21", "read_ireg 200", "write_reg 100 3", "write_coil 10 1", "write_coil 10 0", "read_ireg 200"]);
        let st = sim.snapshot();
        assert_eq!((st.iregs[200], st.iregs[201], st.coils[10]), (1, 3, false), "running program 3, and the start pulse was released");
        assert!(!sim.seen().iter().any(|c| c.starts_with("write_coil 3")), "a reserved coil was written");
        // it is running: another run is refused by the profile's own idle check, and nothing is written
        sim.with(|s| s.seen.clear());
        let e = links.apply(&d, &run("PARK")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("not idle"), "{}", e.text());
        assert!(!sim.seen().iter().any(|c| c.starts_with("write_")), "{:?}", sim.seen());
        // the stop is a second pulse, on its own coil
        let o = links.stop(&d).await.unwrap();
        assert_eq!(o.status, "accepted");
        assert_eq!(sim.snapshot().iregs[200], 0);
        assert!(sim.seen().iter().any(|c| c == "write_coil 11 1") && sim.seen().iter().any(|c| c == "write_coil 11 0"));
    }

    #[tokio::test]
    async fn the_plc_route_has_the_same_allow_list_and_a_program_needs_its_number() {
        with_plc_profile();
        let sim = crate::plcsim::PlcSim::start(1, &[3]).await.unwrap();
        let links = TextLinks::default();
        for (prog, list, why) in [("OPEN_ARM", "OPEN_ARM", "has no program number"), ("PARK", "OPEN_ARM=3", "not on"), ("OPEN_ARM", "", "no list of allowed programs"), ("open_arm", "OPEN_ARM=3", "not a value")] {
            let e = links.apply(&plc_dev(sim.port, list), &run(prog)).await.err().unwrap_or_else(|| panic!("{prog} should be refused"));
            assert!(e.text().contains(why), "{prog}/{list}: {}", e.text());
        }
        assert!(sim.seen().is_empty(), "not one byte reached the PLC: {:?}", sim.seen());
        assert_eq!(program_number(&plc_dev(1, "A=7, B"), "A"), Some(7));
        assert_eq!(program_number(&plc_dev(1, "A=7, B"), "B"), None);
        assert_eq!(allowed_programs(&plc_dev(1, "A=7, B")), ["A", "B"]);
    }

    #[tokio::test]
    async fn the_plc_saying_no_stops_the_sequence_before_anything_is_written() {
        with_plc_profile();
        let sim = crate::plcsim::PlcSim::start(1, &[3]).await.unwrap();
        let links = TextLinks::default();
        let d = plc_dev(sim.port, "OPEN_ARM=3");
        sim.fault("not-remote").unwrap();
        let e = links.apply(&d, &run("OPEN_ARM")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("not accepting commands from outside"), "{}", e.text());
        sim.fault("healthy").unwrap();
        sim.fault("safety-fault").unwrap();
        let e = links.apply(&d, &run("OPEN_ARM")).await.err().unwrap();
        assert!(e.text().contains("safety status is not OK"), "{}", e.text());
        assert!(!sim.seen().iter().any(|c| c.starts_with("write_")), "something was written to a PLC that said no: {:?}", sim.seen());
    }

    #[tokio::test]
    async fn if_a_step_fails_after_the_start_was_raised_the_cleanup_lowers_it() {
        // step 2 writes an address the PLC does not have: it answers exception 2, after the start coil was raised
        let text = r#"{"id":"plc-clean","name":"C","transport":"modbus-tcp","dept":"motion","kinds":["motion.run"],"value":"[A-Z]+","port":1,
            "actions":{"motion.run":{"steps":[{"send":"write_coil 10 1","expect":"OK"},{"send":"write_reg 900 1","expect":"OK","fail":"the PLC said {reply}"}],
                                      "cleanup":[{"send":"write_coil 10 0"}]}}}"#;
        registry().write().unwrap().insert("plc-clean".into(), Arc::new(LinkProfile::from_json(text, false).unwrap()));
        let sim = crate::plcsim::PlcSim::start(1, &[3]).await.unwrap();
        let mut d = dev(sim.port, "");
        d.kind = "plc-clean".into();
        d.programs = None;
        let links = TextLinks::default();
        let e = links.apply(&d, &run("GO")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("EXCEPTION 2 illegal data address"), "{}", e.text());
        assert_eq!(sim.seen(), ["write_coil 10 1", "write_reg 900 1", "write_coil 10 0"], "the cleanup ran, and last");
        assert!(!sim.snapshot().coils[10], "the start coil was left lowered");
        registry().write().unwrap().remove("plc-clean");
    }

    #[tokio::test]
    async fn the_plc_is_read_for_state_and_a_fault_makes_it_not_ready_and_the_probe_reads_it_too() {
        with_plc_profile();
        let sim = crate::plcsim::PlcSim::start(1, &[3]).await.unwrap();
        let links = TextLinks::default();
        let d = plc_dev(sim.port, "OPEN_ARM=3");
        links.ensure(&d).await.unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(4);
        while !links.state("ur1").is_some_and(|s| s.state == "linked") {
            assert!(std::time::Instant::now() < until, "never linked: {:?}", links.state("ur1"));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(links.state("ur1").unwrap().ready, Some(true));
        sim.fault("safety-fault").unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(4);
        while links.state("ur1").is_some_and(|s| s.ready != Some(false)) {
            assert!(std::time::Instant::now() < until, "never noticed the safety fault");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(links.state("ur1").unwrap().note.contains("safety status is not OK"), "{:?}", links.state("ur1"));
        // the probe reads, and sends no writes
        sim.fault("healthy").unwrap();
        sim.with(|s| s.seen.clear());
        let p = get("kuka-plc-example").unwrap();
        let r = probe(&p, &format!("127.0.0.1:{}", sim.port)).await.unwrap();
        assert_eq!(r.greeting, None);
        assert!(r.all_matched() && r.ready == Some(true), "{r:?}");
        assert!(sim.seen().iter().all(|c| c.starts_with("read_")), "a probe only reads: {:?}", sim.seen());
    }

    #[test]
    fn what_a_plc_cue_would_send_is_readable_with_its_number_in() {
        with_plc_profile();
        let lines = describe(&plc_dev(1, "OPEN_ARM=3"), &run("OPEN_ARM")).unwrap();
        let text: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert!(text.iter().any(|l| l.contains("Modbus TCP: write_reg 100 3")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("write_coil 10 1")), "{text:?}");
    }

    #[test]
    fn what_a_cue_would_send_can_be_read_without_sending_it() {
        let d = dev(1, "OPEN_ARM");
        let lines = describe(&d, &run("OPEN_ARM")).unwrap();
        let text: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert!(text.iter().any(|l| l.contains("load OPEN_ARM.urp") && l.contains("skipped if it is already loaded")), "{text:?}");
        assert!(text.iter().any(|l| l.ends_with("play")), "{text:?}");
        assert!(describe(&d, &run("PARK")).is_err());
    }

    // ---- PJLink: a projector that ends every line with a carriage return and never sends a line feed ----

    fn projector_dev(port: u16) -> Device {
        Device { id: "pj1".into(), label: "Projector".into(), kind: "pjlink".into(), dept: "visual".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }

    /// A pretend projector. `greeting` is what it says on connect; `bad_input` is an input it says it does not have. Returns the port and
    /// every request it received, as text without the carriage return.
    async fn pretend_projector(greeting: &'static str, bad_input: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let s2 = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = l.accept().await else { continue };
                let seen = Arc::clone(&s2);
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let _ = wr.write_all(format!("{greeting}\r").as_bytes()).await;
                    let mut rd = tokio::io::BufReader::new(rd);
                    let mut power = 0;
                    loop {
                        let mut b = Vec::new();
                        match rd.read_until(b'\r', &mut b).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) => {}
                        }
                        let req = String::from_utf8_lossy(&b).trim_end_matches('\r').to_string();
                        seen.lock().unwrap().push(req.clone());
                        let reply = match req.as_str() {
                            "%1POWR 1" => {
                                power = 1;
                                "%1POWR=OK".to_string()
                            }
                            "%1POWR 0" => {
                                power = 0;
                                "%1POWR=OK".to_string()
                            }
                            "%1POWR ?" => format!("%1POWR={power}"),
                            "%1ERST ?" => "%1ERST=000000".to_string(),
                            r if r.starts_with("%1INPT ") => if r.ends_with(bad_input) { "%1INPT=ERR2".to_string() } else { "%1INPT=OK".to_string() },
                            r if r.starts_with("%1AVMT ") => "%1AVMT=OK".to_string(),
                            _ => "%1UNKN=ERR1".to_string(),
                        };
                        // the reply is written in two pieces, with no line feed anywhere: a reader that waits for one would hang
                        let (a, c) = reply.split_at(4);
                        let _ = wr.write_all(a.as_bytes()).await;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let _ = wr.write_all(format!("{c}\r").as_bytes()).await;
                    }
                });
            }
        });
        (port, seen)
    }

    fn pj(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"value": v}))
    }

    #[test]
    fn the_built_in_pjlink_profile_cannot_switch_a_projector_off_and_its_whole_command_set_is_known() {
        let p = get("pjlink").expect("pjlink profile");
        assert_eq!((p.dept.as_str(), p.port, p.built_in, p.basis.as_str()), ("visual", Some(4352), true, "community"));
        assert_eq!(p.commands(), ["%1AVMT {0}", "%1ERST ?", "%1INPT {0}", "%1POWR 1", "%1POWR ?"]);
        assert!(!p.commands().iter().any(|c| c.to_ascii_lowercase().starts_with("%1powr 0")), "no way to power off");
        // and a profile that could is refused when it is loaded
        let bad = include_str!("../profiles/link/pjlink.json").replace("\"%1POWR 1\"", "\"%1POWR 0\"");
        assert!(LinkProfile::from_json(&bad, false).err().unwrap().contains("must never send"));
    }

    #[test]
    fn a_pjlink_cue_is_checked_against_that_actions_own_values() {
        let d = projector_dev(1);
        assert!(check_action(&d, &pj("visual.input", "31")).is_ok());
        assert!(check_action(&d, &pj("visual.mute", "11")).is_ok());
        assert!(check_action(&d, &Action::new("visual.power_on", json!({}))).is_ok(), "power on takes no value");
        for bad in ["99", "10", "3", "311", "hdmi1", "31\r%1POWR 0"] {
            assert!(check_action(&d, &pj("visual.input", bad)).is_err(), "input {bad}");
        }
        for bad in ["34", "12", "on", "1"] {
            assert!(check_action(&d, &pj("visual.mute", bad)).is_err(), "mute {bad}");
        }
        assert!(check_action(&d, &pj("visual.power_off", "1")).unwrap_err().contains("no mapping"));
        assert!(check_action(&d, &pj("scene.activate", "3")).unwrap_err().contains("no mapping"));
        assert!(check_action(&d, &pj("visual.input", "")).unwrap_err().contains("no value"));
        let text = describe(&d, &pj("visual.mute", "31")).unwrap();
        assert!(text[0].0.ends_with("%1AVMT 31"), "{text:?}");
    }

    #[tokio::test]
    async fn a_projector_is_driven_with_carriage_returns_only_and_its_words_are_reported() {
        let (port, seen) = pretend_projector("PJLINK 0", "99").await;
        let links = TextLinks::default();
        let d = projector_dev(port);
        let o = links.apply(&d, &Action::new("visual.power_on", json!({}))).await.unwrap();
        assert_eq!(o.status, "running", "a read-back shows it is on: {}", o.note);
        assert_eq!(links.apply(&d, &pj("visual.input", "31")).await.unwrap().status, "accepted");
        assert_eq!(links.apply(&d, &pj("visual.mute", "31")).await.unwrap().status, "accepted");
        assert_eq!(*seen.lock().unwrap(), ["%1POWR 1", "%1POWR ?", "%1INPT 31", "%1AVMT 31"]);
        for c in seen.lock().unwrap().iter() {
            assert!(!c.to_ascii_lowercase().starts_with("%1powr 0"), "PCHI sent '{c}'");
        }
    }

    #[tokio::test]
    async fn a_projector_that_says_no_is_refused_in_its_own_words() {
        // 99 is not a projector input; the pattern lets it through only if it is 11 to 59, so use an input the projector lacks
        let (port, _) = pretend_projector("PJLINK 0", "55").await;
        let links = TextLinks::default();
        let e = links.apply(&projector_dev(port), &pj("visual.input", "55")).await.err().unwrap();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("select input 55") && e.text().contains("ERR2"), "{}", e.text());
    }

    #[tokio::test]
    async fn a_projector_that_wants_a_password_is_refused_and_sent_nothing() {
        for greeting in ["PJLINK 1 498e4a67", "PJLINK ERRA"] {
            let (port, seen) = pretend_projector(greeting, "99").await;
            let links = TextLinks::default();
            let e = links.apply(&projector_dev(port), &pj("visual.input", "31")).await.err().unwrap();
            assert!(e.text().contains("PJLink password"), "{greeting}: {}", e.text());
            assert!(seen.lock().unwrap().is_empty(), "{greeting}: nothing may be sent to it: {:?}", seen.lock().unwrap());
            let r = probe(&get("pjlink").unwrap(), &format!("127.0.0.1:{port}")).await;
            assert!(r.err().unwrap().contains("PJLink password"));
            assert!(seen.lock().unwrap().is_empty(), "a probe sends nothing either");
        }
    }

    #[tokio::test]
    async fn the_pjlink_probe_reads_only_and_reports_the_greeting() {
        let (port, seen) = pretend_projector("PJLINK 0", "99").await;
        let r = probe(&get("pjlink").unwrap(), &format!("127.0.0.1:{port}")).await.unwrap();
        assert_eq!(r.greeting.as_deref(), Some("PJLINK 0"));
        assert!(r.all_matched(), "{r:?}");
        assert_eq!(r.ready, None, "no readiness rule: a projector in standby must still be able to be powered on");
        assert_eq!(*seen.lock().unwrap(), ["%1POWR ?", "%1ERST ?"]);
    }

    #[test]
    fn a_value_can_be_taken_apart_and_the_pieces_put_into_the_command() {
        let text = r#"{"id":"fixed","name":"Fixed","transport":"tcp-text","dept":"visual","port":9000,"request_end":"\r\n",
            "kinds":["visual.cue"],"value":"[0-9]{1,3}","hint":"page/row",
            "actions":{"visual.cue":{"value":"(\\d{1,2})/(\\d{1,2})","steps":[{"send":"CUE {1} {2}","expect":"OK"}]}}}"#;
        let p = LinkProfile::from_json(text, false).unwrap();
        let a = p.actions.get("visual.cue").unwrap();
        let v = "3/12";
        let sub = Subst { name: v, num: None, caps: caps_of(a.value.as_ref().unwrap(), v) };
        assert_eq!(sub.render("CUE {1} {2}"), "CUE 3 12");
        assert_eq!(sub.render("{0}|{1}"), "3/12|3");
        // a part that is not there, or a placeholder that is not one, is refused when the profile is loaded
        let err = |t: &str| LinkProfile::from_json(t, false).err().unwrap();
        assert!(err(&text.replace("CUE {1} {2}", "CUE {1} {3}")).contains("uses {3} but the value pattern has 2"));
        assert!(err(&text.replace("CUE {1} {2}", "CUE {x}")).contains("the only placeholders"));
        assert!(err(&text.replace("\"actions\"", "\"stop\":[{\"send\":\"STOP {1}\"}],\"actions\"")).contains("a stop has no program to put in"));
        // what comes out of a value is never read as a placeholder again
        let odd = Subst { name: "a{1}b", num: None, caps: vec!["X".into()] };
        assert_eq!(odd.render("{0}-{1}"), "a{1}b-X");
        assert_eq!(odd.render("{n}{z}"), "{n}{z}", "a placeholder with nothing to put stays as written");
    }

    #[test]
    fn the_fixed_line_recipe_in_the_docs_loads_and_says_what_it_would_send() {
        // the same JSON as docs/link-profiles.md ("A device that takes fixed lines")
        let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/link-profiles.md")).unwrap();
        let json = doc.split("## A device that takes fixed lines").nth(1).and_then(|s| s.split("```json").nth(1)).and_then(|s| s.split("```").next()).expect("the recipe is in the doc");
        let p = LinkProfile::from_json(json, false).expect("the recipe in the doc is a valid profile");
        assert_eq!(p.commands(), ["RELAY {1} ON"]);
        let a = p.actions.get("lighting.look").unwrap();
        let ok = |v: &str| a.value.as_ref().unwrap().is_match(v);
        assert!(ok("1") && ok("16") && !ok("0") && !ok("17") && !ok("1;format"));
        let sub = Subst { name: "12", num: None, caps: caps_of(a.value.as_ref().unwrap(), "12") };
        assert_eq!(sub.render("RELAY {1} ON"), "RELAY 12 ON");
        assert!(LinkProfile::from_json(&json.replace("\"RELAY {1} ON\"", "\"FORMAT\""), false).err().unwrap().contains("must never send"));
    }

    #[test]
    fn a_carriage_return_is_a_line_end_a_profile_may_name_and_nothing_else_is() {
        let base = include_str!("../profiles/link/pjlink.json");
        assert!(LinkProfile::from_json(base, false).is_ok());
        assert!(LinkProfile::from_json(&base.replace("\"request_end\": \"\\r\"", "\"request_end\": \"\\t\""), false).err().unwrap().contains("request_end"));
        assert!(LinkProfile::from_json(&base.replace("\"reply_until\": \"\\r\"", "\"reply_until\": \"\\u0007\""), false).err().unwrap().contains("reply_until"));
        assert!(LinkProfile::from_json(&base.replace("\"why\": \"this projector", "\"why\": \"\", \"x\": \"this projector"), false).err().unwrap().contains("greeting_refuse"));
        assert!(LinkProfile::from_json(&base.replace("^PJLINK (1 |ERRA)", "("), false).err().unwrap().contains("greeting_refuse"));
    }
}
