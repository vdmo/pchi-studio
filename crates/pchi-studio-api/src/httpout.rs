//! HTTP output: a cue becomes one web request to a device, from a **closed list of request templates** in a profile file.
//!
//! For Bitfocus Companion's button API, cameras and media servers with a web interface, and webhooks. A profile (`transport: "http"`)
//! lists, per cue kind, a method, a path and an optional body. A cue supplies only a *value*, which is checked against the action's
//! pattern and then percent-encoded into the path or escaped into the body. **A cue cannot choose the host, the path, the method or
//! the headers**, so it cannot make PCHI call an address it was not set up for.
//!
//! What this does and does not do:
//!
//! - Plain `http://` only. There is no TLS (a device on a show network does not usually have a certificate), so nothing goes to a
//!   public web service. HTTPS is not supported.
//! - Methods `GET`, `POST` and `PUT`. No `DELETE`, no `PATCH`. Redirects are not followed. No cookies, no authentication headers.
//! - One request per cue, a few seconds' timeout, the reply's first 8 KB read for the status line. A 2xx status is `accepted`;
//!   any other status or no answer is a failure that never stops the show.
//! - "Accepted" means the device said OK to the request, not that the light or camera did what you wanted.

use crate::rig::Device;
use crate::textlink::{LinkError, Outcome};
use pchi_showchart::Action;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONNECT: Duration = Duration::from_secs(2);
const REPLY: Duration = Duration::from_secs(4);
const HEAD_MAX: usize = 8192;
const VALUE_MAX: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    value: Option<String>,
    method: String,
    path: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
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
    #[serde(default)]
    hint: String,
    #[serde(default)]
    notes: String,
    kinds: Vec<String>,
    actions: HashMap<String, RawAction>,
}

/// One cue kind's request.
#[derive(Debug)]
pub struct HttpAction {
    pub value: Option<Regex>,
    pub method: &'static str,
    pub path: String,
    pub body: Option<String>,
    pub content_type: Option<&'static str>,
    pub basis: String,
    pub source: String,
}

#[derive(Debug)]
pub struct HttpProfile {
    pub id: String,
    pub name: String,
    pub dept: String,
    pub port: Option<u16>,
    pub hint: String,
    pub notes: String,
    pub kinds: Vec<String>,
    pub actions: HashMap<String, HttpAction>,
    pub built_in: bool,
}

fn full(p: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(&format!("^(?:{p})$")).size_limit(1 << 20).build().map_err(|e| format!("pattern '{p}': {e}"))
}

/// Placeholders are `{0}` (the whole value) and `{1}` to `{9}` (the bracketed parts of the value's pattern), and nothing else. A literal
/// brace is written twice (`{{` and `}}`), which a JSON body needs.
pub(crate) fn placeholders(t: &str) -> Result<Vec<usize>, String> {
    let mut found = Vec::new();
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' if b.get(i + 1) == Some(&b'{') => i += 2,
            b'}' if b.get(i + 1) == Some(&b'}') => i += 2,
            b'{' if b.get(i + 1).is_some_and(u8::is_ascii_digit) && b.get(i + 2) == Some(&b'}') => {
                found.push((b[i + 1] - b'0') as usize);
                i += 3;
            }
            b'{' | b'}' => return Err(format!("'{t}': the only placeholders are {{0}} (the value) and {{1}} to {{9}} (its bracketed parts); write a literal brace twice")),
            _ => i += 1,
        }
    }
    Ok(found)
}

impl HttpProfile {
    pub fn from_json(text: &str, built_in: bool) -> Result<HttpProfile, String> {
        let raw: RawProfile = serde_json::from_str(text).map_err(|e| format!("not a valid HTTP profile: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("profile id '{id}' must be letters, digits, - or _ (up to 40)"));
        }
        if !matches!(raw.dept.as_str(), "visual" | "lighting" | "laser" | "motion" | "audio") {
            return Err(format!("{id}: dept '{}' is not one of visual, lighting, laser, motion, audio", raw.dept));
        }
        if raw.dept == "laser" || raw.dept == "motion" {
            return Err(format!("{id}: an HTTP profile cannot serve the laser or motion departments: those need an acknowledgement and a stop, which a web request does not give"));
        }
        if raw.actions.is_empty() {
            return Err(format!("{id}: no actions"));
        }
        let mut actions = HashMap::new();
        for (kind, a) in raw.actions {
            let what = format!("{id} {kind}");
            let method: &'static str = match a.method.as_str() {
                "GET" => "GET",
                "POST" => "POST",
                "PUT" => "PUT",
                m => return Err(format!("{what}: method '{m}' is not GET, POST or PUT")),
            };
            if !a.path.starts_with('/') || a.path.starts_with("//") || a.path.len() > 200 {
                return Err(format!("{what}: a path starts with a single / and is up to 200 characters"));
            }
            if let Some(c) = a.path.chars().find(|c| !(c.is_ascii_alphanumeric() || "/._~-?&=:{}".contains(*c))) {
                return Err(format!("{what}: '{c}' cannot appear in a path template (letters, digits and / . _ ~ - ? & = : only, plus placeholders)"));
            }
            if a.path.contains("..") {
                return Err(format!("{what}: '..' in a path"));
            }
            let value = a.value.as_deref().map(full).transpose().map_err(|e| format!("{what}: {e}"))?;
            let mut used = placeholders(&a.path).map_err(|e| format!("{what}: {e}"))?;
            let content_type: Option<&'static str> = match (a.body.is_some(), a.content_type.as_deref()) {
                (false, None) => None,
                (false, Some(_)) => return Err(format!("{what}: a content_type without a body")),
                (true, _) if method == "GET" => return Err(format!("{what}: a GET has no body")),
                (true, Some("application/json")) => Some("application/json"),
                (true, Some("text/plain")) => Some("text/plain"),
                (true, other) => return Err(format!("{what}: a body needs content_type application/json or text/plain, not {other:?}")),
            };
            if let Some(b) = &a.body {
                if b.len() > 500 || b.chars().any(|c| c.is_control() && c != '\n') {
                    return Err(format!("{what}: body is up to 500 plain characters"));
                }
                used.extend(placeholders(b).map_err(|e| format!("{what}: {e}"))?);
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
            actions.insert(kind.clone(), HttpAction { value, method, path: a.path, body: a.body, content_type, basis: a.basis, source: a.source });
        }
        for k in &raw.kinds {
            if !actions.contains_key(k) {
                return Err(format!("{id}: kind '{k}' has no action"));
            }
        }
        Ok(HttpProfile { id, name: raw.name, dept: raw.dept, port: raw.port, hint: raw.hint, notes: raw.notes, kinds: raw.kinds, actions, built_in })
    }
}

const BUILT_IN: [&str; 1] = [include_str!("../profiles/http/companion-http.json")];

fn registry() -> &'static RwLock<HashMap<String, Arc<HttpProfile>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<HttpProfile>>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut m = HashMap::new();
        for text in BUILT_IN {
            let p = HttpProfile::from_json(text, true).expect("a built-in HTTP profile is valid (a test checks them all)");
            m.insert(p.id.clone(), Arc::new(p));
        }
        RwLock::new(m)
    })
}

pub fn get(id: &str) -> Option<Arc<HttpProfile>> {
    registry().read().unwrap().get(id).cloned()
}

pub fn dept_of(id: &str) -> Option<String> {
    get(id).map(|p| p.dept.clone())
}

pub fn list() -> Vec<Arc<HttpProfile>> {
    let mut v: Vec<_> = registry().read().unwrap().values().cloned().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Load the HTTP profiles among a folder's files; `profiles::load_dir` leaves those to this.
pub fn load_values(items: Vec<Value>, from: &str) -> Result<Vec<String>, String> {
    let mut loaded = Vec::new();
    for item in items {
        loaded.push(HttpProfile::from_json(&item.to_string(), false).map_err(|e| format!("{from}: {e}"))?);
    }
    let mut lines = Vec::new();
    let mut reg = registry().write().unwrap();
    for p in loaded {
        let replaced = reg.get(&p.id).map(|o| o.built_in);
        lines.push(format!("{} ({}), an HTTP device{}", p.name, p.id, match replaced { Some(true) => ", replaces the built-in profile", Some(false) => ", replaces an earlier one", None => "" }));
        reg.insert(p.id.clone(), Arc::new(p));
    }
    Ok(lines)
}

/// Whether a file's JSON is an HTTP profile (so the OSC loader leaves it alone).
pub fn is_http_json(v: &Value) -> bool {
    v.get("transport").and_then(Value::as_str) == Some("http")
}

/// Everything the Doors check and the Rig tab list about a profile's rules and what each rests on.
pub fn summary() -> Value {
    Value::Array(
        list()
            .iter()
            .map(|p| {
                let mut kinds: Vec<_> = p.actions.iter().map(|(k, a)| serde_json::json!({"kind": k, "method": a.method, "path": a.path, "basis": a.basis, "source": a.source})).collect();
                kinds.sort_by_key(|k| k["kind"].as_str().unwrap_or_default().to_string());
                serde_json::json!({"id": p.id, "name": p.name, "dept": p.dept, "built_in": p.built_in, "hint": p.hint, "notes": p.notes, "actions": kinds})
            })
            .collect(),
    )
}

// ---- building a request ----

/// The first value in a cue's payload, as text.
pub fn value_of(action: &Action) -> String {
    crate::textlink::value_of(action)
}

#[derive(Clone, Copy)]
pub(crate) enum Enc {
    Path,
    Json,
    Text,
}

pub(crate) fn encode(v: &str, how: Enc) -> String {
    match how {
        Enc::Path => v.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect(),
        Enc::Json => {
            let s = serde_json::to_string(v).unwrap_or_default();
            s.get(1..s.len().saturating_sub(1)).unwrap_or_default().to_string()
        }
        Enc::Text => v.to_string(),
    }
}

pub(crate) fn render(t: &str, whole: &str, caps: &[String], how: Enc) -> String {
    let mut out = String::new();
    let b = t.as_bytes();
    let (mut i, mut start) = (0, 0);
    while i < b.len() {
        let two = b.get(i + 1).copied();
        if (b[i] == b'{' && two == Some(b'{')) || (b[i] == b'}' && two == Some(b'}')) {
            out.push_str(&t[start..=i]);
            i += 2;
            start = i;
        } else if b[i] == b'{' {
            out.push_str(&t[start..i]);
            let k = (b[i + 1] - b'0') as usize;
            let v = if k == 0 { whole } else { caps.get(k - 1).map(String::as_str).unwrap_or("") };
            out.push_str(&encode(v, how));
            i += 3;
            start = i;
        } else {
            i += 1;
        }
    }
    out + &t[start..]
}

/// A request ready to send.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Option<String>,
    pub content_type: Option<&'static str>,
}

/// Turn a cue into the request the profile makes of it, or say what is wrong with the cue.
pub fn request_for(p: &HttpProfile, action: &Action) -> Result<Request, String> {
    let a = p.actions.get(&action.kind).ok_or_else(|| format!("{} does not take a '{}' cue (it takes {})", p.name, action.kind, p.kinds.join(", ")))?;
    let v = value_of(action);
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
    let how = if a.content_type == Some("application/json") { Enc::Json } else { Enc::Text };
    Ok(Request { method: a.method, path: render(&a.path, &v, &caps, Enc::Path), body: a.body.as_ref().map(|b| render(b, &v, &caps, how)), content_type: a.content_type })
}

fn profile_of(dev: &Device) -> Result<Arc<HttpProfile>, String> {
    get(dev.profile_key()).ok_or_else(|| format!("{} has no HTTP profile", dev.label))
}

/// Check a cue for an HTTP device. Sends nothing.
pub fn check_action(dev: &Device, action: &Action) -> Result<(), String> {
    request_for(&*profile_of(dev)?, action).map(|_| ())
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 100 && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
}

fn target_of(dev: &Device, p: &HttpProfile) -> Result<(String, u16), String> {
    let host = dev.host.as_deref().map(str::trim).unwrap_or("");
    if host.is_empty() {
        return Err(format!("{} has no host in the rig", dev.label));
    }
    if !host_ok(host) {
        return Err(format!("{}: the host '{host}' is not a plain address or name", dev.label));
    }
    let port = dev.port.or(p.port).ok_or_else(|| format!("{} has no port in the rig", dev.label))?;
    Ok((host.to_string(), port))
}

/// What sending the cue puts on the wire, in words (the request line, and the body if any).
pub fn describe(dev: &Device, action: &Action) -> Result<Vec<(String, Vec<Value>)>, String> {
    let p = profile_of(dev)?;
    let r = request_for(&p, action)?;
    let host = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).unwrap_or("(no host)");
    let port = dev.port.or(p.port).map(|p| format!(":{p}")).unwrap_or_default();
    let args = r.body.iter().map(|b| Value::String(b.clone())).collect();
    Ok(vec![(format!("{} HTTP: {} http://{host}{port}{}", p.name, r.method, r.path), args)])
}

fn wire(host: &str, port: u16, r: &Request) -> Vec<u8> {
    let mut s = format!("{} {} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: pchi-studio\r\nAccept: */*\r\nConnection: close\r\n", r.method, r.path);
    match &r.body {
        Some(b) => s += &format!("Content-Type: {}\r\nContent-Length: {}\r\n\r\n{b}", r.content_type.unwrap_or("text/plain"), b.len()),
        None if r.method == "GET" => s += "\r\n",
        None => s += "Content-Length: 0\r\n\r\n",
    }
    s.into_bytes()
}

fn status_of(head: &str) -> Option<u16> {
    let line = head.lines().next()?;
    let mut w = line.split_whitespace();
    if !w.next()?.starts_with("HTTP/1.") {
        return None;
    }
    w.next()?.parse().ok()
}

/// Send a cue to an HTTP device and read the status line of the answer.
pub async fn apply(dev: &Device, action: &Action) -> Result<Outcome, LinkError> {
    let p = profile_of(dev).map_err(LinkError::Failed)?;
    let r = request_for(&p, action).map_err(LinkError::Failed)?;
    let (host, port) = target_of(dev, &p).map_err(LinkError::Failed)?;
    let target = format!("{host}:{port}");
    let mut sock = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(&target))
        .await
        .map_err(|_| LinkError::Failed(format!("{} did not answer at {target} (timed out)", dev.label)))?
        .map_err(|e| LinkError::Failed(format!("cannot reach {} at {target}: {e}", dev.label)))?;
    let go = async {
        sock.write_all(&wire(&host, port, &r)).await.map_err(|e| format!("could not send: {e}"))?;
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while head.len() < HEAD_MAX && !head.windows(4).any(|w| w == b"\r\n\r\n") && !head.contains(&b'\n') {
            let n = sock.read(&mut buf).await.map_err(|e| format!("no answer: {e}"))?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        Ok::<_, String>(String::from_utf8_lossy(&head).to_string())
    };
    let head = tokio::time::timeout(REPLY, go).await.map_err(|_| LinkError::Failed(format!("{} took the request but did not answer (timed out)", dev.label)))?.map_err(LinkError::Failed)?;
    match status_of(&head) {
        Some(c) if (200..300).contains(&c) => Ok(Outcome { status: "accepted", note: format!("{} {} answered {c}", r.method, r.path), target }),
        Some(c) => Err(LinkError::Refused(format!("{} answered {c} to {} {}", dev.label, r.method, r.path))),
        None => Err(LinkError::Failed(format!("{} did not answer with an HTTP status", dev.label))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(actions: &str) -> String {
        format!(r#"{{"id":"x","name":"X","transport":"http","dept":"visual","port":8000,"kinds":[],"actions":{actions}}}"#)
    }
    fn bad(actions: &str) -> String {
        HttpProfile::from_json(&profile(actions), false).err().unwrap_or_default()
    }
    fn act(kind: &str, v: &str) -> Action {
        Action::new(kind, json!({"v": v}))
    }
    fn dev(profile: &str, host: &str, port: Option<u16>) -> Device {
        Device { id: "h1".into(), label: "Web thing".into(), kind: profile.into(), dept: "visual".into(), host: Some(host.into()), port, listen_port: None, profile: None, universe: None, patch: None, password: None, programs: None }
    }

    #[test]
    fn every_built_in_profile_is_valid_and_says_what_it_rests_on() {
        for text in BUILT_IN {
            let p = HttpProfile::from_json(text, true).unwrap();
            assert!(!p.notes.is_empty() && !p.hint.is_empty(), "{}: say what it is and what is unchecked", p.id);
            for (k, a) in &p.actions {
                assert!(a.basis != "proven", "{} {k}: nothing here has been seen working", p.id);
                assert!(!a.source.is_empty(), "{} {k}: a source is written down", p.id);
            }
        }
        assert!(get("companion-http").is_some());
        assert_eq!(dept_of("companion-http").as_deref(), Some("visual"));
    }

    #[test]
    fn companion_press_is_the_documented_path_and_values_cannot_change_it() {
        let p = get("companion-http").unwrap();
        let r = request_for(&p, &act("visual.button", "1/2/3")).unwrap();
        assert_eq!((r.method, r.path.as_str(), r.body), ("POST", "/api/location/1/2/3/press", None));
        let r = request_for(&p, &act("visual.button_down", "10/0/7")).unwrap();
        assert_eq!(r.path, "/api/location/10/0/7/down");
        for bad in ["1/2", "1/2/3/4", "a/b/c", "1/2/3?x=1", "1/2/../3", "../../admin", "1/2/3 HTTP/1.1\r\nHost: evil", "1/2/3\nPOST /x", ""] {
            assert!(request_for(&p, &act("visual.button", bad)).is_err(), "{bad:?}");
        }
        assert!(request_for(&p, &act("visual.nothing", "1/2/3")).unwrap_err().contains("does not take"));
    }

    #[test]
    fn a_profile_can_only_say_what_is_safe() {
        assert!(bad(r#"{"k":{"method":"DELETE","path":"/x"}}"#).contains("not GET, POST or PUT"));
        assert!(bad(r#"{"k":{"method":"GET","path":"http://evil/x"}}"#).contains("single /"));
        assert!(bad(r#"{"k":{"method":"GET","path":"//evil/x"}}"#).contains("single /"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a b"}}"#).contains("cannot appear"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a/../b"}}"#).contains(".."));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a/{x}","value":"1"}}"#).contains("placeholders"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a/{0}"}}"#).contains("need a value pattern"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a/{2}","value":"(\\d)"}}"#).contains("no bracketed part 2"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a","body":"x","content_type":"text/plain"}}"#).contains("GET has no body"));
        assert!(bad(r#"{"k":{"method":"POST","path":"/a","body":"x","content_type":"text/html"}}"#).contains("content_type"));
        assert!(bad(r#"{"k":{"method":"POST","path":"/a","body":"x"}}"#).contains("content_type"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a","basis":"proven"}}"#).contains("nothing here may call itself proven"));
        assert!(bad(r#"{"k":{"method":"GET","path":"/a","extra":1}}"#).contains("unknown field"));
        assert!(bad("{}").contains("no actions"));
        let laser = profile(r#"{"k":{"method":"GET","path":"/a"}}"#).replace("visual", "laser");
        assert!(HttpProfile::from_json(&laser, false).unwrap_err().contains("laser or motion"));
        assert!(HttpProfile::from_json(&profile(r#"{"k":{"method":"GET","path":"/a"}}"#).replace(r#""kinds":[]"#, r#""kinds":["z"]"#), false).unwrap_err().contains("no action"));
    }

    #[test]
    fn values_are_encoded_into_the_path_and_escaped_into_the_body() {
        let t = r#"{"cam.preset":{"value":"([A-Za-z0-9 \"]{1,20})","method":"POST","path":"/p/{1}","body":"{{\"name\":\"{0}\"}}","content_type":"application/json"},"cam.home":{"method":"GET","path":"/home"}}"#;
        let p = HttpProfile::from_json(&profile(t), false).unwrap();
        let r = request_for(&p, &act("cam.preset", "Stage \"left\"")).unwrap();
        assert_eq!(r.path, "/p/Stage%20%22left%22", "a space and a quote are encoded, so they cannot end the path");
        assert_eq!(r.body.as_deref(), Some(r#"{"name":"Stage \"left\""}"#));
        assert_eq!(r.content_type, Some("application/json"));
        assert_eq!(request_for(&p, &act("cam.home", "")).unwrap().path, "/home");
        assert!(request_for(&p, &act("cam.home", "x")).unwrap_err().contains("takes no value"));
        assert!(request_for(&p, &act("cam.preset", &"a".repeat(65))).is_err());
    }

    #[test]
    fn the_wire_bytes_are_a_plain_http_1_1_request() {
        let r = Request { method: "POST", path: "/api/location/1/2/3/press".into(), body: None, content_type: None };
        assert_eq!(String::from_utf8(wire("10.0.0.5", 8000, &r)).unwrap(), "POST /api/location/1/2/3/press HTTP/1.1\r\nHost: 10.0.0.5:8000\r\nUser-Agent: pchi-studio\r\nAccept: */*\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        let r = Request { method: "GET", path: "/home".into(), body: None, content_type: None };
        assert!(String::from_utf8(wire("cam", 80, &r)).unwrap().ends_with("Connection: close\r\n\r\n"));
        let r = Request { method: "PUT", path: "/x".into(), body: Some("{\"a\":1}".into()), content_type: Some("application/json") };
        assert!(String::from_utf8(wire("h", 1, &r)).unwrap().ends_with("Content-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"));
        assert_eq!(status_of("HTTP/1.1 204 No Content\r\n"), Some(204));
        assert_eq!(status_of("garbage"), None);
    }

    #[test]
    fn a_cue_is_checked_and_described_without_sending() {
        let d = dev("companion-http", "10.0.0.5", None);
        assert!(check_action(&d, &act("visual.button", "1/1/1")).is_ok());
        assert!(check_action(&d, &act("visual.button", "1/1")).is_err());
        let lines = describe(&d, &act("visual.button", "1/2/3")).unwrap();
        assert_eq!(lines[0].0, "Bitfocus Companion (HTTP) HTTP: POST http://10.0.0.5:8000/api/location/1/2/3/press");
        assert!(target_of(&dev("companion-http", "", None), &get("companion-http").unwrap()).is_err());
        assert!(target_of(&dev("companion-http", "a/b", None), &get("companion-http").unwrap()).unwrap_err().contains("plain address"));
        assert!(target_of(&dev("companion-http", "h", Some(99)), &get("companion-http").unwrap()).unwrap().1 == 99);
    }

    /// A stand-in web server: reads one request, records it, answers with `status_line`.
    async fn standin(status_line: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 2048];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(format!("{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (port, h)
    }

    #[tokio::test]
    async fn a_stand_in_server_receives_exactly_the_request_and_its_status_decides_the_outcome() {
        let (port, got) = standin("HTTP/1.1 200 OK").await;
        let d = dev("companion-http", "127.0.0.1", Some(port));
        let o = apply(&d, &act("visual.button", "4/5/6")).await.unwrap();
        assert_eq!(o.status, "accepted");
        assert_eq!(o.target, format!("127.0.0.1:{port}"));
        let req = got.await.unwrap();
        assert!(req.starts_with("POST /api/location/4/5/6/press HTTP/1.1\r\nHost: 127.0.0.1:"), "{req}");

        let (port, _) = standin("HTTP/1.1 404 Not Found").await;
        let e = apply(&dev("companion-http", "127.0.0.1", Some(port)), &act("visual.button", "4/5/6")).await.unwrap_err();
        assert_eq!(e.status(), "refused");
        assert!(e.text().contains("404"), "{}", e.text());

        let (port, _) = standin("not http at all").await;
        assert_eq!(apply(&dev("companion-http", "127.0.0.1", Some(port)), &act("visual.button", "4/5/6")).await.unwrap_err().status(), "failed");

        // nothing listening: failed, quickly, not a hang
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(apply(&dev("companion-http", "127.0.0.1", Some(dead)), &act("visual.button", "1/1/1")).await.unwrap_err().status(), "failed");
        // a bad value is refused before any connection is made
        assert!(apply(&dev("companion-http", "127.0.0.1", Some(dead)), &act("visual.button", "../x")).await.unwrap_err().text().contains("not a value"));
    }
}
