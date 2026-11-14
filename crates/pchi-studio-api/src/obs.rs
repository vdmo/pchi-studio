//! OBS Studio as a rig device: a cue switches the program scene.
//!
//! This is how a virtual event gets a picture cut: a `scene.activate` cue to an `obs` device names an OBS scene and OBS is told
//! to make it the program (obs-websocket 5, `SetCurrentProgramScene`). The connection is kept open and re-made if it drops, so
//! the link can be watched.
//!
//! The protocol (hello, identify with a salted double-SHA256 password proof, requests and responses) is from the obs-websocket
//! documentation. It has been run against a fake OBS written from that document, not against OBS itself. The default port is not
//! assumed: set the one OBS shows under Tools, WebSocket Server Settings (it is usually 4455).
//!
//! The password is a device parameter (`password`), or the environment variable `PCHI_OBS_PASSWORD`. A parameter is stored in the
//! show file in clear text: prefer the variable.

use crate::rig::Device;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use pchi_showchart::Action;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

const RECONNECT: Duration = Duration::from_secs(2);
const STEP: Duration = Duration::from_secs(3);

/// The `authentication` string for a Hello that carries a `challenge` and a `salt`.
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    b64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

/// The scene name a cue asks for, checked.
pub fn scene_of(action: &Action) -> Result<String, String> {
    if action.kind != "scene.activate" {
        return Err(format!("OBS switches scenes: it takes scene.activate cues, not \"{}\"", action.kind));
    }
    let v = match &action.payload {
        Value::Object(p) => p.values().next().and_then(|v| v.as_str().map(|s| s.trim().to_string()).or_else(|| v.as_i64().map(|n| n.to_string()))).unwrap_or_default(),
        _ => String::new(),
    };
    if v.is_empty() {
        return Err("scene.activate has no value: write the OBS scene's name".into());
    }
    if v.len() > 100 || v.chars().any(|c| c.is_control()) {
        return Err("the scene name is too long or has control characters".into());
    }
    Ok(v)
}

type Reply = oneshot::Sender<Result<Value, String>>;

struct Conn {
    tx: mpsc::Sender<(Value, Reply)>,
    status: Arc<Mutex<(&'static str, String)>>,
    key: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct Obs {
    conns: tokio::sync::Mutex<HashMap<String, Arc<Conn>>>,
}

fn password_of(dev: &Device) -> Option<String> {
    dev.password.clone().filter(|p| !p.is_empty()).or_else(|| std::env::var("PCHI_OBS_PASSWORD").ok().filter(|p| !p.is_empty()))
}

impl Obs {
    /// Make sure there is a connection (or an attempt) for this device.
    pub async fn ensure(&self, dev: &Device) -> Result<(), String> {
        self.conn(dev).await.map(|_| ())
    }

    async fn conn(&self, dev: &Device) -> Result<Arc<Conn>, String> {
        let host = dev.host.as_deref().map(str::trim).filter(|h| !h.is_empty()).ok_or_else(|| format!("{} has no host in the rig", dev.label))?;
        let port = dev.port.ok_or_else(|| format!("{} has no port in the rig: set the one OBS shows under WebSocket Server Settings", dev.label))?;
        let url = format!("ws://{host}:{port}");
        let password = password_of(dev);
        let key = format!("{url}|{}", password.is_some());
        let mut conns = self.conns.lock().await;
        if let Some(c) = conns.get(&dev.id).filter(|c| c.key == key) {
            return Ok(Arc::clone(c));
        }
        let (tx, rx) = mpsc::channel(16);
        let status = Arc::new(Mutex::new(("silent", "connecting".to_string())));
        let task = tokio::spawn(run(url, password, rx, Arc::clone(&status)));
        let c = Arc::new(Conn { tx, status, key, task });
        conns.insert(dev.id.clone(), Arc::clone(&c));
        Ok(c)
    }

    /// "linked" (identified), "lost" (was, is not now), "silent" (not yet), or "none" (no connection tried).
    pub fn state(&self, id: &str) -> (&'static str, String) {
        match self.conns.try_lock() {
            Ok(c) => c.get(id).map(|c| c.status.lock().unwrap().clone()).unwrap_or(("none", String::new())),
            Err(_) => ("silent", String::new()),
        }
    }

    /// Make the scene the program. Returns where it went.
    pub async fn apply(&self, dev: &Device, action: &Action) -> Result<String, String> {
        let scene = scene_of(action)?;
        let conn = self.conn(dev).await?;
        // a first cue waits briefly for the connection to be made
        let until = Instant::now() + STEP;
        while conn.status.lock().unwrap().0 != "linked" && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let (what, text) = conn.status.lock().unwrap().clone();
        if what != "linked" {
            return Err(format!("{} is not connected to OBS ({text}): is OBS running with its WebSocket server on {}:{}?", dev.label, dev.host.clone().unwrap_or_default(), dev.port.unwrap_or(0)));
        }
        let (otx, orx) = oneshot::channel();
        conn.tx.send((json!({"requestType": "SetCurrentProgramScene", "requestData": {"sceneName": scene}}), otx)).await.map_err(|_| "the OBS connection closed".to_string())?;
        match tokio::time::timeout(STEP, orx).await {
            Err(_) => Err(format!("OBS did not answer within {} s", STEP.as_secs())),
            Ok(Err(_)) => Err("the OBS connection dropped before it answered".into()),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Ok(Ok(_))) => Ok(format!("{}:{}", dev.host.clone().unwrap_or_default(), dev.port.unwrap_or(0))),
        }
    }
}

async fn run(url: String, password: Option<String>, mut rx: mpsc::Receiver<(Value, Reply)>, status: Arc<Mutex<(&'static str, String)>>) {
    let set = |s: &'static str, t: &str| *status.lock().unwrap() = (s, t.to_string());
    let mut was_linked = false;
    loop {
        match session(&url, password.as_deref(), &mut rx, &status, &mut was_linked).await {
            Ok(()) => return, // the device was removed
            Err(e) => set(if was_linked { "lost" } else { "silent" }, &e),
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

async fn session(url: &str, password: Option<&str>, rx: &mut mpsc::Receiver<(Value, Reply)>, status: &Arc<Mutex<(&'static str, String)>>, was_linked: &mut bool) -> Result<(), String> {
    let (mut ws, _) = tokio::time::timeout(STEP, tokio_tungstenite::connect_async(url)).await.map_err(|_| format!("connecting to {url} timed out"))?.map_err(|e| format!("cannot connect to {url}: {e}"))?;
    let hello = loop {
        match tokio::time::timeout(STEP, ws.next()).await.map_err(|_| "OBS sent no hello".to_string())? {
            Some(Ok(Message::Text(t))) => break serde_json::from_str::<Value>(&t).map_err(|e| format!("odd hello: {e}"))?,
            Some(Ok(_)) => continue,
            _ => return Err("OBS closed the connection before saying hello".into()),
        }
    };
    if hello["op"] != json!(0) {
        return Err("that is not an obs-websocket server (no Hello)".into());
    }
    let mut d = json!({"rpcVersion": 1, "eventSubscriptions": 0});
    if let Some(a) = hello["d"].get("authentication").filter(|a| a.is_object()) {
        let pw = password.ok_or("OBS wants a password: set the device's password parameter or PCHI_OBS_PASSWORD")?;
        d["authentication"] = json!(auth_string(pw, a["salt"].as_str().unwrap_or(""), a["challenge"].as_str().unwrap_or("")));
    }
    ws.send(Message::Text(json!({"op": 1, "d": d}).to_string())).await.map_err(|e| e.to_string())?;
    loop {
        match tokio::time::timeout(STEP, ws.next()).await.map_err(|_| "OBS did not answer the identify".to_string())? {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                if v["op"] == json!(2) {
                    break;
                }
            }
            Some(Ok(Message::Close(f))) => return Err(format!("OBS refused the login{}", f.map(|f| format!(": {}", f.reason)).filter(|s| s.len() > 2).unwrap_or_default())),
            Some(Ok(_)) => continue,
            _ => return Err("OBS closed the connection: wrong password?".into()),
        }
    }
    *was_linked = true;
    *status.lock().unwrap() = ("linked", "connected".into());
    let next_id = AtomicU64::new(1);
    let mut pending: HashMap<String, Reply> = HashMap::new();
    let outcome: Result<(), String> = loop {
        tokio::select! {
            req = rx.recv() => {
                let Some((mut body, reply)) = req else { return Ok(()) };
                let id = next_id.fetch_add(1, Ordering::Relaxed).to_string();
                body["requestId"] = json!(id);
                if let Err(e) = ws.send(Message::Text(json!({"op": 6, "d": body}).to_string())).await {
                    let _ = reply.send(Err(format!("could not send to OBS: {e}")));
                    break Err("the connection to OBS broke".into());
                }
                pending.insert(id, reply);
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    if v["op"] == json!(7) {
                        let id = v["d"]["requestId"].as_str().unwrap_or("").to_string();
                        if let Some(reply) = pending.remove(&id) {
                            let st = &v["d"]["requestStatus"];
                            let _ = reply.send(if st["result"] == json!(true) { Ok(v["d"]["responseData"].clone()) } else { Err(format!("OBS said no: {}", st["comment"].as_str().unwrap_or("(no reason given)"))) });
                        }
                    }
                }
                Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                Some(Ok(Message::Close(_))) | None => break Err("OBS closed the connection".into()),
                Some(Err(e)) => break Err(format!("the connection to OBS broke: {e}")),
                _ => {}
            }
        }
    };
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err("the OBS connection dropped before it answered".into()));
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::fake::fake_obs;
    use super::*;

    #[test]
    fn the_password_proof_is_the_documented_double_hash() {
        // computed independently: base64(sha256(base64(sha256(password + salt)) + challenge))
        let b64 = base64::engine::general_purpose::STANDARD;
        let inner = b64.encode(Sha256::digest(b"secretSALT"));
        let want = b64.encode(Sha256::digest(format!("{inner}CHALLENGE").as_bytes()));
        assert_eq!(auth_string("secret", "SALT", "CHALLENGE"), want);
        assert_ne!(auth_string("secret", "SALT", "OTHER"), want);
        assert_eq!(auth_string("a", "b", "c").len(), 44, "a base64 SHA-256 is 44 characters");
    }

    #[test]
    fn a_scene_cue_is_read_and_checked() {
        let a = |v: Value| Action::new("scene.activate", json!({"scene": v}));
        assert_eq!(scene_of(&a(json!("Main Camera"))).unwrap(), "Main Camera");
        assert_eq!(scene_of(&a(json!(2))).unwrap(), "2");
        assert!(scene_of(&a(json!(""))).unwrap_err().contains("no value"));
        assert!(scene_of(&a(json!("x".repeat(200)))).unwrap_err().contains("too long"));
        assert!(scene_of(&a(json!("a\nb"))).unwrap_err().contains("control"));
        assert!(scene_of(&Action::new("lighting.look", json!({"look": "1"}))).unwrap_err().contains("takes scene.activate"));
    }

    fn dev(port: u16, password: Option<&str>) -> Device {
        Device { id: "obs1".into(), label: "OBS".into(), kind: "obs".into(), dept: "visual".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: password.map(String::from) }
    }

    fn scene(n: &str) -> Action {
        Action::new("scene.activate", json!({"scene": n}))
    }

    #[tokio::test]
    async fn a_cue_makes_the_scene_the_program() {
        let (port, scenes) = fake_obs(None).await;
        let obs = Obs::default();
        assert_eq!(obs.state("obs1").0, "none");
        obs.apply(&dev(port, None), &scene("Main Camera")).await.unwrap();
        obs.apply(&dev(port, None), &scene("Slides")).await.unwrap();
        assert_eq!(*scenes.lock().unwrap(), ["Main Camera", "Slides"]);
        assert_eq!(obs.state("obs1").0, "linked");
        // OBS saying no is the cue's failure, with OBS's own reason
        let e = obs.apply(&dev(port, None), &scene("Missing")).await.unwrap_err();
        assert!(e.contains("OBS said no") && e.contains("No scene was found"), "{e}");
        assert_eq!(obs.apply(&dev(port, None), &scene("Back")).await.is_ok(), true, "and the connection is still good");
    }

    #[tokio::test]
    async fn the_password_is_proved_and_a_wrong_one_is_refused_with_a_reason() {
        let (port, scenes) = fake_obs(Some("hunter2")).await;
        let obs = Obs::default();
        obs.apply(&dev(port, Some("hunter2")), &scene("Locked")).await.unwrap();
        assert_eq!(*scenes.lock().unwrap(), ["Locked"]);
        let bad = Obs::default();
        let e = bad.apply(&dev(port, Some("wrong")), &scene("X")).await.unwrap_err();
        assert!(e.contains("not connected to OBS"), "{e}");
        assert!(bad.state("obs1").1.contains("refused") || bad.state("obs1").1.contains("password"), "{:?}", bad.state("obs1"));
        // no password given at all: it says so
        let none = Obs::default();
        let e = none.apply(&dev(port, None), &scene("X")).await.unwrap_err();
        assert!(e.contains("not connected") && none.state("obs1").1.contains("wants a password"), "{e} / {:?}", none.state("obs1"));
    }

    #[tokio::test]
    async fn an_obs_that_is_not_running_is_a_clear_failure_and_a_comeback_is_noticed() {
        let obs = Obs::default();
        // nothing listening
        let free = { let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
        let e = obs.apply(&dev(free, None), &scene("Main")).await.unwrap_err();
        assert!(e.contains("is OBS running with its WebSocket server"), "{e}");
        assert_eq!(obs.state("obs1").0, "silent");
        assert!(Device::is_direct_kind("obs"));
    }
}

/// A pretend OBS for tests, here and in the runtime's own tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use tokio::net::TcpListener;

    /// A fake obs-websocket 5 server. With a password it asks for the proof. Scene "Missing" does not exist.
    pub async fn fake_obs(password: Option<&'static str>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let scenes: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&scenes);
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                    let mut hello = json!({"op": 0, "d": {"obsStudioVersion": "31.0.0", "obsWebSocketVersion": "5.5.0", "rpcVersion": 1}});
                    if password.is_some() {
                        hello["d"]["authentication"] = json!({"challenge": "CH4LLENGE", "salt": "S4LT"});
                    }
                    ws.send(Message::Text(hello.to_string())).await.unwrap();
                    let Some(Ok(Message::Text(t))) = ws.next().await else { return };
                    let id: Value = serde_json::from_str(&t).unwrap();
                    assert_eq!(id["op"], 1);
                    assert_eq!(id["d"]["rpcVersion"], 1);
                    if let Some(pw) = password {
                        if id["d"]["authentication"] != json!(auth_string(pw, "S4LT", "CH4LLENGE")) {
                            let _ = ws.close(None).await;
                            return;
                        }
                    }
                    ws.send(Message::Text(json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}).to_string())).await.unwrap();
                    while let Some(Ok(Message::Text(t))) = ws.next().await {
                        let v: Value = serde_json::from_str(&t).unwrap();
                        if v["op"] != 6 {
                            continue;
                        }
                        let d = &v["d"];
                        assert_eq!(d["requestType"], "SetCurrentProgramScene");
                        let name = d["requestData"]["sceneName"].as_str().unwrap_or("").to_string();
                        let ok = name != "Missing";
                        if ok {
                            log.lock().unwrap().push(name);
                        }
                        let status = if ok { json!({"result": true, "code": 100}) } else { json!({"result": false, "code": 600, "comment": "No scene was found by the name of `Missing`."}) };
                        ws.send(Message::Text(json!({"op": 7, "d": {"requestType": d["requestType"], "requestId": d["requestId"], "requestStatus": status}}).to_string())).await.unwrap();
                    }
                });
            }
        });
        (port, scenes)
    }

}
