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

/// A primary whose event loop runs this far behind its own timers is called slow.
pub const SLOW_MS: u64 = 250;

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
    /// A primary that accepts connections but does not answer is alive and stuck, not gone: it gets this much longer before a takeover.
    pub stall_grace: Duration,
    /// How long one request to the primary may take before it counts as not answered.
    pub poll_timeout: Duration,
    /// This standby's governance key file and log file (the same paths it runs with): to continue the primary's signed record when it takes over.
    pub governance: Option<(String, String)>,
    /// The token this standby's own address asks for (`GET /takeover-check` needs it when set).
    pub serve_token: Option<String>,
    /// The `--record` file, so the run goes on being written to it after a takeover.
    pub record: Option<String>,
    /// The witness that decides who may send, if there is one (see `witness`): a takeover needs its lease.
    pub witness: Option<crate::witness::Cfg>,
    /// Where others can reach this standby (`--advertise`): told to the runtime it fences and shown by `GET /primary`.
    pub advertise: Option<String>,
}

impl Config {
    pub fn new(primary: &str, name: &str, key: &str) -> Self {
        Self { primary: primary.into(), name: name.into(), key: key.into(), token: None, takeover_after: Duration::from_secs(6), auto: true, poll_every: Duration::from_millis(1000), db: None, stall_grace: Duration::from_secs(6), poll_timeout: Duration::from_secs(3), governance: None, serve_token: None, record: None, witness: None, advertise: None }
    }
}

/// Builds the running show from the standby's copy of the log: given a store holding the copy and the takeover number, it returns the
/// show (resumed, configured as the primary was) and the router to serve it. Supplied by `main`, which knows the command line.
pub type Activate = Box<dyn Fn(EventStore, u64) -> Result<(Arc<Show>, Router), String> + Send + Sync>;

/// Builds the show a takeover would run, from a copy of the log, with sending off and nothing configured: to look at, not to run. Supplied by `main`.
pub type Probe = Box<dyn Fn(EventStore) -> Result<Arc<Show>, String> + Send + Sync>;

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
    /// How the last failed request failed: gone, stalled, unreachable, other (empty after a good answer).
    last_kind: &'static str,
    /// How far behind its own timers the primary's event loop was running, as of its last answer (ms).
    primary_lag_ms: u64,
    copied: u64,
    /// The primary's signed receipts as far as they have been copied, and the key it signs with.
    receipts: Vec<Value>,
    primary_vk: Option<String>,
    receipts_note: String,
    /// The primary's own account of the show as of the last answer (see `Show::replicate_json`).
    primary_summary: Value,
    took_over: Option<Value>,
    /// What the witness last said to a takeover that was refused (empty if there is none or it agreed).
    witness_note: String,
}

pub struct Standby {
    cfg: Config,
    store: Mutex<EventStore>,
    status: Mutex<Status>,
    active: Mutex<Option<Router>>,
    activate: Activate,
    taking: AtomicBool,
    started: Mutex<Option<Started>>,
    probe: Mutex<Option<Probe>>,
    stop: AtomicBool,
}

struct Adoption {
    adopted: usize,
    text: String,
}

/// One HTTP request to a runtime, over a plain socket. `Err` means it could not be reached or did not answer in time. The text starts with what kind of
/// trouble it was, because a standby treats them differently: `refused:` (nothing is listening, or it hung up: the process is gone), `unreachable:` (the
/// network did not carry it) and `stalled:` (it accepted the connection and then did not answer: alive and stuck).
pub async fn http(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<(u16, Value), String> {
    http_t(addr, method, path, headers, body, Duration::from_secs(3)).await
}

pub async fn http_t(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>, timeout: Duration) -> Result<(u16, Value), String> {
    let began = Instant::now();
    let mut s = match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Err(_) => return Err(format!("unreachable: {addr} did not accept the connection in time")),
        Ok(Err(e)) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => return Err(format!("refused: nothing is listening at {addr} ({e})")),
        Ok(Err(e)) => return Err(format!("unreachable: cannot reach {addr}: {e}")),
        Ok(Ok(s)) => s,
    };
    let body = body.unwrap_or("");
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head += &format!("{k}: {v}\r\n");
    }
    let rest_of_time = timeout.saturating_sub(began.elapsed());
    let io = async {
        s.write_all(format!("{head}\r\n{body}").as_bytes()).await.map_err(|e| format!("refused: {addr} hung up ({e})"))?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.map_err(|e| format!("refused: {addr} hung up ({e})"))?;
        if out.is_empty() {
            return Err(format!("refused: {addr} closed the connection without answering"));
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| "other: not an HTTP answer".to_string())?;
        let (h, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let rest = if h.to_ascii_lowercase().contains("transfer-encoding: chunked") { crate::mcp::dechunk(rest) } else { rest.to_string() };
        Ok::<_, String>((status, serde_json::from_str(rest.trim()).unwrap_or(Value::Null)))
    };
    tokio::time::timeout(rest_of_time, io).await.map_err(|_| format!("stalled: {addr} accepted the connection but did not answer in time"))?
}

/// What kind of trouble a failed request was (see `http`).
fn kind_of(err: &str) -> &'static str {
    if err.starts_with("refused:") { "gone" } else if err.starts_with("stalled:") { "stalled" } else if err.starts_with("unreachable:") { "unreachable" } else { "other" }
}

fn copy_into(from: &EventStore, to: &EventStore) -> Result<(), String> {
    for r in from.all().map_err(|e| e.to_string())? {
        to.append(r.timestamp, &r.source, r.target.as_deref(), &r.event_type, &r.payload).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Standby {
    pub fn new(cfg: Config, store: EventStore, activate: Activate) -> Arc<Self> {
        let mut status = Status::default();
        if let Some((_, log)) = &cfg.governance {
            // what an earlier run of this standby had already copied
            if let Ok(text) = std::fs::read_to_string(format!("{log}.primary")) {
                status.receipts = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();
            }
        }
        Arc::new(Self { cfg, store: Mutex::new(store), status: Mutex::new(status), active: Mutex::new(None), activate, taking: AtomicBool::new(false), started: Mutex::new(None), probe: Mutex::new(None), stop: AtomicBool::new(false) })
    }

    /// How to build a takeover for a rehearsal (see `rehearse_takeover`).
    pub fn set_probe(&self, p: Probe) {
        *self.probe.lock().unwrap() = Some(p);
    }

    /// What to do once this standby is running the show (open the inputs).
    pub fn on_active(&self, hook: Started) {
        *self.started.lock().unwrap() = Some(hook);
    }

    pub fn is_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// How many of the primary's signed receipts this standby holds.
    pub fn receipts_copied(&self) -> usize {
        self.status.lock().unwrap().receipts.len()
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
            let have = self.status.lock().unwrap().receipts.len();
            let path = format!("/replicate?after={after}&receipts_after={have}{}", last_ts.map(|t| format!("&last_ts={t}")).unwrap_or_default());
            let res = http_t(&self.cfg.primary, "GET", &path, &self.headers(&bearer), None, self.cfg.poll_timeout).await;
            let (code, v) = match res {
                Ok(x) => x,
                Err(e) => {
                    let mut st = self.status.lock().unwrap();
                    st.last_kind = kind_of(&e);
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
            // the primary's signed receipts: if it has fewer than we hold, it is a different record (a fresh run): start the copy again
            if v["receipts_total"].as_u64().is_some_and(|n| (n as usize) < st.receipts.len()) {
                st.receipts.clear();
                st.receipts_note.clear();
                if let Some((_, log)) = &self.cfg.governance {
                    let _ = std::fs::remove_file(format!("{log}.primary"));
                }
            }
            // kept only while each one follows the one before
            for r in v["receipts"].as_array().into_iter().flatten() {
                let parent = st.receipts.last().and_then(|l| l["decision_hash"].as_str());
                if r["parent_decision_hash"].as_str() != parent {
                    st.receipts_note = "the primary's receipts did not follow one another when copied".into();
                    break;
                }
                st.receipts.push(r.clone());
                if let Some((_, log)) = &self.cfg.governance {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(format!("{log}.primary")) {
                        let _ = writeln!(f, "{r}");
                    }
                }
            }
            st.primary_vk = v["verifying_key"].as_str().map(String::from);
            st.primary_summary = v["summary"].clone();
            st.copied += recs.len() as u64;
            st.last_ok = Some(Instant::now());
            st.stuck = false;
            st.last_kind = "";
            st.primary_lag_ms = v["lag_ms"].as_u64().unwrap_or(0);
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
            let (silent, stuck, has_run, kind) = {
                let st = self.status.lock().unwrap();
                (st.last_ok.map(|t| t.elapsed()), st.stuck, st.copied > 0, st.last_kind)
            };
            // a standby that was refused does not take over: that is a mistake to fix, not a dead primary
            if stuck || !has_run || !self.cfg.auto {
                continue;
            }
            // a primary that still accepts connections but does not answer is stuck, not gone: it is given longer, since taking over from a runtime that is only
            // busy is how two runtimes end up sending at once
            let limit = if kind == "stalled" { self.cfg.takeover_after + self.cfg.stall_grace } else { self.cfg.takeover_after };
            if silent.is_some_and(|s| s >= limit) {
                let how = match kind {
                    "stalled" => " (it accepted connections but stopped answering)",
                    "gone" => " (nothing was listening)",
                    "unreachable" => " (the network did not reach it)",
                    _ => "",
                };
                let why = format!("the primary at {} was silent for {:.0} s{how}", self.cfg.primary, silent.unwrap().as_secs_f64());
                if let Err(e) = self.take_over(&why).await {
                    self.status.lock().unwrap().problem = Some(format!("could not take over: {e}"));
                }
            }
        }
    }

    /// Become the running show, from the copy of the log. Fences the primary at once if it is reachable.
    pub async fn take_over(self: &Arc<Self>, why: &str) -> Result<Value, String> {
        self.take_over_with(why, false).await
    }

    /// `ignore_witness`: a person who knows the witness is down and the primary is dead takes over without its lease.
    pub async fn take_over_with(self: &Arc<Self>, why: &str, ignore_witness: bool) -> Result<Value, String> {
        if self.taking.swap(true, Ordering::SeqCst) || self.is_active() {
            return Err("already running the show".into());
        }
        let result = self.take_over_inner(why, ignore_witness).await;
        if result.is_err() {
            self.taking.store(false, Ordering::SeqCst);
        }
        result
    }

    async fn take_over_inner(self: &Arc<Self>, why: &str, ignore_witness: bool) -> Result<Value, String> {
        // with a witness, nothing starts until it says this runtime is the only one that may send
        let mut leased: Option<Instant> = None;
        let mut witness_text = String::new();
        if let Some(w) = &self.cfg.witness {
            let sent = Instant::now();
            let refusal = match crate::witness::claim(w).await {
                Ok(c) if c.granted => {
                    leased = Some(sent);
                    self.status.lock().unwrap().witness_note.clear();
                    None
                }
                Ok(c) => Some(format!("the witness at {} says {} holds the show's lease ({:.0} s left): the primary can still reach the witness, so this standby did not take over", w.addr, c.holder, c.left_ms as f64 / 1000.0)),
                Err(e) => Some(format!("the witness at {} could not be reached ({e}): taking over without it could put two runtimes on the show, so this standby did not take over", w.addr)),
            };
            if let Some(note) = refusal {
                if !ignore_witness {
                    self.status.lock().unwrap().witness_note = note.clone();
                    return Err(note);
                }
                witness_text = format!(" Taken over WITHOUT the witness's lease, on a person's word. The witness had said: {note}.");
            }
        }
        let _ = self.poll_once().await; // the last of the log, if the primary can still say it
        let (epoch, copied) = {
            let st = self.status.lock().unwrap();
            (st.primary_epoch + 1, st.copied)
        };
        // the show gets its own store, so a refusal leaves the copy untouched for another try
        let mut fresh = match &self.cfg.db {
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
        if let Some(path) = &self.cfg.record {
            // the file already holds everything copied so far; from here the show writes to it
            fresh.set_tee(crate::recording::file_tee(path, &[])?);
        }
        let adoption = self.adopt_receipts(true);
        let (show, router) = (self.activate)(fresh, epoch)?;
        show.set_advertise(self.cfg.advertise.clone());
        if let Some(w) = self.cfg.witness.clone().filter(|_| leased.is_some()) {
            // from now on this runtime holds the lease; a takeover on a person's word without one runs unleased, as that person asked
            crate::witness::hold(Arc::clone(&show), w, leased);
        }
        let started = self.started.lock().unwrap().as_ref().map(|h| h(Arc::clone(&show)));
        if let Some(fut) = started {
            fut.await?;
        }
        let info = json!({"epoch": epoch, "why": why, "copied_records": copied, "primary": self.cfg.primary, "state": show.current_state(), "receipts_adopted": adoption.adopted, "receipts": adoption.text});
        {
            let mut st = self.status.lock().unwrap();
            st.took_over = Some(info.clone());
        }
        show.receipt("failover.takeover", "ALLOW", &crate::governance::Actor::Person(None), &format!("{} took over the show from {} (takeover {epoch}): {why}.{}{}", self.cfg.name, self.cfg.primary, adoption.text, witness_text));
        println!("failover: {} took over as takeover {epoch} in {} ({why}); lasers are disarmed and departments must report READY again", self.cfg.name, info["state"].as_str().unwrap_or("the show"));
        *self.active.lock().unwrap() = Some(router);
        let me = Arc::clone(self);
        tokio::spawn(async move { me.fence_loop(epoch).await });
        Ok(info)
    }

    /// Make the primary's signed receipts the start of this runtime's own log, so the record goes on as one chain that one verifying key checks. That works only
    /// with the same signing key on both machines and a governance log here that is still empty; otherwise this runtime starts its own chain and the takeover
    /// receipt says which of the primary's receipts it follows, so a reader can join the two by hand.
    fn adopt_receipts(&self, write: bool) -> Adoption {
        use primeswarm_pchi::governance::{verify_receipt, GovernanceReceipt};
        let (receipts, vk) = {
            let st = self.status.lock().unwrap();
            (st.receipts.clone(), st.primary_vk.clone())
        };
        let Some((key, log)) = &self.cfg.governance else { return Adoption { adopted: 0, text: String::new() } };
        let Some(tail) = receipts.last().and_then(|r| r["decision_hash"].as_str()) else { return Adoption { adopted: 0, text: " The primary had written no signed receipts.".into() } };
        let follows = format!(" This runtime's own record follows the primary's receipt {tail} (primary verifying key {}).", vk.as_deref().unwrap_or("unknown"));
        let separate = |why: &str| Adoption { adopted: 0, text: format!(" The primary's signed receipts were not adopted: {why}.{follows}") };
        if std::fs::metadata(log).map(|m| m.len() > 0).unwrap_or(false) {
            return separate("this runtime's own governance log already holds receipts, so its chain stays separate");
        }
        let probe = std::env::temp_dir().join(format!("pchi-vk-probe-{}-{}", std::process::id(), tail.chars().take(8).collect::<String>()));
        let own = primeswarm_pchi::GovernanceLog::open(key, &probe.to_string_lossy());
        let _ = std::fs::remove_file(&probe);
        let own_vk = match own {
            Ok(l) => l.verifying_key_hex(),
            Err(e) => return separate(&format!("this runtime's signing key could not be read ({e})")),
        };
        if vk.as_deref() != Some(own_vk.as_str()) {
            return separate("this runtime signs with a different key than the primary: use the same --governance-key file on both to continue one chain");
        }
        let mut parent: Option<String> = None;
        let mut lines = String::new();
        for r in &receipts {
            let Ok(rec) = serde_json::from_value::<GovernanceReceipt>(r.clone()) else { return separate("a copied receipt could not be read") };
            if rec.parent_decision_hash != parent || !verify_receipt(&rec, &own_vk) {
                return separate("the copied receipts did not verify");
            }
            parent = Some(rec.decision_hash.clone());
            lines += &format!("{r}\n");
        }
        if !write {
            return Adoption { adopted: receipts.len(), text: format!(" The primary's {} signed receipts would be adopted: the record would go on as one chain.", receipts.len()) };
        }
        if let Some(dir) = std::path::Path::new(log).parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(log, lines) {
            return separate(&format!("the log could not be written ({e})"));
        }
        Adoption { adopted: receipts.len(), text: format!(" The primary's {} signed receipts were adopted: the record goes on as one chain.", receipts.len()) }
    }

    /// Rehearse a takeover now, changing nothing: copy the last of the log, build the show a takeover would run (sending off, nothing configured, no inputs
    /// opened, no fence sent), and compare it with the primary's own account of the show. Says whether a takeover now would carry the show on, what it would
    /// skip, and how the signed record would continue. The rehearsal show is stopped before it returns.
    pub async fn rehearse_takeover(&self) -> Value {
        let began = Instant::now();
        let reached = self.poll_once().await.is_ok();
        let (primary, health, heard, copied, stuck) = {
            let st = self.status.lock().unwrap();
            (st.primary_summary.clone(), st.last_kind, st.last_ok.map(|t| t.elapsed().as_millis() as u64), st.copied, st.stuck)
        };
        let mut problems: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        if stuck {
            problems.push(self.status.lock().unwrap().problem.clone().unwrap_or_else(|| "the primary refused this standby".into()));
        } else if copied == 0 {
            problems.push("nothing has been copied from the primary yet".into());
        }
        if !reached && !stuck {
            notes.push(format!("the primary did not answer just now ({}): this compares with what it said the last time{}", if health.is_empty() { "unknown" } else { health }, heard.map(|h| format!(", {:.0} s ago", h as f64 / 1000.0)).unwrap_or_default()));
        }
        let mut carried = Value::Null;
        let mut skipped: Vec<String> = Vec::new();
        let mut took_ms = 0u64;
        if problems.is_empty() {
            let copy = EventStore::open_in_memory().map_err(|e| e.to_string()).and_then(|s| copy_into(&self.store.lock().unwrap(), &s).map(|_| s));
            let probe = self.probe.lock().unwrap();
            match (copy, probe.as_ref()) {
                (Err(e), _) => problems.push(format!("the copy could not be read: {e}")),
                (_, None) => problems.push("this standby was not set up for rehearsals".into()),
                (Ok(store), Some(build)) => {
                    let t = Instant::now();
                    match build(store) {
                        Err(e) => problems.push(format!("a takeover would be refused: {e}")),
                        Ok(show) => {
                            took_ms = t.elapsed().as_millis() as u64;
                            let snap = show.snapshot();
                            let polls: Vec<Value> = show.polls_json().as_array().cloned().unwrap_or_default();
                            let pending = show.proposals().as_array().map_or(0, |a| a.iter().filter(|p| p["status"] == "pending").count());
                            carried = json!({"state": snap["state"], "context": snap["context"], "polls": polls, "proposals_pending": pending, "undo_depth": snap["undo_depth"]});
                            skipped = snap["resumed"]["skipped_cues"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(String::from)).collect();
                            if primary.is_object() {
                                let mut diff = |what: &str, mine: &Value, theirs: &Value| {
                                    if mine != theirs {
                                        problems.push(format!("{what} would differ: a takeover would have {mine}, the primary has {theirs}"));
                                    }
                                };
                                diff("the scene", &snap["state"], &primary["state"]);
                                diff("the context", &snap["context"], &primary["context"]);
                                diff("the show's definition", &json!(primeswarm_pchi::governance::hash_state(&snap["definition"])), &primary["definition_hash"]);
                                diff("the undo list", &snap["undo_depth"], &primary["undo_depth"]);
                                diff("the waiting proposals", &json!(pending), &primary["proposals_pending"]);
                                for t in primary["polls"].as_array().into_iter().flatten() {
                                    let mine = polls.iter().find(|p| p["id"] == t["id"]);
                                    match mine {
                                        None => problems.push(format!("the poll '{}' would be missing", t["id"].as_str().unwrap_or(""))),
                                        Some(m) if m["open"] != t["open"] => problems.push(format!("the poll '{}' would be {} instead of {}", t["id"].as_str().unwrap_or(""), if m["open"] == json!(true) { "open" } else { "closed" }, if t["open"] == json!(true) { "open" } else { "closed" })),
                                        Some(m) if m["total"] != t["total"] => notes.push(format!("the poll '{}' would have {} votes, the primary has {}: votes in the last half second are written to the log in batches", t["id"].as_str().unwrap_or(""), m["total"], t["total"])),
                                        _ => {}
                                    }
                                }
                            } else {
                                notes.push("the primary has not given an account of itself to compare with".into());
                            }
                            // retire the rehearsal show: its timers do nothing from here
                            show.fence(u64::MAX / 2, "rehearsal");
                        }
                    }
                }
            }
        }
        let adoption = self.adopt_receipts(false);
        if !adoption.text.trim().is_empty() {
            notes.push(adoption.text.trim().to_string());
        }
        if !skipped.is_empty() {
            notes.push(format!("a takeover now would skip {} cue(s) that are already due: {}", skipped.len(), skipped.join(", ")));
        }
        if matches!(health, "stalled" | "gone" | "unreachable") {
            notes.push("the primary is not answering: a takeover may already be under way".into());
        }
        notes.push("not carried over in any case: signed-in sessions (people sign in again), lasers (they start disarmed), departments' READY reports (they report again)".into());
        let ready = problems.is_empty();
        json!({
            "ready": ready,
            "verdict": if ready { format!("ready: a takeover now would carry the show on from {}", carried["state"].as_str().unwrap_or("where it is")) } else { format!("NOT ready: {}", problems[0]) },
            "problems": problems, "notes": notes, "carried": carried, "skipped_cues": skipped,
            "build_ms": took_ms, "took_ms": began.elapsed().as_millis() as u64, "records_copied": copied, "primary_health": health,
        })
    }

    /// Tell the old primary, for as long as this runs, which takeover it is on, so one that comes back stops at once.
    async fn fence_loop(self: Arc<Self>, epoch: u64) {
        let bearer = self.cfg.token.as_ref().map(|t| format!("Bearer {t}"));
        let body = json!({"epoch": epoch, "by": self.cfg.name, "url": self.cfg.advertise}).to_string();
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
            "ts": crate::now_ms(), "role": "standby", "name": self.cfg.name, "following": self.cfg.primary, "copied_records": st.copied,
            "primary_epoch": st.primary_epoch, "primary_state": st.primary_state, "primary_fenced": st.primary_fenced,
            "witness": self.cfg.witness.as_ref().map(|w| json!({"addr": w.addr, "note": st.witness_note})),
            "last_heard_ms_ago": silent, "problem": st.problem, "stuck": st.stuck,
            "primary_lag_ms": st.primary_lag_ms,
            // ok: answering well; slow: answering but its own event loop is running behind; stalled: accepts connections, does not answer; gone: nothing listening; unreachable: the network
            "primary_health": if st.last_kind.is_empty() { if st.last_ok.is_none() { "unknown" } else if st.primary_lag_ms >= SLOW_MS { "slow" } else { "ok" } } else { st.last_kind },
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
        ("GET", "/primary") => (StatusCode::OK, Json(json!({"primary": sb.cfg.primary, "self": false, "role": "standby", "epoch": sb.role_json()["primary_epoch"], "health": sb.role_json()["primary_health"]}))).into_response(),
        ("GET", "/takeover-check") => {
            let bearer = req.headers().get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(String::from);
            let q_token = req.uri().query().and_then(|q| q.split('&').find_map(|p| p.strip_prefix("token="))).map(String::from);
            if sb.cfg.serve_token.as_deref().is_some_and(|t| bearer.as_deref() != Some(t) && q_token.as_deref() != Some(t)) {
                return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))).into_response();
            }
            (StatusCode::OK, Json(sb.rehearse_takeover().await)).into_response()
        }
        ("POST", "/takeover") => {
            if key.as_deref() != Some(sb.cfg.key.as_str()) {
                return (StatusCode::FORBIDDEN, Json(json!({"error": "taking over needs the failover key"}))).into_response();
            }
            let ignore = req.uri().query().is_some_and(|q| q.split('&').any(|p| p == "witness=ignore"));
            match sb.take_over_with("a person asked for it", ignore).await {
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

/// The probe `main` hands to a standby: resume from the copy, sending off, nothing else.
pub fn probe(file_devices: Option<Vec<rig::Device>>) -> Probe {
    Box::new(move |store| {
        let records = store.all().map_err(|e| e.to_string())?;
        let (chart, logged) = Show::logged_show(&records)?;
        let output = merge_rig(logged, file_devices.clone()).map(|d| crate::output::Output::new(d, false));
        Show::resume(chart, store, output).map(|(show, _)| show)
    })
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
