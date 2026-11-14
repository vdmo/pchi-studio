//! A second runtime that follows the first and takes over if it goes quiet.
//!
//! The runtime is one process on one machine. If it dies mid-show, a person restarts it with `--resume` and the show goes on, minutes
//! later. A **standby** shortens that to seconds: it runs on another machine, copies the primary's log as it is written (every input,
//! every context change, every live edit, whole), and when the primary has been silent for a while it builds the run from that copy and
//! starts running the show itself.
//!
//! * **What the standby does while it waits:** nothing but copy. It sends no cue, opens no input, answers every request with
//!   "standby" (except `GET /role`). It has the rig from the show file it was started with, and the live edits from the log.
//! * **What taking over is:** the same as restarting with `--resume`, from the copy: the scene and context the show was in, the cues
//!   still ahead armed, cues that came due while nobody was running skipped and listed, lasers disarmed, departments asked to report
//!   READY again. Not carried over: open polls, signed-in sessions, edit proposals waiting, the undo list. The standby's receipts
//!   start a new chain (it has its own `--governance` folder).
//! * **Who may do what:** `--failover-key` (the same on both) opens the log to the standby and lets it tell the primary "I have taken
//!   over". Without it the primary refuses both.
//! * **Taking over by itself (`--takeover auto`, the default) or on a person's word (`--takeover manual`, then `POST /takeover`).**
//! * **Fencing:** once the standby is running it keeps telling the old primary, every few seconds, which takeover it is on. A primary
//!   that hears a newer one stops: sends nothing, stops streaming lighting, disarms its lasers, ignores inputs and timers. So a primary
//!   that was only slow or restarted does not fight the standby over the lights. A primary that cannot be reached cannot be told: if the
//!   network between the two is cut while both can still reach the tools, both will send until it is mended. Use `--takeover manual`
//!   when that is the way your network fails.
//!
//! A handover on purpose (`POST /takeover` while the primary is healthy) first copies the last of the log, then takes over and fences
//! the primary at once.

use crate::{rig, router_with_token, Show};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json, Router,
};
use pchi_eventstore::{recorder, EventStore};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

/// How the standby follows and when it takes over.
#[derive(Clone)]
pub struct Config {
    /// `host:port` of the primary.
    pub primary: String,
    /// This standby's name as the primary's record will show it.
    pub name: String,
    /// The failover key, the same on both.
    pub key: String,
    /// The primary's feed token, if it has one.
    pub token: Option<String>,
    /// How long the primary may be silent before the standby takes over (when `auto`).
    pub takeover_after: Duration,
    /// Whether the standby takes over by itself.
    pub auto: bool,
    /// How often the log is asked for.
    pub poll_every: Duration,
    /// Where the standby's copy of the log is kept (`None`: in memory).
    pub db: Option<String>,
}

impl Config {
    pub fn new(primary: &str, name: &str, key: &str) -> Self {
        Self { primary: primary.into(), name: name.into(), key: key.into(), token: None, takeover_after: Duration::from_secs(6), auto: true, poll_every: Duration::from_millis(1000), db: None }
    }
}

/// Builds the running show from the standby's copy of the log: given a store holding the copy and the takeover number, it returns the
/// show (resumed, configured as the primary was) and the router to serve it. Supplied by `main`, which knows the command line.
pub type Activate = Box<dyn Fn(EventStore, u64) -> Result<(Arc<Show>, Router), String> + Send + Sync>;

/// Runs once after taking over, for what needs waiting on (opening the inputs).
pub type Started = Box<dyn Fn(Arc<Show>) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> + Send + Sync>;

#[derive(Default)]
struct Status {
    /// When the primary last answered.
    last_ok: Option<Instant>,
    /// The error from the last poll that failed, in words.
    problem: Option<String>,
    /// Set when following cannot go on without a person (the log diverged, the key is wrong).
    stuck: bool,
    primary_epoch: u64,
    primary_state: String,
    primary_fenced: bool,
    copied: u64,
    took_over: Option<Value>,
}

pub struct Standby {
    cfg: Config,
    store: Mutex<EventStore>,
    status: Mutex<Status>,
    active: Mutex<Option<Router>>,
    activate: Activate,
    taking: AtomicBool,
    started: Mutex<Option<Started>>,
    stop: AtomicBool,
}

/// One HTTP request to a runtime, over a plain socket. `Err` means it could not be reached or did not answer in time.
pub(crate) async fn http(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<(u16, Value), String> {
    let io = async {
        let mut s = TcpStream::connect(addr).await.map_err(|e| format!("cannot reach {addr}: {e}"))?;
        let body = body.unwrap_or("");
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
        for (k, v) in headers {
            head += &format!("{k}: {v}\r\n");
        }
        s.write_all(format!("{head}\r\n{body}").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out).into_owned();
        let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("not an HTTP answer")?;
        let (h, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let rest = if h.to_ascii_lowercase().contains("transfer-encoding: chunked") { crate::mcp::dechunk(rest) } else { rest.to_string() };
        Ok::<_, String>((status, serde_json::from_str(rest.trim()).unwrap_or(Value::Null)))
    };
    tokio::time::timeout(Duration::from_secs(3), io).await.map_err(|_| format!("{addr} did not answer in time"))?
}

fn copy_into(from: &EventStore, to: &EventStore) -> Result<(), String> {
    for r in from.all().map_err(|e| e.to_string())? {
        to.append(r.timestamp, &r.source, r.target.as_deref(), &r.event_type, &r.payload).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Standby {
    pub fn new(cfg: Config, store: EventStore, activate: Activate) -> Arc<Self> {
        Arc::new(Self { cfg, store: Mutex::new(store), status: Mutex::new(Status::default()), active: Mutex::new(None), activate, taking: AtomicBool::new(false), started: Mutex::new(None), stop: AtomicBool::new(false) })
    }

    /// What to do once this standby is running the show (open the inputs).
    pub fn on_active(&self, hook: Started) {
        *self.started.lock().unwrap() = Some(hook);
    }

    pub fn is_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// How many records of the primary's log this standby holds.
    pub fn copied(&self) -> u64 {
        self.status.lock().unwrap().copied
    }

    fn headers<'a>(&'a self, bearer: &'a Option<String>) -> Vec<(&'a str, &'a str)> {
        let mut h = vec![("X-Pchi-Failover-Key", self.cfg.key.as_str())];
        if let Some(t) = bearer {
            h.push(("Authorization", t.as_str()));
        }
        h
    }

    /// Ask the primary for the log after what we hold and add it to our copy. Repeats while there is more.
    pub async fn poll_once(&self) -> Result<(), String> {
        let bearer = self.cfg.token.as_ref().map(|t| format!("Bearer {t}"));
        loop {
            let (after, last_ts) = {
                let s = self.store.lock().unwrap();
                let after = s.last_id().map_err(|e| e.to_string())?;
                (after, s.timestamp_of(after).map_err(|e| e.to_string())?)
            };
            let path = format!("/replicate?after={after}{}", last_ts.map(|t| format!("&last_ts={t}")).unwrap_or_default());
            let res = http(&self.cfg.primary, "GET", &path, &self.headers(&bearer), None).await;
            let (code, v) = match res {
                Ok(x) => x,
                Err(e) => {
                    let mut st = self.status.lock().unwrap();
                    if !st.stuck {
                        st.problem = Some(e.clone()); // a refusal stays the reason until the primary answers properly
                    }
                    return Err(e);
                }
            };
            match code {
                200 => {}
                403 | 404 | 401 | 409 => {
                    let why = v["error"].as_str().unwrap_or("refused").to_string();
                    let mut st = self.status.lock().unwrap();
                    st.stuck = true;
                    st.problem = Some(format!("the primary refused the standby: {why}"));
                    return Err(st.problem.clone().unwrap());
                }
                _ => {
                    let e = format!("the primary answered {code}");
                    self.status.lock().unwrap().problem = Some(e.clone());
                    return Err(e);
                }
            }
            let recs = v["records"].as_array().cloned().unwrap_or_default();
            {
                let s = self.store.lock().unwrap();
                for r in &recs {
                    let id = s.append(r["ts"].as_i64().unwrap_or(0), r["source"].as_str().unwrap_or(""), r["target"].as_str(), r["type"].as_str().unwrap_or(""), &r["payload"]).map_err(|e| e.to_string())?;
                    if Some(id) != r["id"].as_i64() {
                        let mut st = self.status.lock().unwrap();
                        st.stuck = true;
                        st.problem = Some("the copy's numbering no longer matches the primary's: start the standby with an empty database".into());
                        return Err(st.problem.clone().unwrap());
                    }
                }
            }
            let mut st = self.status.lock().unwrap();
            st.copied += recs.len() as u64;
            st.last_ok = Some(Instant::now());
            st.stuck = false;
            st.problem = None;
            st.primary_epoch = v["epoch"].as_u64().unwrap_or(0);
            st.primary_state = v["state"].as_str().unwrap_or("").to_string();
            st.primary_fenced = v["fenced"].as_bool().unwrap_or(false);
            let (last, mine) = (v["last_id"].as_i64().unwrap_or(0), after + recs.len() as i64);
            drop(st);
            if recs.is_empty() || mine >= last {
                return Ok(());
            }
        }
    }

    /// Follow the primary until this standby takes over (or is stopped). Spawn it.
    pub async fn follow(self: Arc<Self>) {
        loop {
            tokio::time::sleep(self.cfg.poll_every).await;
            if self.stop.load(Ordering::SeqCst) || self.is_active() {
                return;
            }
            if self.poll_once().await.is_ok() {
                continue;
            }
            let (silent, stuck, has_run) = {
                let st = self.status.lock().unwrap();
                (st.last_ok.map(|t| t.elapsed()), st.stuck, st.copied > 0)
            };
            // a standby that was refused does not take over: that is a mistake to fix, not a dead primary
            if stuck || !has_run || !self.cfg.auto {
                continue;
            }
            if silent.is_some_and(|s| s >= self.cfg.takeover_after) {
                let why = format!("the primary at {} was silent for {:.0} s", self.cfg.primary, silent.unwrap().as_secs_f64());
                if let Err(e) = self.take_over(&why).await {
                    self.status.lock().unwrap().problem = Some(format!("could not take over: {e}"));
                }
            }
        }
    }

    /// Become the running show, from the copy of the log. Fences the primary at once if it is reachable.
    pub async fn take_over(self: &Arc<Self>, why: &str) -> Result<Value, String> {
        if self.taking.swap(true, Ordering::SeqCst) || self.is_active() {
            return Err("already running the show".into());
        }
        let result = self.take_over_inner(why).await;
        if result.is_err() {
            self.taking.store(false, Ordering::SeqCst);
        }
        result
    }

    async fn take_over_inner(self: &Arc<Self>, why: &str) -> Result<Value, String> {
        let _ = self.poll_once().await; // the last of the log, if the primary can still say it
        let (epoch, copied) = {
            let st = self.status.lock().unwrap();
            (st.primary_epoch + 1, st.copied)
        };
        // the show gets its own store, so a refusal leaves the copy untouched for another try
        let fresh = match &self.cfg.db {
            Some(p) => EventStore::open(p).map_err(|e| e.to_string())?,
            None => {
                let s = EventStore::open_in_memory().map_err(|e| e.to_string())?;
                copy_into(&self.store.lock().unwrap(), &s)?;
                s
            }
        };
        if fresh.all().map_err(|e| e.to_string())?.iter().all(|r| r.event_type != recorder::RUN_START) {
            return Err("nothing has been copied from the primary yet: there is no run to take over".into());
        }
        let (show, router) = (self.activate)(fresh, epoch)?;
        let started = self.started.lock().unwrap().as_ref().map(|h| h(Arc::clone(&show)));
        if let Some(fut) = started {
            fut.await?;
        }
        let info = json!({"epoch": epoch, "why": why, "copied_records": copied, "primary": self.cfg.primary, "state": show.current_state()});
        {
            let mut st = self.status.lock().unwrap();
            st.took_over = Some(info.clone());
        }
        show.receipt("failover.takeover", "ALLOW", &crate::governance::Actor::Person(None), &format!("{} took over the show from {} (takeover {epoch}): {why}", self.cfg.name, self.cfg.primary));
        println!("failover: {} took over as takeover {epoch} in {} ({why}); lasers are disarmed and departments must report READY again", self.cfg.name, info["state"].as_str().unwrap_or("the show"));
        *self.active.lock().unwrap() = Some(router);
        let me = Arc::clone(self);
        tokio::spawn(async move { me.fence_loop(epoch).await });
        Ok(info)
    }

    /// Tell the old primary, for as long as this runs, which takeover it is on, so one that comes back stops at once.
    async fn fence_loop(self: Arc<Self>, epoch: u64) {
        let bearer = self.cfg.token.as_ref().map(|t| format!("Bearer {t}"));
        let body = json!({"epoch": epoch, "by": self.cfg.name}).to_string();
        let mut told = false;
        loop {
            if let Ok((200, v)) = http(&self.cfg.primary, "POST", "/fence", &self.headers(&bearer), Some(&body)).await {
                if v["fenced"] == json!(true) && !told {
                    told = true;
                    eprintln!("failover: {} has been told takeover {epoch} is running and stopped", self.cfg.primary);
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// What a client sees from a standby that is not running the show yet.
    pub fn role_json(&self) -> Value {
        let st = self.status.lock().unwrap();
        let silent = st.last_ok.map(|t| t.elapsed().as_millis() as u64);
        json!({
            "role": "standby", "name": self.cfg.name, "following": self.cfg.primary, "copied_records": st.copied,
            "primary_epoch": st.primary_epoch, "primary_state": st.primary_state, "primary_fenced": st.primary_fenced,
            "last_heard_ms_ago": silent, "problem": st.problem, "stuck": st.stuck,
            "takeover": if self.cfg.auto { json!({"mode": "auto", "after_s": self.cfg.takeover_after.as_secs_f64()}) } else { json!({"mode": "manual"}) },
            "ready": st.copied > 0 && !st.stuck,
        })
    }
}

/// The standby's HTTP face: everything is "standby" until it takes over, then everything is the show's.
pub fn standby_router(sb: Arc<Standby>) -> Router {
    Router::new().fallback(entry).with_state(sb)
}

async fn entry(State(sb): State<Arc<Standby>>, req: Request) -> Response {
    let active = sb.active.lock().unwrap().clone();
    if let Some(r) = active {
        return match r.oneshot(req).await {
            Ok(resp) => resp,
            Err(e) => match e {},
        };
    }
    let (path, method) = (req.uri().path().to_string(), req.method().clone());
    let key = req.headers().get("x-pchi-failover-key").and_then(|v| v.to_str().ok()).map(String::from);
    match (method.as_str(), path.as_str()) {
        ("GET", "/role") => (StatusCode::OK, Json(sb.role_json())).into_response(),
        ("POST", "/takeover") => {
            if key.as_deref() != Some(sb.cfg.key.as_str()) {
                return (StatusCode::FORBIDDEN, Json(json!({"error": "taking over needs the failover key"}))).into_response();
            }
            match sb.take_over("a person asked for it").await {
                Ok(info) => (StatusCode::OK, Json(json!({"took_over": true, "info": info}))).into_response(),
                Err(e) => (StatusCode::CONFLICT, Json(json!({"error": e}))).into_response(),
            }
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "this is the standby runtime: it is following the show and will answer if it takes over", "role": sb.role_json()}))).into_response(),
    }
}

/// Which rig the standby should run with: the live rig from the log, with passwords (never logged) taken from the show file.
pub fn merge_rig(logged: Option<Vec<rig::Device>>, from_file: Option<Vec<rig::Device>>) -> Option<Vec<rig::Device>> {
    match (logged.filter(|d| !d.is_empty()), from_file) {
        (Some(mut l), f) => {
            for d in l.iter_mut() {
                if d.password.is_none() {
                    d.password = f.as_ref().and_then(|f| f.iter().find(|x| x.id == d.id)).and_then(|x| x.password.clone());
                }
            }
            Some(l)
        }
        (None, f) => f,
    }
}

/// Make the activation `main` hands to a standby: resume from the copy, with the rig as the log last had it.
pub fn activation(file_devices: Option<Vec<rig::Device>>, send: bool, token: Option<String>, configure: Box<dyn Fn(&Arc<Show>) -> Result<(), String> + Send + Sync>) -> Activate {
    Box::new(move |store, epoch| {
        let records = store.all().map_err(|e| e.to_string())?;
        let (chart, logged) = Show::logged_show(&records)?;
        let output = merge_rig(logged, file_devices.clone()).map(|d| crate::output::Output::new(d, send));
        let (show, _info) = Show::resume(chart, store, output)?;
        show.set_epoch(epoch);
        configure(&show)?;
        Ok((show.clone(), router_with_token(show, token.clone())))
    })
}
