//! Device profiles: how a PCHI action becomes the OSC a tool actually understands.
//!
//! A profile is **data**, a small JSON file: which action kinds it maps, and an ordered list of rules, each a pattern on the cue's
//! value and the OSC message to send when it matches. The seven built-in profiles (Resolume, grandMA3, Eos, X32, BEYOND, QLab,
//! MagicQ) are such files, in `crates/pchi-studio-api/profiles/`. A crew can add or correct a tool without touching the code:
//! put a file in a folder and start the runtime with `--profiles <folder>` (see `docs/profiles.md`). A profile file with a built-in's
//! `id` replaces it.
//!
//! Every rule says what it **rests on** (`basis`): `vendor` (the vendor's own documentation), `community` (reported by users or
//! third-party manuals), or `memory` (not confirmed). The Doors check warns when a cue goes out through a rule that is not `vendor`.
//! Whether a message really works on the real tool is another matter: that is what `pchi-gearcheck` is for.
//!
//! Safety is in the format, not in each profile's good behaviour:
//! - every address that is built must be a plain OSC address (letters, digits and `/_-.`), whatever the value was;
//! - a string argument may not contain control characters or exceed 200 characters;
//! - a value must match a rule's whole pattern, so a profile can only send what its patterns allow (grandMA3's rule lists the
//!   playback command words and nothing else, so a show file cannot delete or store anything on a console).
//!
//! Ports are never assumed: they are whatever is set in the tool.

use pchi_showchart::Action;
use regex::{Regex, RegexBuilder};
use rosc::{encoder, OscMessage, OscPacket, OscType};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

/// The built-in profiles' ids.
pub const PROFILES: [&str; 7] = ["resolume", "grandma3", "eos", "x32", "beyond", "qlab", "magicq"];

const BUILT_IN: [&str; 7] = [
    include_str!("../profiles/resolume.json"),
    include_str!("../profiles/grandma3.json"),
    include_str!("../profiles/eos.json"),
    include_str!("../profiles/x32.json"),
    include_str!("../profiles/beyond.json"),
    include_str!("../profiles/qlab.json"),
    include_str!("../profiles/magicq.json"),
];

fn pack(addr: &str, args: Vec<OscType>) -> Result<Vec<u8>, String> {
    encoder::encode(&OscPacket::Message(OscMessage { addr: addr.to_string(), args })).map_err(|e| format!("{e:?}"))
}

fn safe_address(s: &str) -> bool {
    s.starts_with('/') && s.len() <= 200 && s.chars().all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c))
}

/// What a rule rests on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Basis {
    Vendor,
    Community,
    Memory,
}

impl Basis {
    pub fn word(self) -> &'static str {
        match self {
            Basis::Vendor => "vendor",
            Basis::Community => "community",
            Basis::Memory => "memory",
        }
    }
}

#[derive(Clone, Debug)]
enum ArgKind {
    Int,
    Float,
    Str,
}

#[derive(Clone, Debug)]
struct ArgSpec {
    kind: ArgKind,
    template: String,
}

#[derive(Clone, Debug)]
struct Msg {
    address: String,
    args: Vec<ArgSpec>,
}

pub struct Rule {
    re: Option<Regex>,
    kinds: Option<Vec<String>>,
    ignore_value: bool,
    send: String,
    args: Vec<ArgSpec>,
    pub basis: Basis,
    pub source: String,
    /// The pattern as written, for messages.
    pub pattern: String,
}

/// What a message the tool sends back means as a cue: when it fires something, the cue that would fire the same thing.
pub struct CaptureRule {
    re: Regex,
    pub kind: String,
    value: String,
    pub basis: Basis,
    pub source: String,
    pub pattern: String,
}

pub struct Profile {
    pub id: String,
    pub name: String,
    pub dept: String,
    kinds: Vec<String>,
    pub hint: String,
    error: String,
    pub rules: Vec<Rule>,
    probe: Option<Msg>,
    stop: Vec<Msg>,
    pub capture: Vec<CaptureRule>,
    pub notes: String,
    pub built_in: bool,
}

/// The parts of a profile file, as written.
#[derive(Deserialize)]
struct RawProfile {
    id: String,
    name: String,
    #[serde(default)]
    dept: Option<String>,
    kinds: Vec<String>,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    error: Option<String>,
    rules: Vec<RawRule>,
    #[serde(default)]
    probe: Option<RawMsg>,
    #[serde(default)]
    stop: Vec<RawMsg>,
    #[serde(default)]
    capture: Vec<RawCapture>,
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize)]
struct RawCapture {
    listen: String,
    kind: String,
    value: String,
    #[serde(default)]
    basis: Option<String>,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct RawRule {
    #[serde(default, rename = "match")]
    pattern: Option<String>,
    #[serde(default)]
    kinds: Option<Vec<String>>,
    #[serde(default)]
    ignore_value: bool,
    send: String,
    #[serde(default)]
    args: Vec<Value>,
    #[serde(default)]
    basis: Option<String>,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct RawMsg {
    address: String,
    #[serde(default)]
    args: Vec<Value>,
}

/// `{1}` is the first capture of the pattern, `{0}` the whole value; `{1|lower}` and `{1|upper}` change case.
fn render(t: &str, caps: &[String]) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}').map(|j| j + i).ok_or_else(|| format!("'{t}' has a {{ with no }}"))?;
        let tok = &rest[i + 1..j];
        let (n, f) = tok.split_once('|').unwrap_or((tok, ""));
        let idx: usize = n.trim().parse().map_err(|_| format!("'{{{tok}}}' in '{t}': write {{0}}, {{1}}, {{2}}..."))?;
        let g = caps.get(idx).ok_or_else(|| format!("'{{{tok}}}' in '{t}' names a part the pattern does not capture"))?;
        out.push_str(&match f.trim() {
            "" => g.clone(),
            "lower" => g.to_lowercase(),
            "upper" => g.to_uppercase(),
            other => return Err(format!("'{{{tok}}}': '{other}' is not lower or upper")),
        });
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Check a template names only parts the pattern has (`groups` = captures, whole match counted).
fn check_template(t: &str, groups: usize) -> Result<(), String> {
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        let j = rest[i..].find('}').map(|j| j + i).ok_or_else(|| format!("'{t}' has a {{ with no }}"))?;
        let tok = &rest[i + 1..j];
        let (n, f) = tok.split_once('|').unwrap_or((tok, ""));
        let idx: usize = n.trim().parse().map_err(|_| format!("'{{{tok}}}' in '{t}': write {{0}}, {{1}}, {{2}}..."))?;
        if idx >= groups {
            return Err(format!("'{{{tok}}}' in '{t}' names a part the pattern does not capture"));
        }
        if !matches!(f.trim(), "" | "lower" | "upper") {
            return Err(format!("'{{{tok}}}': '{f}' is not lower or upper"));
        }
        rest = &rest[j + 1..];
    }
    Ok(())
}

fn arg_spec(v: &Value) -> Result<ArgSpec, String> {
    let o = v.as_object().filter(|o| o.len() == 1).ok_or("an argument is one of {\"int\": ...}, {\"float\": ...} or {\"string\": ...}")?;
    let (k, val) = o.iter().next().unwrap();
    let template = match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err("an argument's value is a string or a number".into()),
    };
    let kind = match k.as_str() {
        "int" => ArgKind::Int,
        "float" => ArgKind::Float,
        "string" => ArgKind::Str,
        other => return Err(format!("argument type '{other}' is not int, float or string")),
    };
    Ok(ArgSpec { kind, template })
}

fn kinds_ok(kinds: &[String]) -> Result<(), String> {
    for k in kinds {
        let body = k.strip_suffix(".*").unwrap_or(k);
        if body.is_empty() || !body.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
            return Err(format!("action kind '{k}' is not like lighting.look or audio.*"));
        }
    }
    Ok(())
}

fn kind_matches(kinds: &[String], kind: &str) -> bool {
    kinds.iter().any(|k| k == kind || k.strip_suffix(".*").is_some_and(|p| kind.starts_with(p) && kind[p.len()..].starts_with('.')))
}

fn msg_of(raw: &RawMsg) -> Result<Msg, String> {
    if !safe_address(&raw.address) {
        return Err(format!("'{}' is not a plain OSC address (letters, digits and /_-.)", raw.address));
    }
    let args: Vec<ArgSpec> = raw.args.iter().map(arg_spec).collect::<Result<_, _>>()?;
    for a in &args {
        check_template(&a.template, 1)?;
    }
    Ok(Msg { address: raw.address.clone(), args })
}

impl Profile {
    /// Read and check one profile file.
    pub fn from_json(text: &str, built_in: bool) -> Result<Profile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a profile file: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("id '{id}' must be 1 to 32 characters: a to z, 0 to 9 and -"));
        }
        let dept = raw.dept.clone().unwrap_or_default();
        if !["visual", "lighting", "laser", "audio"].contains(&dept.as_str()) {
            return Err(format!("{id}: dept must be visual, lighting, laser or audio"));
        }
        if raw.kinds.is_empty() {
            return Err(format!("{id}: kinds is empty: list the action kinds this tool handles, for example [\"lighting.look\"]"));
        }
        kinds_ok(&raw.kinds).map_err(|e| format!("{id}: {e}"))?;
        if raw.rules.is_empty() {
            return Err(format!("{id}: no rules"));
        }
        let mut rules = Vec::new();
        for (n, r) in raw.rules.iter().enumerate() {
            let at = |e: String| format!("{id} rule {}: {e}", n + 1);
            let (re, groups) = match (&r.pattern, r.ignore_value) {
                (Some(p), _) => {
                    // the whole value must match: a rule never sends for a value that merely contains a good part
                    let re = RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| at(format!("pattern '{p}': {e}")))?;
                    let groups = re.captures_len();
                    (Some(re), groups)
                }
                (None, true) => (None, 1),
                (None, false) => return Err(at("needs a \"match\" pattern, or \"ignore_value\": true".into())),
            };
            if let Some(k) = &r.kinds {
                kinds_ok(k).map_err(at)?;
                if let Some(bad) = k.iter().find(|k| !kind_matches(&raw.kinds, k) && !raw.kinds.iter().any(|p| p == *k)) {
                    return Err(at(format!("kind '{bad}' is not one of the profile's kinds")));
                }
            }
            check_template(&r.send, groups).map_err(at)?;
            let probe_addr = r.send.replace(|c: char| c == '{' || c == '}', "");
            if !probe_addr.starts_with('/') && !r.send.starts_with('{') {
                return Err(at(format!("send '{}' must be an OSC address starting with /", r.send)));
            }
            let args: Vec<ArgSpec> = r.args.iter().map(arg_spec).collect::<Result<_, _>>().map_err(at)?;
            for a in &args {
                check_template(&a.template, groups).map_err(at)?;
            }
            let basis = match r.basis.as_deref() {
                Some("vendor") => Basis::Vendor,
                Some("community") => Basis::Community,
                Some("memory") | None => Basis::Memory,
                Some(other) => return Err(at(format!("basis '{other}' is not vendor, community or memory"))),
            };
            rules.push(Rule { re, kinds: r.kinds.clone(), ignore_value: r.ignore_value, send: r.send.clone(), args, basis, source: r.source.clone(), pattern: r.pattern.clone().unwrap_or_default() });
        }
        let probe = raw.probe.as_ref().map(msg_of).transpose().map_err(|e| format!("{id} probe: {e}"))?;
        let stop = raw.stop.iter().map(msg_of).collect::<Result<Vec<_>, _>>().map_err(|e| format!("{id} stop: {e}"))?;
        let mut capture = Vec::new();
        for (n, c) in raw.capture.iter().enumerate() {
            let at = |e: String| format!("{id} capture {}: {e}", n + 1);
            let re = RegexBuilder::new(&format!("^(?:{})$", c.listen)).size_limit(1 << 20).build().map_err(|e| at(format!("listen '{}': {e}", c.listen)))?;
            check_template(&c.value, re.captures_len()).map_err(at)?;
            kinds_ok(std::slice::from_ref(&c.kind)).map_err(at)?;
            if !kind_matches(&raw.kinds, &c.kind) {
                return Err(at(format!("kind '{}' is not one of the profile's kinds: a captured cue must be one the profile can send back", c.kind)));
            }
            let basis = match c.basis.as_deref() {
                Some("vendor") => Basis::Vendor,
                Some("community") => Basis::Community,
                Some("memory") | None => Basis::Memory,
                Some(other) => return Err(at(format!("basis '{other}' is not vendor, community or memory"))),
            };
            capture.push(CaptureRule { re, kind: c.kind.clone(), value: c.value.clone(), basis, source: c.source.clone(), pattern: c.listen.clone() });
        }
        let error = raw.error.unwrap_or_else(|| if raw.hint.is_empty() { "\"{value}\" is not a value {name} understands".into() } else { "\"{value}\": use {hint}".into() });
        Ok(Profile { id, name: raw.name, dept, kinds: raw.kinds, hint: raw.hint, error, rules, probe, stop, capture, notes: raw.notes, built_in })
    }

    /// The first value of the action's payload as text ("blue", 12 and "2/3" all arrive as text), None if there is none.
    fn value_of(action: &Action) -> Option<String> {
        let v = match &action.payload {
            Value::Object(p) => p.values().next().cloned(),
            _ => None,
        };
        let text = match v {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        (!text.is_empty()).then_some(text)
    }

    /// The rule that handles `action`, with the parts it captured (0 is the whole value). `Ok(None)` is "this tool has no mapping
    /// for that kind"; an error says what to fix.
    fn pick(&self, action: &Action) -> Result<Option<(&Rule, Vec<String>)>, String> {
        if !kind_matches(&self.kinds, &action.kind) {
            return Ok(None);
        }
        let value = Self::value_of(action);
        for r in &self.rules {
            if r.kinds.as_ref().is_some_and(|k| !k.iter().any(|x| x == &action.kind)) {
                continue;
            }
            if r.ignore_value {
                return Ok(Some((r, vec![value.unwrap_or_default()])));
            }
            let (Some(v), Some(re)) = (value.as_deref(), r.re.as_ref()) else { continue };
            if let Some(c) = re.captures(v) {
                return Ok(Some((r, c.iter().map(|m| m.map(|m| m.as_str().to_string()).unwrap_or_default()).collect())));
            }
        }
        match value {
            None => Err(format!("{} has no value: set one in the Cue sheet", action.kind)),
            Some(v) => Err(self.error.replace("{value}", &v).replace("{hint}", &self.hint).replace("{name}", &self.name)),
        }
    }

    /// The messages for `action`. `None` when this tool has no mapping for that kind of action.
    pub fn encode(&self, action: &Action) -> Option<Result<Vec<Vec<u8>>, String>> {
        let (rule, caps) = match self.pick(action) {
            Ok(Some(x)) => x,
            Ok(None) => return Some(Err(format!("{} has no mapping for \"{}\"", self.name, action.kind))),
            Err(e) => return Some(Err(e)),
        };
        Some((|| {
            let address = render(&rule.send, &caps)?;
            if !safe_address(&address) {
                return Err(format!("\"{address}\" is not a usable OSC address"));
            }
            let mut args = Vec::new();
            for a in &rule.args {
                let text = render(&a.template, &caps)?;
                args.push(match a.kind {
                    ArgKind::Int => OscType::Int(text.trim().parse().map_err(|_| format!("\"{text}\" is not a whole number"))?),
                    ArgKind::Float => OscType::Float(text.trim().parse().map_err(|_| format!("\"{text}\" is not a number"))?),
                    ArgKind::Str => {
                        if text.len() > 200 || text.chars().any(|c| c.is_control()) {
                            return Err("the value is too long or has control characters".into());
                        }
                        OscType::String(text)
                    }
                });
            }
            Ok(vec![pack(&address, args)?])
        })())
    }

    /// What a message heard from the tool means as a cue: (action kind, value, the rule). None if no capture rule fits.
    /// The value is checked the way a typed one is: it must be one this profile would send back to the tool.
    pub fn capture(&self, address: &str) -> Option<(String, String, &CaptureRule)> {
        for c in &self.capture {
            if let Some(m) = c.re.captures(address) {
                let caps: Vec<String> = m.iter().map(|g| g.map(|g| g.as_str().to_string()).unwrap_or_default()).collect();
                let value = render(&c.value, &caps).ok()?;
                // a captured cue must be something this profile can send, or it would be a cue that fails on the night
                let probe = Action::new(&c.kind, serde_json::json!({"value": value}));
                if matches!(self.encode(&probe), Some(Ok(_))) {
                    return Some((c.kind.clone(), value, c));
                }
            }
        }
        None
    }

    /// Which rule would send `action`, for the Doors check: how well-founded the message is. None if no rule applies.
    pub fn rule_for(&self, action: &Action) -> Option<&Rule> {
        self.pick(action).ok().flatten().map(|(r, _)| r)
    }

    /// The address a rule sends this action to, without the arguments: for messages.
    pub fn address_for(&self, action: &Action) -> Option<String> {
        let (r, caps) = self.pick(action).ok().flatten()?;
        render(&r.send, &caps).ok()
    }
}

// ---- the registry: built-in profiles, plus any loaded from a folder ----

fn registry() -> &'static RwLock<HashMap<String, Arc<Profile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<Profile>>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut m = HashMap::new();
        for text in BUILT_IN {
            let p = Profile::from_json(text, true).expect("a built-in profile file is valid (a test checks them all)");
            m.insert(p.id.clone(), Arc::new(p));
        }
        RwLock::new(m)
    })
}

/// The profile with this id, if there is one.
pub fn get(id: &str) -> Option<Arc<Profile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn exists(id: &str) -> bool {
    registry().read().unwrap().contains_key(id)
}

/// Every profile, built-in first then by id.
pub fn list() -> Vec<Arc<Profile>> {
    let mut v: Vec<Arc<Profile>> = registry().read().unwrap().values().cloned().collect();
    v.sort_by(|a, b| b.built_in.cmp(&a.built_in).then_with(|| a.id.cmp(&b.id)));
    v
}

/// The department a profile's devices serve, for a device that names it.
pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

/// Add the profile files (`*.json`, each one profile or a list of them) in `dir`. A file with a built-in's id replaces it.
/// Returns one line per profile loaded. Any bad file stops the whole load with the file's name: a typo must not silently
/// leave a tool on its old mapping.
pub fn load_dir(dir: &Path) -> Result<Vec<String>, String> {
    let mut files: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let mut loaded: Vec<Profile> = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{}: not valid JSON: {e}", f.display()))?;
        let items: Vec<Value> = if let Value::Array(a) = v { a } else { vec![v] };
        for item in items {
            loaded.push(Profile::from_json(&item.to_string(), false).map_err(|e| format!("{}: {e}", f.display()))?);
        }
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.get(&p.id).map(|o| o.built_in);
        lines.push(format!("{} ({}){}", p.name, p.id, match replaced { Some(true) => ", replaces the built-in profile", Some(false) => ", replaces an earlier one", None => "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

/// What the profile of `device_profile` makes of a message heard from the tool, as a cue.
pub fn capture(device_profile: &str, address: &str) -> Option<(String, String, Basis)> {
    let p = get(device_profile)?;
    p.capture(address).map(|(k, v, r)| (k, v, r.basis))
}

/// Every profile and what each rule rests on, for `GET /profiles` and the docs.
pub fn summary() -> Value {
    Value::Array(
        list()
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id, "name": p.name, "dept": p.dept, "built_in": p.built_in, "notes": p.notes, "hint": p.hint,
                    "kinds": p.kinds, "probe": p.probe.as_ref().map(|m| m.address.clone()), "stop": p.stop.iter().map(|m| m.address.clone()).collect::<Vec<_>>(),
                    "capture": p.capture.iter().map(|c| serde_json::json!({"listen": c.pattern, "kind": c.kind, "value": c.value, "basis": c.basis.word(), "source": c.source})).collect::<Vec<_>>(),
                    "rules": p.rules.iter().map(|r| serde_json::json!({"match": r.pattern, "send": r.send, "basis": r.basis.word(), "source": r.source})).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// The messages for `action` on a device whose profile is `profile`; `None` when there is no such profile (the generic
/// `/pchi/...` message is used then). Errors say what to fix.
pub fn encode(profile: &str, action: &Action) -> Option<Result<Vec<Vec<u8>>, String>> {
    get(profile)?.encode(action)
}

/// A harmless message that makes this kind of tool answer, so the runtime can tell it is listening.
/// Only where the vendor's own material shows a reply: the X32 answers `/info`.
pub fn probe(profile: &str) -> Option<Vec<u8>> {
    let p = get(profile)?;
    let m = p.probe.as_ref()?;
    pack(&m.address, m.args.iter().filter_map(|a| literal(a)).collect()).ok()
}

/// What a hard stop sends to a device of this profile, so the output actually goes dark.
/// Empty when no safe message is known (the tool's own stop must be used).
pub fn stop_messages(profile: &str) -> Vec<Vec<u8>> {
    let Some(p) = get(profile) else { return Vec::new() };
    p.stop.iter().filter_map(|m| pack(&m.address, m.args.iter().filter_map(|a| literal(a)).collect()).ok()).collect()
}

/// An argument with no placeholders (probe and stop messages are fixed).
fn literal(a: &ArgSpec) -> Option<OscType> {
    Some(match a.kind {
        ArgKind::Int => OscType::Int(a.template.trim().parse().ok()?),
        ArgKind::Float => OscType::Float(a.template.trim().parse().ok()?),
        ArgKind::Str => OscType::String(a.template.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(t: &str, kind: &str, payload: Value) -> Result<(String, Vec<OscType>), String> {
        let r = encode(t, &Action::new(kind, payload)).expect("has a profile")?;
        assert_eq!(r.len(), 1);
        match rosc::decoder::decode_udp(&r[0]).unwrap().1 {
            OscPacket::Message(m) => Ok((m.addr, m.args)),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn only_the_listed_tools_have_profiles() {
        for t in PROFILES {
            assert!(encode(t, &Action::new("x", json!({}))).is_some());
        }
        assert!(encode("laser", &Action::new("laser.cue", json!({"cue": "a"}))).is_none());
        assert!(encode("notch", &Action::new("scene.activate", json!({"scene": "a"}))).is_none());
    }

    #[test]
    fn resolume_columns_clips_and_learned_addresses() {
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "3"})).unwrap(), ("/composition/columns/3/connect".into(), vec![OscType::Int(1)]));
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "2/5"})).unwrap(), ("/composition/layers/2/clips/5/connect".into(), vec![OscType::Int(1)]));
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "/composition/layers/1/clips/1/connect"})).unwrap().0, "/composition/layers/1/clips/1/connect");
        assert!(one("resolume", "scene.activate", json!({"scene": "tunnel"})).unwrap_err().contains("column number"));
        assert!(one("resolume", "scene.activate", json!({"scene": "/bad address"})).is_err());
        assert!(one("resolume", "lighting.look", json!({"look": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn grandma3_sends_playback_commands_only() {
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "402"})).unwrap(), ("/cmd".into(), vec![OscType::String("Go+ Exec 402".into())]));
        assert_eq!(one("grandma3", "lighting.look", json!({"look": 12})).unwrap().1, vec![OscType::String("Go+ Exec 12".into())]);
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "Goto Cue 5"})).unwrap().1, vec![OscType::String("Goto Cue 5".into())]);
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "go+ sequence 1"})).unwrap().1, vec![OscType::String("go+ sequence 1".into())]);
        for bad in ["Delete Cue 1", "Store Cue 1", "Clear", "Go+ Exec 1; Delete Cue 1", "Go+ Exec 1\nDelete Cue 1", "blue"] {
            assert!(one("grandma3", "lighting.look", json!({"look": bad})).unwrap_err().contains("only playback commands"), "{bad}");
        }
    }

    #[test]
    fn eos_fires_cues_with_or_without_a_cuelist() {
        assert_eq!(one("eos", "lighting.look", json!({"look": "2"})).unwrap(), ("/eos/cue/2/fire".into(), vec![]));
        assert_eq!(one("eos", "lighting.look", json!({"look": "2.5"})).unwrap().0, "/eos/cue/2.5/fire");
        assert_eq!(one("eos", "lighting.look", json!({"look": "3/9"})).unwrap().0, "/eos/cue/3/9/fire");
        assert!(one("eos", "lighting.look", json!({"look": "warm"})).unwrap_err().contains("cue number"));
    }

    #[test]
    fn x32_recalls_a_scene_by_number() {
        assert_eq!(one("x32", "audio.snapshot", json!({"snapshot": "12"})).unwrap(), ("/-action/goscene".into(), vec![OscType::Int(12)]));
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "100"})).is_err());
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "intro"})).is_err());
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "-1"})).is_err());
    }

    #[test]
    fn qlab_starts_cues_by_number_and_takes_the_workspace_commands() {
        assert_eq!(one("qlab", "audio.snapshot", json!({"snapshot": "12"})).unwrap(), ("/cue/12/start".into(), vec![]));
        assert_eq!(one("qlab", "audio.scene", json!({"scene": "12.5"})).unwrap().0, "/cue/12.5/start");
        assert_eq!(one("qlab", "scene.activate", json!({"scene": "A1"})).unwrap().0, "/cue/A1/start");
        for w in ["stop", "panic", "pause", "resume"] {
            assert_eq!(one("qlab", "audio.snapshot", json!({"snapshot": w.to_uppercase()})).unwrap(), (format!("/{w}"), vec![]));
        }
        assert!(one("qlab", "audio.snapshot", json!({"snapshot": "bad cue/../x"})).is_err());
        assert!(one("qlab", "lighting.look", json!({"look": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn magicq_goes_jumps_and_releases_playbacks() {
        assert_eq!(one("magicq", "lighting.look", json!({"look": "3"})).unwrap(), ("/pb/3/go".into(), vec![]));
        assert_eq!(one("magicq", "lighting.look", json!({"look": "3/5"})).unwrap().0, "/pb/3/5");
        assert_eq!(one("magicq", "lighting.look", json!({"look": "Release 3"})).unwrap().0, "/pb/3/release");
        assert!(one("magicq", "lighting.look", json!({"look": "warm"})).unwrap_err().contains("playback number"));
        assert!(one("magicq", "lighting.look", json!({"look": "release x"})).is_err());
        assert!(one("magicq", "audio.snapshot", json!({"snapshot": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn beyond_starts_and_stops_cues_and_blacks_out() {
        assert_eq!(one("beyond", "laser.cue", json!({"cue": "sweep"})).unwrap(), ("/beyond/general/StartCue".into(), vec![OscType::String("sweep".into())]));
        assert_eq!(one("beyond", "laser.stop", json!({"cue": "sweep"})).unwrap().0, "/beyond/general/StopCue");
        assert_eq!(one("beyond", "laser.blackout", json!({})).unwrap(), ("/beyond/general/BlackOut".into(), vec![]));
        assert!(one("beyond", "laser.cue", json!({"cue": ""})).unwrap_err().contains("no value"));
        assert!(one("beyond", "audio.snapshot", json!({"snapshot": "1"})).unwrap_err().contains("no mapping"));
        let stop = stop_messages("beyond");
        assert_eq!(stop.len(), 2);
        assert!(stop_messages("laser").is_empty());
    }

    // ---- profiles are data: loading, checking, and what a bad one cannot do ----

    const ACME: &str = r#"{
        "id": "acme-desk", "name": "Acme Desk", "dept": "lighting", "kinds": ["lighting.look"],
        "hint": "go N (a playback number)",
        "rules": [
            {"match": "(?i)go (\\d{1,3})", "send": "/acme/go/{1}", "args": [{"int": "{1}"}, {"string": "go"}], "basis": "vendor", "source": "the Acme manual"},
            {"match": "(?i)off", "send": "/acme/off", "basis": "community"}
        ],
        "stop": [{"address": "/acme/off"}]
    }"#;

    #[test]
    fn every_built_in_profile_is_valid_and_says_what_it_rests_on() {
        let all = list();
        for id in PROFILES {
            let p = all.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("{id} is missing"));
            assert!(p.built_in && !p.rules.is_empty() && !p.name.is_empty(), "{id}");
            assert!(p.rules.iter().all(|r| !r.source.is_empty()), "{id}: every rule says where it comes from");
        }
        // the ones the docs call unconfirmed are marked so
        let basis = |id: &str| get(id).unwrap().rules.iter().map(|r| r.basis).collect::<Vec<_>>();
        assert!(basis("x32").iter().all(|b| *b == Basis::Memory));
        assert!(basis("eos").iter().all(|b| *b == Basis::Vendor));
        assert!(basis("magicq").iter().all(|b| *b == Basis::Community));
    }

    #[test]
    fn a_profile_file_is_checked_and_says_what_is_wrong() {
        let bad = |t: &str| Profile::from_json(t, false).err().unwrap_or_default();
        let with = |rule: &str| format!(r#"{{"id":"t","name":"T","dept":"audio","kinds":["audio.snapshot"],"rules":[{rule}]}}"#);
        assert!(bad(&ACME.replace("acme-desk", "Bad Id")).contains("id 'Bad Id'"));
        assert!(bad(&ACME.replace("lighting\"", "lights\"")).contains("dept must be"));
        assert!(bad(&ACME.replace("[\"lighting.look\"]", "[]")).contains("kinds is empty"));
        assert!(bad(&ACME.replace("[\"lighting.look\"]", "[\"light ing\"]")).contains("not like lighting.look"));
        assert!(bad(&with(r#"{"match":"(","send":"/a"}"#)).contains("pattern"));
        assert!(bad(&with(r#"{"match":"\\d+","send":"/a/{2}"}"#)).contains("does not capture"));
        assert!(bad(&with(r#"{"match":"\\d+","send":"a/b"}"#)).contains("must be an OSC address"));
        assert!(bad(&with(r#"{"send":"/a"}"#)).contains("needs a \"match\""));
        assert!(bad(&with(r#"{"match":"\\d+","send":"/a","basis":"hunch"}"#)).contains("basis 'hunch'"));
        assert!(bad(&with(r#"{"match":"\\d+","send":"/a","args":[{"int":1,"float":2}]}"#)).contains("one of"));
        assert!(bad(&with(r#"{"match":"\\d+","send":"/a","args":[{"bool":1}]}"#)).contains("not int, float or string"));
        assert!(bad(&with(r#"{"match":"\\d+","send":"/a","kinds":["lighting.look"]}"#)).contains("not one of the profile's kinds"));
        assert!(bad(&format!("{}", with(r#"{"match":"\\d+","send":"/a"}"#).replace("\"rules\"", "\"probe\":{\"address\":\"/bad address\"},\"rules\""))).contains("not a plain OSC address"));
        assert!(bad("{").contains("not a profile file"));
        assert!(Profile::from_json(ACME, false).is_ok());
    }

    #[test]
    fn a_crews_own_profile_is_loaded_from_a_folder_and_used() {
        let dir = std::env::temp_dir().join(format!("pchi-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("acme.json"), ACME).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a profile").unwrap();
        let lines = load_dir(&dir).unwrap();
        assert_eq!(lines, vec!["Acme Desk (acme-desk)".to_string()]);
        let got = |v: &str| one("acme-desk", "lighting.look", json!({"look": v}));
        assert_eq!(got("Go 12").unwrap(), ("/acme/go/12".into(), vec![OscType::Int(12), OscType::String("go".into())]));
        assert_eq!(got("OFF").unwrap().0, "/acme/off");
        let e = got("fade up").unwrap_err();
        assert!(e.contains("fade up") && e.contains("go N (a playback number)"), "{e}");
        assert!(one("acme-desk", "audio.snapshot", json!({"snapshot": "1"})).unwrap_err().contains("no mapping"));
        assert_eq!(stop_messages("acme-desk").len(), 1);
        assert_eq!(dept_of("acme-desk").as_deref(), Some("lighting"));
        assert!(!get("acme-desk").unwrap().built_in);
        // loading it again replaces it, and says so
        assert_eq!(load_dir(&dir).unwrap(), vec!["Acme Desk (acme-desk), replaces an earlier one".to_string()]);
        // one bad file stops the whole load, names the file, and changes nothing
        std::fs::write(dir.join("broken.json"), r#"{"id":"broken","name":"B","dept":"audio","kinds":["audio.snapshot"],"rules":[{"match":"(","send":"/a"}]}"#).unwrap();
        let err = load_dir(&dir).unwrap_err();
        assert!(err.contains("broken.json") && err.contains("pattern"), "{err}");
        assert!(!exists("broken"));
        // a list of profiles in one file works too
        std::fs::remove_file(dir.join("broken.json")).unwrap();
        std::fs::write(dir.join("two.json"), format!("[{}, {}]", ACME.replace("acme-desk", "acme-two"), ACME.replace("acme-desk", "acme-three"))).unwrap();
        assert!(load_dir(&dir).unwrap().iter().any(|l| l.contains("acme-three")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_cannot_send_what_the_format_forbids() {
        let p = Profile::from_json(r#"{"id":"loose","name":"Loose","dept":"audio","kinds":["audio.snapshot"],"rules":[
            {"match":".+","send":"/x/{0}","basis":"memory"},
            {"match":".+","send":"/y","args":[{"string":"{0}"}]}]}"#, false).unwrap();
        let run = |v: &str| p.encode(&Action::new("audio.snapshot", json!({"v": v}))).unwrap();
        assert!(run("a b").unwrap_err().contains("not a usable OSC address"), "a value cannot put a space or odd character in an address");
        assert!(run("a/../b").is_ok(), "dots and slashes are fine in an address");
        // the first rule takes every value, so the second never runs: the address check is the guard
        let q = Profile::from_json(r#"{"id":"loose2","name":"Loose","dept":"audio","kinds":["audio.snapshot"],"rules":[
            {"match":".+","send":"/y","args":[{"string":"{0}"}]}]}"#, false).unwrap();
        assert!(q.encode(&Action::new("audio.snapshot", json!({"v": "bell\u{7}"}))).unwrap().unwrap_err().contains("control characters"));
        assert!(q.encode(&Action::new("audio.snapshot", json!({"v": "x".repeat(250)}))).unwrap().unwrap_err().contains("too long"));
        // an int argument must be a whole number
        let r = Profile::from_json(r#"{"id":"intsonly","name":"I","dept":"audio","kinds":["audio.snapshot"],"rules":[{"match":"\\w+","send":"/n","args":[{"int":"{0}"}]}]}"#, false).unwrap();
        assert!(r.encode(&Action::new("audio.snapshot", json!({"v": "abc"}))).unwrap().unwrap_err().contains("not a whole number"));
        // a value is matched whole: a good part inside a longer value does not count
        let eos = get("eos").unwrap();
        assert!(eos.encode(&Action::new("lighting.look", json!({"look": "2; delete everything"}))).unwrap().is_err());
    }

    // ---- capture: what a tool's own messages mean as cues ----

    #[test]
    fn a_cue_the_console_fires_becomes_the_cue_that_would_fire_the_same_thing() {
        let eos = get("eos").unwrap();
        let cap = |a: &str| eos.capture(a).map(|(k, v, r)| (k, v, r.basis));
        assert_eq!(cap("/eos/out/event/cue/3/9/fire"), Some(("lighting.look".into(), "3/9".into(), Basis::Community)));
        assert_eq!(cap("/eos/out/event/cue/1/2.5/fire").unwrap().1, "1/2.5");
        assert_eq!(cap("/eos/out/event/cue/-1/0/stop"), None, "a stop is not a cue");
        assert_eq!(cap("/eos/out/event/cue/1/2/fire/extra"), None, "the whole address must fit");
        assert_eq!(cap("/eos/cue/3/9/fire"), None, "that is the address Eos listens on, not one it sends");
        // and the captured value goes straight back out as the same message
        let (kind, value, _) = eos.capture("/eos/out/event/cue/3/9/fire").unwrap();
        assert_eq!(one("eos", &kind, json!({"look": value})).unwrap().0, "/eos/cue/3/9/fire");
        assert_eq!(capture("eos", "/eos/out/event/cue/7/1/fire").unwrap().1, "7/1");
        assert!(capture("resolume", "/anything").is_none() && capture("nosuch", "/x").is_none());
    }

    #[test]
    fn a_capture_rule_is_checked_like_everything_else_in_a_profile() {
        let with = |cap: &str| format!(r#"{{"id":"c","name":"C","dept":"lighting","kinds":["lighting.look"],"rules":[{{"match":"\\d+","send":"/go/{{0}}"}}],"capture":[{cap}]}}"#);
        let bad = |c: &str| Profile::from_json(&with(c), false).err().unwrap_or_default();
        assert!(Profile::from_json(&with(r#"{"listen":"/out/(\\d+)","kind":"lighting.look","value":"{1}"}"#), false).is_ok());
        assert!(bad(r#"{"listen":"(","kind":"lighting.look","value":"{0}"}"#).contains("listen"));
        assert!(bad(r#"{"listen":"/out/(\\d+)","kind":"lighting.look","value":"{2}"}"#).contains("does not capture"));
        assert!(bad(r#"{"listen":"/out/(\\d+)","kind":"audio.snapshot","value":"{1}"}"#).contains("not one of the profile's kinds"));
        // a capture of something the profile cannot send back is dropped, not offered as a cue that would fail on the night
        let p = Profile::from_json(&with(r#"{"listen":"/out/(\\w+)","kind":"lighting.look","value":"{1}"}"#), false).unwrap();
        assert!(p.capture("/out/12").is_some());
        assert!(p.capture("/out/warm").is_none(), "the profile's rules only take numbers");
    }
}
