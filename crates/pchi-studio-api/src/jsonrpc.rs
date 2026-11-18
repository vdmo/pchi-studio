//! JSON-RPC 2.0 over TCP: a cue becomes one request to a device that speaks it, such as Q-SYS (its QRC control protocol).
//!
//! Like the HTTP output, the requests come from a **closed list in a profile file** (`"transport": "jsonrpc-tcp"`): for each cue kind
//! a fixed method and a parameter template. A cue supplies only a value, checked against the action's pattern. The request is built as
//! JSON data, not by gluing text, so a value cannot add a field, a method or a second message.
//!
//! In the parameter template a string that is exactly `{k}` is replaced by the text of part *k* of the value (`{0}` is all of it), and
//! one that is exactly `{k:number}` by that part as a number. Any other string with a brace in it is refused when the profile loads.
//!
//! One connection per cue: connect, send the request and its terminator, read one reply, close. A reply with a `result` is `accepted`; a
//! reply with an `error` is `refused` with the device's own message. No login: a device that needs a password is not reachable
//! through this yet, and the profile cannot carry one. Laser and motion profiles are refused, as for HTTP.

use crate::rig::Device;
use crate::textlink::{LinkError, Outcome};
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONNECT: Duration = Duration::from_secs(2);
const REPLY: Duration = Duration::from_secs(3);
const REPLY_MAX: usize = 16 * 1024;
const VALUE_MAX: usize = 80;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    value: Option<String>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
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
    #[serde(default)]
    port: Option<u16>,
    /// `nul` (a zero byte, as Q-SYS uses), `lf` or `crlf`.
    terminator: String,
    #[serde(default)]
    hint: String,
    #[serde(default)]
    notes: String,
    kinds: Vec<String>,
    actions: HashMap<String, RawAction>,
}

#[derive(Debug)]
pub struct RpcAction {
    pub value: Option<Regex>,
    pub method: String,
    pub params: Option<Value>,
    pub basis: String,
    pub source: String,
}

#[derive(Debug)]
pub struct RpcProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub port: Option<u16>,
    pub terminator: &'static [u8],
    pub hint: String,
    pub notes: String,
    pub kinds: Vec<String>,
    pub actions: HashMap<String, RpcAction>,
    pub built_in: bool,
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

/// A placeholder string: `{k}` or `{k:number}`.
fn placeholder(s: &str) -> Option<(usize, bool)> {
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;
    let (k, num) = match inner.split_once(':') {
        Some((k, "number")) => (k, true),
        Some(_) => return None,
        None => (inner, false),
    };
    (k.len() == 1 && k.as_bytes()[0].is_ascii_digit()).then(|| ((k.as_bytes()[0] - b'0') as usize, num))
}

/// Every placeholder in a template, or why the template is not allowed.
fn walk(v: &Value, out: &mut Vec<usize>, depth: usize) -> Result<(), String> {
    if depth > 4 {
        return Err("params nest more than four deep".into());
    }
    match v {
        Value::String(s) => {
            if s.contains('{') || s.contains('}') {
                match placeholder(s) {
                    Some((k, _)) => out.push(k),
                    None => return Err(format!("'{s}': a string with a brace must be exactly {{k}} or {{k:number}}")),
                }
            }
        }
        Value::Array(a) => a.iter().try_for_each(|x| walk(x, out, depth + 1))?,
        Value::Object(m) => m.values().try_for_each(|x| walk(x, out, depth + 1))?,
        _ => {}
    }
    Ok(())
}

impl RpcProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<RpcProfile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a valid JSON-RPC profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("profile id '{id}' must be letters, digits, - or _ (up to 40)"));
        }
        if !matches!(raw.dept.as_str(), "visual" | "lighting" | "audio") {
            return Err(format!("{id}: dept '{}' is not one of visual, lighting, audio (a laser or motion device needs an acknowledgement and a stop, which this does not give)", raw.dept));
        }
        let terminator: &'static [u8] = match raw.terminator.as_str() {
            "nul" => b"\0",
            "lf" => b"\n",
            "crlf" => b"\r\n",
            t => return Err(format!("{id}: terminator '{t}' is not nul, lf or crlf")),
        };
        if raw.actions.is_empty() {
            return Err(format!("{id}: no actions"));
        }
        let mut actions = HashMap::new();
        for (kind, a) in raw.actions {
            let what = format!("{id} {kind}");
            if a.method.is_empty() || a.method.len() > 60 || !a.method.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
                return Err(format!("{what}: a method is letters, digits, . and _ (up to 60)"));
            }
            if a.method.starts_with("rpc.") {
                return Err(format!("{what}: methods starting rpc. are reserved by JSON-RPC"));
            }
            let value = a.value.as_deref().map(full).transpose().map_err(|e| format!("{what}: {e}"))?;
            let mut used = Vec::new();
            if let Some(p) = &a.params {
                if !p.is_object() && !p.is_array() {
                    return Err(format!("{what}: params is an object or an array"));
                }
                walk(p, &mut used, 0).map_err(|e| format!("{what}: {e}"))?;
            }
            if !used.is_empty() && value.is_none() {
                return Err(format!("{what}: placeholders need a value pattern"));
            }
            if let Some(re) = &value {
                if let Some(k) = used.iter().find(|k| **k > 0 && **k >= re.captures_len()) {
                    return Err(format!("{what}: {{{k}}} but the value pattern has no bracketed part {k}"));
                }
            }
            if !matches!(a.basis.as_str(), "vendor" | "community" | "memory") {
                return Err(format!("{what}: basis '{}' is not vendor, community or memory (nothing here may call itself proven)", a.basis));
            }
            actions.insert(kind, RpcAction { value, method: a.method, params: a.params, basis: a.basis, source: a.source });
        }
        for k in &raw.kinds {
            if !actions.contains_key(k) {
                return Err(format!("{id}: kind '{k}' has no action"));
            }
        }
        Ok(RpcProfile { id, name: raw.name, dept: raw.dept, port: raw.port, terminator, hint: raw.hint, notes: raw.notes, kinds: raw.kinds, actions, built_in })
    }
}

const BUILT_IN: [&str; 1] = [include_str!("../profiles/jsonrpc/qsys.json")];

fn registry() -> &'static RwLock<HashMap<String, Arc<RpcProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<RpcProfile>>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut m = HashMap::new();
        for text in BUILT_IN {
            let p = RpcProfile::from_json(text, true).expect("a built-in JSON-RPC profile is valid (a test checks them all)");
            m.insert(p.id.clone(), Arc::new(p));
        }
        RwLock::new(m)
    })
}

pub fn get(id: &str) -> Option<Arc<RpcProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

/// Load the JSON-RPC profiles among a folder's files; `profiles::load_dir` leaves those to this.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(RpcProfile::from_json(&item.to_string(), false).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.get(&p.id).map(|o| o.built_in);
        lines.push(format!("{} ({}), a JSON-RPC device{}", p.name, p.id, match replaced { Some(true) => ", replaces the built-in profile", Some(false) => ", replaces an earlier one", None => "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

/// Whether a file's JSON is a JSON-RPC profile (so the other loaders leave it alone).
pub fn is_rpc_json(v: &Value) -> bool {
    v.get("transport").and_then(Value::as_str) == Some("jsonrpc-tcp")
}

fn fill(t: &Value, whole: &str, caps: &[String]) -> Result<Value, String> {
    Ok(match t {
        Value::String(s) => match placeholder(s) {
            Some((k, num)) => {
                let v = if k == 0 { whole } else { caps.get(k - 1).map(String::as_str).unwrap_or("") };
                if num {
                    let n: f64 = v.parse().map_err(|_| format!("'{v}' is not a number"))?;
                    serde_json::Number::from_f64(n).map(Value::Number).ok_or_else(|| format!("'{v}' is not a finite number"))?
                } else {
                    Value::String(v.to_string())
                }
            }
            None => t.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| fill(x, whole, caps)).collect::<Result<_, _>>()?),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| fill(x, whole, caps).map(|v| (k.clone(), v))).collect::<Result<Map<_, _>, _>>()?),
        other => other.clone(),
    })
}

/// The request a cue makes, as JSON, with the given id. Errors say what is wrong with the cue.
pub fn request_for(p: &RpcProfile, action: &Action, id: u64) -> Result<Value, String> {
    let a = p.actions.get(&action.kind).ok_or_else(|| format!("{} does not take a '{}' cue (it takes {})", p.name, action.kind, p.kinds.join(", ")))?;
    let v = crate::textlink::value_of(action);
    if v.chars().any(|c| c.is_control()) || v.len() > VALUE_MAX {
        return Err(format!("the value for {} is up to {VALUE_MAX} plain characters", p.name));
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
    let mut req = json!({"jsonrpc": "2.0", "method": a.method, "id": id});
    if let Some(t) = &a.params {
        req["params"] = fill(t, &v, &caps).map_err(|e| format!("{}: {e}", action.kind))?;
    }
    Ok(req)
}

fn profile_of(dev: &Device) -> Result<Arc<RpcProfile>, String> {
    get(dev.profile_key()).ok_or_else(|| format!("{} has no JSON-RPC profile", dev.label))
}

/// Check a cue for a JSON-RPC device. Sends nothing.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    request_for(&*profile_of(dev)?, action, 1).map(|_| ())
}

/// What sending the cue puts on the wire, in words.
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let p = profile_of(dev)?;
    let r = request_for(&p, action, 1)?;
    Ok(vec![(format!("{} JSON-RPC: {}", p.name, r), vec![])])
}

fn target_of(dev: &Device, p: &RpcProfile) -> Result<String, String> {
    let host = dev.host.as_deref().map(str::trim).unwrap_or("");
    if host.is_empty() {
        return Err(format!("{} has no host in the rig", dev.label));
    }
    if host.len() > 100 || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) {
        return Err(format!("{}: the host '{host}' is not a plain address or name", dev.label));
    }
    let port = dev.port.or(p.port).ok_or_else(|| format!("{} has no port in the rig", dev.label))?;
    Ok(format!("{host}:{port}"))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What a reply means: `Ok` with a note for a result, `Err(message)` for the device's error.
fn judge(reply: &str) -> Result<String, LinkError> {
    let v: Value = serde_json::from_str(reply.trim_matches(|c: char| c == '\0' || c.is_whitespace())).map_err(|_| LinkError::Failed("the device's answer is not JSON".into()))?;
    if let Some(e) = v.get("error") {
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("no message");
        let code = e.get("code").map(|c| format!(" (code {c})")).unwrap_or_default();
        return Err(LinkError::Refused(format!("the device refused it: {msg}{code}")));
    }
    if v.get("result").is_some() {
        return Ok("it answered with a result".into());
    }
    Err(LinkError::Failed("the device's answer has neither a result nor an error".into()))
}

/// Send a cue and read the answer.
pub async fn apply(dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
    let p = profile_of(dev).map_err(LinkError::Failed)?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let req = request_for(&p, action, id).map_err(LinkError::Failed)?;
    let target = target_of(dev, &p).map_err(LinkError::Failed)?;
    let mut sock = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(&target))
        .await
        .map_err(|_| LinkError::Failed(format!("{} did not answer at {target} (timed out)", dev.label)))?
        .map_err(|e| LinkError::Failed(format!("cannot reach {} at {target}: {e}", dev.label)))?;
    let mut bytes = req.to_string().into_bytes();
    bytes.extend_from_slice(p.terminator);
    let end = p.terminator;
    let go = async {
        sock.write_all(&bytes).await.map_err(|e| format!("could not send: {e}"))?;
        let mut got = Vec::new();
        let mut buf = [0u8; 1024];
        while got.len() < REPLY_MAX && !got.windows(end.len()).any(|w| w == end) {
            let n = sock.read(&mut buf).await.map_err(|e| format!("no answer: {e}"))?;
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        Ok::<_, String>(got)
    };
    let got = tokio::time::timeout(REPLY, go).await.map_err(|_| LinkError::Failed(format!("{} took the request but did not answer (timed out)", dev.label)))?.map_err(LinkError::Failed)?;
    // the first message in the answer
    let text = String::from_utf8_lossy(&got).to_string();
    let first = text.split(|c| c == '\0' || c == '\n').find(|s| !s.trim().is_empty()).unwrap_or("");
    let note = judge(first)?;
    Ok(Outcome { status: "accepted", note: format!("{}: {note}", req["method"].as_str().unwrap_or("")), target })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(actions: &str) -> String {
        format!(r#"{{"id":"x","name":"X","transport":"jsonrpc-tcp","dept":"audio","port":1710,"terminator":"nul","kinds":[],"actions":{actions}}}"#)
    }
    fn bad(actions: &str) -> String {
        RpcProfile::from_json(&profile(actions), false).err().unwrap_or_default()
    }
    fn act(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"v": v}))
    }
    fn dev(host: &str, port: Option<u16>) -> Device {
        Device { id: "q1".into(), label: "Core".into(), kind: "qsys".into(), dept: "audio".into(), host: Some(host.into()), port, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }

    #[test]
    fn the_built_in_profile_is_valid_and_says_what_it_rests_on() {
        for text in BUILT_IN {
            let p = RpcProfile::from_json(text, true).unwrap();
            assert!(!p.notes.is_empty() && !p.hint.is_empty(), "{}", p.id);
            for (k, a) in &p.actions {
                assert!(a.basis != "proven" && !a.source.is_empty(), "{} {k}", p.id);
            }
        }
        assert_eq!(dept_of("qsys").as_deref(), Some("audio"));
        assert_eq!(get("qsys").unwrap().terminator, b"\0");
    }

    #[test]
    fn q_sys_requests_are_the_documented_shape() {
        let p = get("qsys").unwrap();
        assert_eq!(request_for(&p, &act("audio.control", "Main Gain=-10.5"), 7).unwrap(), json!({"jsonrpc": "2.0", "id": 7, "method": "Control.Set", "params": {"Name": "Main Gain", "Value": -10.5}}));
        assert_eq!(request_for(&p, &act("audio.control", "Mute=1"), 1).unwrap()["params"], json!({"Name": "Mute", "Value": 1.0}));
        assert_eq!(request_for(&p, &act("audio.snapshot", "Show Open/2"), 3).unwrap(), json!({"jsonrpc": "2.0", "id": 3, "method": "Snapshot.Load", "params": {"Name": "Show Open", "Bank": 2.0}}));
        for v in ["", "Mute", "=1", "Mute=", "Mute=abc", "Mute=1=2", "Mu\"te=1", "Mute=1\u{0}", "Mute=1e5", &format!("{}=1", "a".repeat(70))] {
            assert!(request_for(&p, &act("audio.control", v), 1).is_err(), "{v:?}");
        }
        assert!(request_for(&p, &act("audio.other", "x=1"), 1).unwrap_err().contains("does not take"));
    }

    #[test]
    fn a_value_is_data_it_cannot_add_fields_or_methods() {
        let t = r#"{"k":{"value":"(.{1,40})","method":"Do.It","params":{"Name":"{1}"}}}"#;
        let p = RpcProfile::from_json(&profile(t), false).unwrap();
        let r = request_for(&p, &act("k", r#"x","Value":9},"method":"Evil"#), 1).unwrap();
        assert_eq!(r["method"], "Do.It");
        assert_eq!(r["params"], json!({"Name": r#"x","Value":9},"method":"Evil"#}), "quotes and braces stay inside the string");
        assert!(!r.to_string().contains("\n"), "one line");
    }

    #[test]
    fn a_profile_can_only_say_what_is_safe() {
        assert!(bad(r#"{"k":{"method":"a b"}}"#).contains("letters, digits"));
        assert!(bad(r#"{"k":{"method":"rpc.x"}}"#).contains("reserved"));
        assert!(bad(r#"{"k":{"method":"M","params":"x"}}"#).contains("object or an array"));
        assert!(bad(r#"{"k":{"method":"M","params":{"a":"{0}"}}}"#).contains("need a value pattern"));
        assert!(bad(r#"{"k":{"method":"M","value":"(.)","params":{"a":"x {1}"}}}"#).contains("exactly"));
        assert!(bad(r#"{"k":{"method":"M","value":"(.)","params":{"a":"{1:text}"}}}"#).contains("exactly"));
        assert!(bad(r#"{"k":{"method":"M","value":"(.)","params":{"a":"{2}"}}}"#).contains("no bracketed part 2"));
        assert!(bad(r#"{"k":{"method":"M","basis":"proven"}}"#).contains("proven"));
        assert!(bad(r#"{"k":{"method":"M","extra":1}}"#).contains("unknown field"));
        assert!(bad("{}").contains("no actions"));
        assert!(RpcProfile::from_json(&profile(r#"{"k":{"method":"M"}}"#).replace("audio", "laser"), false).unwrap_err().contains("laser or motion"));
        assert!(RpcProfile::from_json(&profile(r#"{"k":{"method":"M"}}"#).replace("nul", "tab"), false).unwrap_err().contains("terminator"));
        assert!(RpcProfile::from_json(&profile(r#"{"k":{"method":"M"}}"#).replace(r#""kinds":[]"#, r#""kinds":["z"]"#), false).unwrap_err().contains("no action"));
    }

    #[test]
    fn a_cue_is_checked_and_described_without_sending() {
        let d = dev("10.0.0.9", None);
        assert!(check_action(&d, &act("audio.control", "Gain=0")).is_ok());
        assert!(check_action(&d, &act("audio.control", "Gain")).is_err());
        let l = describe(&d, &act("audio.control", "Gain=-3")).unwrap();
        assert!(l[0].0.starts_with("Q-SYS Core (QRC) JSON-RPC: {") && l[0].0.contains("Control.Set"), "{}", l[0].0);
        assert_eq!(target_of(&d, &get("qsys").unwrap()).unwrap(), "10.0.0.9:1710");
        assert!(target_of(&dev("", None), &get("qsys").unwrap()).is_err());
        assert!(target_of(&dev("a b", None), &get("qsys").unwrap()).is_err());
    }

    #[test]
    fn replies_are_judged_by_result_and_error() {
        assert!(judge(r#"{"jsonrpc":"2.0","result":true,"id":1}"#).is_ok());
        let e = judge(r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid params"},"id":1}"#).unwrap_err();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("Invalid params") && e.text().contains("-32602"));
        assert_eq!(judge("hello").unwrap_err().status(), "failed");
        assert_eq!(judge(r#"{"jsonrpc":"2.0","id":1}"#).unwrap_err().status(), "failed");
    }

    /// A stand-in device: reads one request up to the zero byte, answers with `reply` and a zero byte.
    async fn standin(reply: &'static str) -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            while !got.contains(&0) {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(reply.as_bytes()).await.unwrap();
            s.write_all(&[0]).await.unwrap();
            got
        });
        (port, h)
    }

    #[tokio::test]
    async fn a_stand_in_device_receives_one_request_and_its_answer_decides_the_outcome() {
        let (port, got) = standin(r#"{"jsonrpc":"2.0","result":true,"id":1}"#).await;
        let o = apply(&dev("127.0.0.1", Some(port)), &act("audio.control", "Main=-6")).await.unwrap();
        assert_eq!(o.status, "accepted");
        let bytes = got.await.unwrap();
        assert_eq!(*bytes.last().unwrap(), 0, "the request ends with a zero byte");
        let v: Value = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!((v["method"].as_str(), &v["params"]), (Some("Control.Set"), &json!({"Name": "Main", "Value": -6.0})));
        assert_eq!(bytes.iter().filter(|b| **b == 0).count(), 1, "exactly one message");

        let (port, _) = standin(r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Control not found"},"id":1}"#).await;
        let e = apply(&dev("127.0.0.1", Some(port)), &act("audio.control", "Nope=1")).await.unwrap_err();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("Control not found"));

        let (port, _) = standin("not json").await;
        assert_eq!(apply(&dev("127.0.0.1", Some(port)), &act("audio.control", "A=1")).await.unwrap_err().status(), "failed");

        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(apply(&dev("127.0.0.1", Some(dead)), &act("audio.control", "A=1")).await.unwrap_err().status(), "failed");
        assert!(apply(&dev("127.0.0.1", Some(dead)), &act("audio.control", "bad")).await.unwrap_err().text().contains("not a value"));
    }
}
