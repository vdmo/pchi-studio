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
}

#[derive(Deserialize)]
struct RawAction {
    steps: Vec<RawStep>,
    #[serde(default)]
    confirm: Option<RawStep>,
}

#[derive(Deserialize)]
struct RawRead {
    var: String,
    send: String,
    expect: String,
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
}

struct Action2 {
    steps: Vec<Step>,
    confirm: Option<Step>,
}

struct Read {
    var: String,
    send: String,
    expect: Regex,
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
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

/// A command template: printable text, with `{0}` as its only placeholder.
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
        if !after.starts_with("{0}") {
            return Err(format!("'{t}': the only placeholder is {{0}} (the program name)"));
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
    })
}

impl LinkProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<LinkProfile, String> {
        let raw: RawLink = serde_json::from_str(text).map_err(|e| format!("not a link profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("id '{id}' must be 1 to 32 characters: a to z, 0 to 9 and -"));
        }
        if raw.transport != "tcp-text" {
            return Err(format!("{id}: transport must be \"tcp-text\""));
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
            actions.insert(kind.clone(), Action2 { steps, confirm });
        }
        let mut stop = Vec::new();
        for (n, s) in raw.stop.iter().enumerate() {
            let at = format!("{id} stop step {}", n + 1);
            sent.push((at.clone(), s.send.clone()));
            stop.push(step_of(s, &at)?);
        }
        let (mut reads, mut ready_when, mut poll) = (Vec::new(), Vec::new(), Duration::from_secs(1));
        if let Some(st) = &raw.status {
            poll = Duration::from_millis(st.poll_ms.unwrap_or(1000).clamp(200, 30_000));
            for (n, r) in st.reads.iter().enumerate() {
                let at = format!("{id} status read {}", n + 1);
                template_ok(&r.send).map_err(|e| format!("{at}: {e}"))?;
                if r.send.contains("{0}") {
                    return Err(format!("{at}: a status read has no program name to put in"));
                }
                sent.push((at.clone(), r.send.clone()));
                reads.push(Read { var: r.var.clone(), send: r.send.clone(), expect: full(&r.expect).map_err(|e| format!("{at}: {e}"))? });
            }
            for r in &st.ready_when {
                if !st.reads.iter().any(|x| x.var == r.var) {
                    return Err(format!("{id} ready_when: '{}' is not read by any status read", r.var));
                }
                if r.equals.is_none() && r.one_of.is_none() {
                    return Err(format!("{id} ready_when '{}': give equals or one_of", r.var));
                }
                ready_when.push(Ready { var: r.var.clone(), equals: r.equals.clone(), one_of: r.one_of.clone().unwrap_or_default(), why: r.why.clone() });
            }
        }
        // the never list: nothing the profile can send may begin with a forbidden command
        for (at, cmd) in &sent {
            let c = cmd.trim().to_ascii_lowercase();
            if let Some(bad) = never.iter().find(|n| c == **n || c.starts_with(&format!("{n} "))) {
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
        })
    }

    /// Every command this profile can ever send, in the order a person would read them. This is the whole of what the device is ever told.
    pub fn commands(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for a in self.actions.values() {
            for s in a.steps.iter().chain(a.confirm.iter()) {
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

const BUILT_IN: [&str; 1] = [include_str!("../profiles/link/ur.json")];

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
    v.get("transport").and_then(Value::as_str) == Some("tcp-text")
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

/// The names on a device's allow-list (the rig node's `programs`: names separated by commas, spaces or semicolons).
pub fn allowed_programs(dev: &Device) -> Vec<String> {
    dev.programs.as_deref().unwrap_or("").split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Check a cue against the profile and the device's allow-list. Sends nothing. The error says what to fix.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    let p = get(dev.profile_key()).ok_or_else(|| format!("{} has no link profile", dev.label))?;
    let a = p.actions.get(&action.kind);
    if a.is_none() {
        return Err(format!("{} has no mapping for \"{}\"", p.name, action.kind));
    }
    // pause and stop take no program name; run does. A step uses {0} only if it needs one.
    let needs_value = a.is_some_and(|a| a.steps.iter().chain(a.confirm.iter()).any(|s| s.send.contains("{0}") || s.skip_if.is_some()));
    let v = value_of(action);
    if !needs_value {
        return Ok(());
    }
    if v.is_empty() {
        return Err(format!("{} has no value: write the program name in the Cue sheet", action.kind));
    }
    if !p.value.is_match(&v) {
        return Err(format!("\"{v}\" is not a program name {} accepts: use {}", p.name, if p.hint.is_empty() { "letters, digits and _ . -" } else { &p.hint }));
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
    Ok(())
}

/// The commands a cue would send, in order, for a person to read (a step that is skipped when something is already true says so).
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    check_action(dev, action)?;
    let p = get(dev.profile_key()).ok_or("no link profile")?;
    let a = p.actions.get(&action.kind).ok_or("no mapping")?;
    let v = value_of(action);
    let mut out = Vec::new();
    for s in &a.steps {
        let line = s.send.replace("{0}", &v);
        let note = if s.skip_if.is_some() { " (skipped if it is already loaded)" } else { "" };
        out.push((format!("{} text: {}{}", p.name, line, note), vec![]));
    }
    if let Some(c) = &a.confirm {
        out.push((format!("{} text: {} (read back, to see it running)", p.name, c.send.replace("{0}", &v)), vec![]));
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

pub struct Outcome {
    /// `accepted` (it answered yes) or `running` (a read-back shows it is doing it).
    pub status: &'static str,
    pub note: String,
    pub target: String,
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

fn render(t: &str, v: &str) -> String {
    t.replace("{0}", v)
}

impl Link {
    async fn connect(&self, conn: &mut Conn) -> Result<(), String> {
        if conn.stream.is_some() {
            return Ok(());
        }
        let s = tokio::time::timeout(CONNECT, TcpStream::connect(&self.addr)).await.map_err(|_| format!("{} did not answer within {} s", self.addr, CONNECT.as_secs()))?.map_err(|e| format!("cannot reach {}: {e}", self.addr))?;
        let _ = s.set_nodelay(true);
        let mut r = BufReader::new(s);
        // some devices greet on connect: take that line if it comes, so it is not read as the answer to the first command
        let mut greeting = String::new();
        let _ = tokio::time::timeout(GREETING, r.read_line(&mut greeting)).await;
        conn.stream = Some(r);
        Ok(())
    }

    /// One command, one reply line. On any trouble the connection is dropped, so a late reply can never be read as the next one's.
    async fn exchange(&self, conn: &mut Conn, cmd: &str) -> Result<String, String> {
        self.connect(conn).await?;
        let r = conn.stream.as_mut().ok_or("not connected")?;
        let go = async {
            r.get_mut().write_all(format!("{cmd}\n").as_bytes()).await.map_err(|e| format!("could not send to {}: {e}", self.addr))?;
            let mut line = String::new();
            match r.read_line(&mut line).await {
                Ok(0) => Err(format!("{} closed the connection", self.addr)),
                Ok(_) => Ok(line.trim().to_string()),
                Err(e) => Err(format!("could not read from {}: {e}", self.addr)),
            }
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
    async fn run_steps(&self, conn: &mut Conn, steps: &[Step], value: &str, vars: &mut HashMap<String, String>) -> Result<(), LinkError> {
        for s in steps {
            if let Some((var, pat)) = &s.skip_if {
                let re = full(&pat.replace("{0}", &regex::escape(value))).map_err(LinkError::Failed)?;
                if vars.get(var).is_some_and(|x| re.is_match(x)) {
                    continue;
                }
            }
            let reply = self.exchange(conn, &render(&s.send, value)).await.map_err(LinkError::Failed)?;
            if let Some(re) = &s.expect {
                match re.captures(&reply) {
                    None => return Err(LinkError::Refused(render(&s.fail, value).replace("{reply}", &reply))),
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

    async fn confirm(&self, conn: &mut Conn, c: &Step, value: &str) -> Result<bool, LinkError> {
        let until = tokio::time::Instant::now() + c.within;
        loop {
            let reply = self.exchange(conn, &render(&c.send, value)).await.map_err(LinkError::Failed)?;
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
        let mut conn = self.lock().await?;
        let mut vars = HashMap::new();
        let result = self.run_steps(&mut conn, &a.steps, &value, &mut vars).await;
        if let Err(e) = &result {
            self.note_failure(e);
        }
        result?;
        let target = self.addr.clone();
        match &a.confirm {
            None => Ok(Outcome { status: "accepted", note: "the device said yes".into(), target }),
            Some(c) => match self.confirm(&mut conn, c, &value).await {
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
        let mut r = self.run_steps(&mut conn, &self.profile.stop, "", &mut vars).await;
        if matches!(r, Err(LinkError::Failed(_))) {
            conn.stream = None;
            r = self.run_steps(&mut conn, &self.profile.stop, "", &mut vars).await;
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
                Ok(reply) => {
                    let v = r.expect.captures(&reply).map(|c| c.get(1).map_or(reply.clone(), |m| m.as_str().to_string()));
                    vars.insert(r.var.clone(), v.unwrap_or_else(|| format!("? ({reply})")));
                }
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
        let link = Arc::new(Link { profile: Arc::clone(&profile), addr, key, conn: tokio::sync::Mutex::new(Conn { stream: None }), state: Mutex::new(LinkState { state: "silent", ..Default::default() }), was_linked: Default::default(), task: Mutex::new(None) });
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
                if r.expect.captures(&reply).is_none() {
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
        assert!(LinkProfile::from_json(&mk("play {1}"), false).err().unwrap().contains("only placeholder"));
        assert!(LinkProfile::from_json(&mk("play\\nstop"), false).err().unwrap().contains("control character"), "one command is one line");
        assert!(LinkProfile::from_json(&mk("play").replace("tcp-text", "udp"), false).is_err());
    }

    #[tokio::test]
    async fn a_cue_not_on_the_allow_list_sends_nothing_at_all() {
        let sim = UrSim::start(&["OPEN_ARM", "PARK"]).await.unwrap();
        let links = TextLinks::default();
        for (prog, list, why) in [("OPEN_ARM", "", "no list of allowed programs"), ("PARK", "OPEN_ARM", "not on UR's list"), ("OPEN ARM", "OPEN_ARM", "not a program name"), ("a;power on", "a;power on", "not a program name"), ("", "OPEN_ARM", "no value")] {
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

    #[test]
    fn what_a_cue_would_send_can_be_read_without_sending_it() {
        let d = dev(1, "OPEN_ARM");
        let lines = describe(&d, &run("OPEN_ARM")).unwrap();
        let text: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert!(text.iter().any(|l| l.contains("load OPEN_ARM.urp") && l.contains("skipped if it is already loaded")), "{text:?}");
        assert!(text.iter().any(|l| l.ends_with("play")), "{text:?}");
        assert!(describe(&d, &run("PARK")).is_err());
    }
}
