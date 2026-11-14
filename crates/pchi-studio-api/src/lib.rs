//! Live state feed for Studio.
//!
//! The server owns the engine; Studio observes it and may send inputs.
//!
//! WebSocket `/ws`, server -> client (JSON text frames):
//!   {"type":"hello", "chart":{id,name,version}, "definition":<showchart>, "state", "context", "timers":[ms]}
//!   {"type":"transition", "ts", "source", "event", "from", "to", "applied"}
//!   {"type":"action", "ts", "event":{"type":<kind>, ...payload}}
//!   {"type":"context", "ts", "key", "value"}
//!   {"type":"readiness", "ts", "dept", "ready", "note", "source"}
//!   {"type":"delivery", "ts", "kind", "lane", "cue", "status", "device", "label", "dept", "target", "error"}
//!       status: "sent" | "dry-run" | "no-device" | "failed" (see the `output` module)
//!   {"type":"feedback", "ts", "device", "label", "address", "args"}: what a device sent back (rate-limited)
//!   {"type":"link", "ts", "device", "state"}: linked | lost | heard | silent | none (see `Output::link_state`)
//!   {"type":"arming", "ts", "device", "armed"}: a laser was armed or disarmed (lasers get no cues until armed)
//!   {"type":"show", "definition", "devices", "diff"}: the show was replaced while running
//!   {"type":"error", "message"}
//! client -> server:
//!   {"type":"input","event":"START"}  {"type":"context","key":"energy","value":0.3}  {"type":"reset"}
//!   {"type":"ready","dept":"lighting","ready":true,"note":"cue 12 loaded","source":"dept-page"}
//!       (a client that repeats the same report every few seconds is a heartbeat: it is not logged)
//!
//! HTTP: `POST /show` (send an edited show; `?dry=1` previews the changes), `GET /preflight` (the doors check), `POST /devices/<id>/try` (one cue to one device), `POST /laser/stop` (hard stop), `POST /devices/<id>/arm?armed=1|0`, `POST /show/undo` (back to before the last send; with `--edit-key` both need `X-Pchi-Edit-Key`), `GET /report` (planned vs actual cue times, delivery problems),
//! `POST /devices/<id>/test` (one harmless test message to a device), `GET /state` (the hello snapshot), `GET /events` (the event log),
//! `GET /ready`, `POST /ready/<dept>` {"ready":true,"note":"..."} and
//! `POST /input` {"event":"GO"}: plain HTTP so any desk, script or button box can take part.
//!
//! Readiness is how departments (visuals, lighting, lasers, audio, anything
//! else) tell the operator they are standing by. It never blocks the show:
//! the operator sees it and decides. With a token set (`--token`) every
//! request must carry it as `?token=` or `Authorization: Bearer`.
//! Every input is recorded to the event store, so a live session can be replayed.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
pub mod audience;
pub mod diff;
pub mod drill;
pub mod dmx;
pub mod failover;
pub mod fixtures;
pub mod gearcheck;
pub mod governance;
pub mod inputs;
pub mod mcp;
pub mod midi;
pub mod links;
pub mod obs;
pub mod ltc;
pub mod output;
pub mod preflight;
pub mod profiles;
pub mod recording;
pub mod rehearse;
pub mod timecode;
pub mod rig;

use futures_util::{SinkExt, StreamExt};
use governance::{said_suffix, Actor, Perm, Proposal};
use output::{Output, Plan};
use timecode::{Fps, Step as TcStep};
use pchi_eventstore::{recorder, EventStore};
use pchi_showchart::{timer_event, Action, ScheduledCue, ShowEngine, Showchart};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Something the timecode made due.
enum TcDue {
    /// A cue of the scene's timecode-locked timeline, with the timecode (ms) its scene clock reads zero at.
    Cue(ScheduledCue, f64),
    /// A `tc:` transition's event.
    Event(String),
}

/// What happened to an input fed to the show.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// It moved the show to another scene.
    Moved,
    /// It did nothing (no transition for it here).
    Ignored,
    /// It would have moved the scene on, but the scene has not run its minimum yet.
    Held { scene: String, remaining_ms: u64, min_ms: u64 },
}

impl Applied {
    /// Words for a person: why a press did nothing.
    pub fn held_text(&self) -> Option<String> {
        match self {
            Applied::Held { scene, remaining_ms, .. } => Some(format!("held: {scene} must run {:.1} s more before it can be moved on", *remaining_ms as f64 / 1000.0)),
            _ => None,
        }
    }
}

/// What a session is kept under: a hash of its token, so neither memory dumps nor the sessions file hold a token that works.
fn session_id(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct Inner {
    engine: ShowEngine,
    store: EventStore,
    /// Bumped on every state change so timers armed for an older state are ignored.
    generation: u64,
    /// When the current state was entered (ms since epoch). Lets a client that
    /// connects mid-state draw the right timer countdown.
    entered_at: i64,
}

pub struct Show {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Value>,
    /// Latest word from each department: {ready, note, ts, source}.
    readiness: Mutex<BTreeMap<String, Value>>,
    /// Where cues are sent. None when the show was loaded without a rig (nothing is routed).
    output: Mutex<Option<Arc<Output>>>,
    /// When set, sending a show, undoing a send and test-firing need this key (the operator's).
    edit_key: Mutex<Option<String>>,
    /// Shows replaced by a send, newest last, so a send can be undone. Each keeps the rig that went with it.
    history: Mutex<Vec<(Showchart, Option<Vec<rig::Device>>)>>,
    /// Whether the link watcher task is running (see `watch_links`).
    watching: std::sync::atomic::AtomicBool,
    /// Set when this run was resumed from the log after a restart (shown to clients in the hello).
    resumed: Mutex<Option<Value>>,
    /// Who may do what, agent proposals, and the signed receipts (see the `governance` module).
    gov: governance::Gov,
    /// Named people and their roles (empty: the single operator key, or an open show).
    roster: Mutex<governance::Roster>,
    /// OSC inputs listening for button presses (see `inputs`).
    inputs: Mutex<Vec<Arc<dyn inputs::InputInfo>>>,
    /// The timecode being heard (from MIDI Time Code or OSC), if any.
    tc: Mutex<timecode::Clock>,
    /// When a timecode message last went out to clients (they get at most five a second).
    tc_published: std::sync::atomic::AtomicI64,
    /// Cues the tools themselves fired (heard as feedback), newest last: what Studio can add to the show.
    captures: Mutex<std::collections::VecDeque<Value>>,
    /// Audience polls (see `audience`), the token the audience's gateway votes with, and when each poll's tally last went out.
    votes: Mutex<audience::Votes>,
    audience_token: Mutex<Option<String>>,
    vote_window: Mutex<(i64, u32)>,
    poll_pub: Mutex<HashMap<String, (i64, bool)>>,
    /// Signed-in sessions: a short-lived token stands in for the person's key, so the key is typed once and not kept.
    sessions: Mutex<HashMap<String, (Actor, i64)>>,
    session_hours: std::sync::atomic::AtomicU64,
    /// Where sessions are kept so a restart does not sign everyone out (only a hash of each token is written).
    sessions_file: Mutex<Option<String>>,
    /// Recent failed sign-ins: (start of the minute, count). Too many in a minute and sign-in waits.
    signin_fails: Mutex<(i64, u32)>,
    /// Failover: which takeover this run is on (0 until a standby takes over; a standby that does so is one higher), who took over from it, and the secret a standby proves itself with.
    epoch: std::sync::atomic::AtomicU64,
    fenced: Mutex<Option<Value>>,
    failover_key: Mutex<Option<String>>,
}

/// How long a department page may stay silent before it counts as not linked (mirrors the Studio).
const STALE_MS: i64 = 30_000;
/// How many sends can be undone.
const UNDO_DEPTH: usize = 10;

impl Show {
    /// Validates the chart, begins a recorded run and arms the initial state's timers.
    pub fn start(chart: Showchart, store: EventStore) -> Result<Arc<Self>, String> {
        Self::start_with(chart, store, None)
    }

    /// As `start`, routing cues to the devices in `output` (see the `output` module).
    pub fn start_with(chart: Showchart, store: EventStore, output: Option<Output>) -> Result<Arc<Self>, String> {
        let engine = ShowEngine::new(chart)
            .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
        let ts = now_ms();
        recorder::begin_run(&store, &engine, ts).map_err(|e| e.to_string())?;
        recorder::record_start(&store, &engine, ts).map_err(|e| e.to_string())?;
        let show = Self::assemble(engine, store, output, ts);
        show.watch_links();
        show.arm(0);
        Ok(show)
    }

    fn assemble(engine: ShowEngine, store: EventStore, output: Option<Output>, entered_at: i64) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner { engine, store, generation: 0, entered_at }),
            tx: broadcast::channel(1024).0,
            readiness: Mutex::new(BTreeMap::new()),
            output: Mutex::new(output.map(Arc::new)),
            edit_key: Mutex::new(None),
            history: Mutex::new(Vec::new()),
            watching: std::sync::atomic::AtomicBool::new(false),
            resumed: Mutex::new(None),
            gov: Default::default(),
            roster: Mutex::new(Default::default()),
            inputs: Mutex::new(Vec::new()),
            tc: Mutex::new(timecode::Clock::default()),
            tc_published: std::sync::atomic::AtomicI64::new(0),
            captures: Mutex::new(std::collections::VecDeque::new()),
            votes: Mutex::new(audience::Votes::default()),
            audience_token: Mutex::new(None),
            vote_window: Mutex::new((0, 0)),
            poll_pub: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            session_hours: std::sync::atomic::AtomicU64::new(8),
            sessions_file: Mutex::new(None),
            signin_fails: Mutex::new((0, 0)),
            epoch: std::sync::atomic::AtomicU64::new(0),
            fenced: Mutex::new(None),
            failover_key: Mutex::new(None),
        })
    }

    /// Carry on a run that was interrupted (the runtime was restarted) from what the log recorded.
    ///
    /// The recorded inputs are replayed through `chart` (the show file given now), which gives the scene the show
    /// was in and its context. The scene's clock continues from when it was entered, so cues still ahead are
    /// armed and cues that came due while the runtime was down are NOT fired late: they are listed instead.
    /// Not restored, on purpose: arming (lasers start disarmed again) and readiness (departments report again).
    /// Refused if the file no longer reproduces the recorded run (a transition was changed, say).
    pub fn resume(chart: Showchart, store: EventStore, output: Option<Output>) -> Result<(Arc<Self>, Value), String> {
        let records = store.all().map_err(|e| e.to_string())?;
        let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no earlier run in that database to resume")?;
        let mut engine = ShowEngine::new(chart).map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
        let mut entered = records[start].timestamp;
        let mut divergences = Vec::new();
        for r in &records[start + 1..] {
            match r.event_type.as_str() {
                recorder::SHOW_CONTEXT => {
                    if let Some(k) = r.payload["key"].as_str() {
                        engine.set_context(k, r.payload["value"].clone());
                    }
                }
                "show.edit" => {
                    // a live edit replaced the show: carry on with the show as it was then
                    if let Some(c) = r.payload.get("chart").and_then(|c| serde_json::from_value::<Showchart>(c.clone()).ok()) {
                        engine.chart = c;
                    }
                }
                recorder::SHOW_INPUT => {
                    let event = r.payload["event"].as_str().unwrap_or("");
                    let applied = engine.try_dispatch(event).is_some();
                    let (rec_applied, rec_to) = (r.payload["applied"].as_bool().unwrap_or(false), r.payload["to"].as_str().unwrap_or(""));
                    if applied != rec_applied || engine.current_state != rec_to {
                        divergences.push(format!("'{event}' went to {} but was recorded going to {}", engine.current_state, rec_to));
                    } else if applied {
                        entered = r.timestamp;
                    }
                }
                _ => {}
            }
        }
        if !divergences.is_empty() {
            return Err(format!("the show file no longer matches the recorded run ({}). Start without --resume for a fresh run, or use the version of the file the run started with", divergences[0]));
        }
        let now = now_ms();
        let elapsed = (now - entered).max(0) as u64;
        let last = records.last().map_or(now, |r| r.timestamp);
        let fired: std::collections::BTreeSet<String> = records
            .iter()
            .filter(|r| r.event_type == "show.cue" && r.timestamp >= entered && r.payload["scene"] == json!(engine.current_state))
            .filter_map(|r| r.payload["cue"].as_str().map(String::from))
            .collect();
        let skipped_cues: Vec<String> = engine.pending_cues().iter().filter(|c| c.at_ms <= elapsed && !fired.contains(&c.cue)).map(|c| format!("{} at {:.1}s", c.action.kind, c.at_ms as f64 / 1000.0)).collect();
        let skipped_timers: Vec<String> = engine.pending_timers().iter().filter(|&&ms| ms <= elapsed).map(|ms| format!("after {:.1}s", *ms as f64 / 1000.0)).collect();
        let info = json!({"state": engine.current_state, "elapsed_ms": elapsed, "downtime_ms": (now - last).max(0), "skipped_cues": skipped_cues, "skipped_timers": skipped_timers});
        let show_id = engine.chart.id.clone().unwrap_or_else(|| "show".into());
        let _ = store.append(now, "runtime", Some(&show_id), "show.resume", &info);
        let show = Self::assemble(engine, store, output, entered);
        *show.resumed.lock().unwrap() = Some(info.clone());
        show.watch_links();
        show.arm_from(0, elapsed);
        Ok((show, info))
    }

    /// The show a recorded run started with and the rig it last had, read from the log alone (a standby has no other source that
    /// is certain to be current: the show file it was started with may be older than the live edits).
    pub fn logged_show(records: &[pchi_eventstore::EventRecord]) -> Result<(Showchart, Option<Vec<rig::Device>>), String> {
        let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("the log holds no run yet")?;
        let chart: Showchart = serde_json::from_value(records[start].payload["chart"].clone()).map_err(|e| format!("the logged show is unreadable: {e}"))?;
        let devices = records[start..].iter().rev().filter(|r| r.event_type == "show.edit").find_map(|r| r.payload.get("devices").and_then(|d| serde_json::from_value::<Vec<rig::Device>>(d.clone()).ok()));
        Ok((chart, devices))
    }

    /// Keep an eye on the devices we send to, while the runtime is really sending: open the sockets of the
    /// ones expected to talk back, ask the ones that can answer a probe every few seconds, publish what they
    /// say as `feedback`, and publish a `link` message whenever a device's link state changes. Runs once;
    /// stops by itself when the show is dropped.
    fn watch_links(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;
        if self.output.lock().unwrap().is_none() || self.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut last: BTreeMap<String, String> = BTreeMap::new();
            let mut probed: BTreeMap<String, i64> = BTreeMap::new();
            let mut warned: std::collections::BTreeSet<String> = Default::default();
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Some(show) = weak.upgrade() else { break };
                let Some(out) = show.output.lock().unwrap().clone() else { continue };
                if !out.sending() {
                    continue;
                }
                let sink_show = Arc::downgrade(&show);
                out.links().set_sink(Arc::new(move |m| {
                    if let Some(s) = sink_show.upgrade() {
                        s.note_capture(&m);
                        s.publish(m);
                    }
                }));
                let now = now_ms();
                for dev in out.devices() {
                    if dev.kind == "obs" {
                        // OBS is held open by its own connection: just make sure it is being made
                        let _ = out.obs_ensure(dev).await;
                    } else if output::Output::expects_feedback(dev) {
                        match out.links().get(dev).await {
                            Err(e) => {
                                if warned.insert(dev.id.clone()) {
                                    show.publish(json!({"type": "error", "message": e}));
                                }
                            }
                            Ok(_) => {
                                if let Some(p) = profiles::probe(dev.profile_key()) {
                                    if now - probed.get(&dev.id).copied().unwrap_or(0) >= out.probe_every.as_millis() as i64 {
                                        probed.insert(dev.id.clone(), now);
                                        let _ = out.send_packets(dev, vec![p]).await;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(l) = out.link_state(dev) {
                        let st = l["state"].as_str().unwrap_or("none").to_string();
                        if last.get(&dev.id) != Some(&st) {
                            if last.contains_key(&dev.id) || st != "none" {
                                show.publish(json!({"type": "link", "ts": now, "device": dev.id, "label": dev.label, "state": st, "heard_ms_ago": l["heard_ms_ago"]}));
                            }
                            last.insert(dev.id.clone(), st);
                        }
                    }
                }
            }
        });
    }

    /// Require `key` for sending shows, undoing sends and test-firing. `None` (or empty) leaves them open.
    pub fn set_edit_key(&self, key: Option<String>) {
        *self.edit_key.lock().unwrap() = key.filter(|k| !k.is_empty());
    }

    /// Name the people who may sign in, each with their own key and a role. With a roster, running the show
    /// (GO, inputs) needs the right too, and the operator key still works as an operator.
    pub fn set_roster(&self, roster: governance::Roster) {
        *self.roster.lock().unwrap() = roster;
    }

    /// How long a signed-in session lasts (default 8 hours).
    pub fn set_session_hours(&self, h: u64) {
        self.session_hours.store(h.clamp(1, 168), std::sync::atomic::Ordering::Relaxed);
    }

    fn session_actor(&self, token: &str) -> Option<Actor> {
        let id = session_id(token);
        let mut s = self.sessions.lock().unwrap();
        match s.get(&id) {
            Some((a, exp)) if *exp > now_ms() => Some(a.clone()),
            Some(_) => {
                s.remove(&id);
                None
            }
            None => None,
        }
    }

    /// Keep sessions in this file (only a hash of each token, so reading the file does not let anyone in), and load the ones it holds:
    /// a restart then does not sign everyone out. A session is dropped when it has expired, when its person is no longer on the roster
    /// or has a different role, and when it was the operator key's and that key is gone. Call after the roster and edit key are set.
    /// Returns how many were restored.
    pub fn use_sessions_file(&self, path: &str) -> Result<usize, String> {
        *self.sessions_file.lock().unwrap() = Some(path.to_string());
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(format!("reading {path}: {e}")),
        };
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{path} is not a sessions file: {e}"))?;
        let now = now_ms();
        let mut kept = 0;
        {
            let roster = self.roster.lock().unwrap();
            let has_key = self.edit_key.lock().unwrap().is_some();
            let mut s = self.sessions.lock().unwrap();
            for e in v["sessions"].as_array().into_iter().flatten() {
                let (Some(id), Some(exp)) = (e["id"].as_str(), e["expires"].as_i64()) else { continue };
                if exp <= now {
                    continue;
                }
                let actor = match (e["kind"].as_str(), e["name"].as_str(), e["role"].as_str()) {
                    (Some("member"), Some(n), Some(r)) if roster.still_valid(n, r, now) => Actor::Member { name: n.into(), role: r.into() },
                    (Some("operator"), _, _) if has_key => Actor::Person(None),
                    _ => continue,
                };
                s.insert(id.to_string(), (actor, exp));
                kept += 1;
            }
        }
        self.save_sessions();
        Ok(kept)
    }

    /// Write the live sessions to the sessions file, if there is one (a temporary file renamed over it; owner-only on Linux and macOS).
    fn save_sessions(&self) {
        let Some(path) = self.sessions_file.lock().unwrap().clone() else { return };
        let now = now_ms();
        let list: Vec<Value> = self.sessions.lock().unwrap().iter().filter(|(_, (_, e))| *e > now).filter_map(|(id, (a, e))| match a {
            Actor::Member { name, role } => Some(json!({"id": id, "expires": e, "kind": "member", "name": name, "role": role})),
            Actor::Person(None) => Some(json!({"id": id, "expires": e, "kind": "operator"})),
            _ => None,
        }).collect();
        let tmp = format!("{path}.tmp");
        let body = serde_json::to_string(&json!({"sessions": list})).unwrap_or_default();
        let written = std::fs::write(&tmp, body).and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            }
            std::fs::rename(&tmp, &path)
        });
        if let Err(e) = written {
            self.publish(json!({"type": "error", "message": format!("could not save sessions to {path}: {e}")}));
        }
    }

    /// Trade a key for a session. The key is checked (a roster person's, or the operator key) and the answer is a token that lasts a
    /// few hours and can be ended with `sign_out`: Studio keeps the token, not the key. Ten wrong keys in a minute make sign-in wait.
    pub fn sign_in(&self, key: &str) -> Result<Value, (u16, String)> {
        let now = now_ms();
        {
            let mut f = self.signin_fails.lock().unwrap();
            if now - f.0 >= 60_000 {
                *f = (now, 0);
            }
            if f.1 >= 10 {
                return Err((429, "too many wrong keys: wait a minute".into()));
            }
        }
        let found = match self.roster.lock().unwrap().find_at(key, now) {
            Ok(Some(a)) => Ok(a),
            Ok(None) if self.edit_key.lock().unwrap().as_deref() == Some(key) => Ok(Actor::Person(None)),
            Ok(None) => Err("that key is not known".to_string()),
            Err(e) => Err(e),
        };
        let actor = match found {
            Ok(a) => a,
            Err(why) => {
                self.signin_fails.lock().unwrap().1 += 1;
                self.receipt("auth.signin", "DENY", &Actor::Anonymous, &format!("sign-in refused: {why}"));
                return Err((403, why));
            }
        };
        if actor.is_agent() {
            return Err((403, "an agent does not sign in: it sends its key with each request".into()));
        }
        use rand::RngCore;
        let mut raw = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut raw);
        let token = hex::encode(raw);
        let hours = self.session_hours.load(std::sync::atomic::Ordering::Relaxed);
        let expires = now + hours as i64 * 3_600_000;
        {
            let mut s = self.sessions.lock().unwrap();
            s.retain(|_, (_, e)| *e > now);
            s.insert(session_id(&token), (actor.clone(), expires));
        }
        self.save_sessions();
        self.receipt("auth.signin", "ALLOW", &actor, "signed in");
        Ok(json!({"token": token, "expires_in_s": hours * 3600, "name": actor.name(), "role": actor.role()}))
    }

    /// End a session at once.
    pub fn sign_out(&self, token: &str) -> bool {
        let gone = self.sessions.lock().unwrap().remove(&session_id(token));
        self.save_sessions();
        if let Some((a, _)) = &gone {
            self.receipt("auth.signout", "ALLOW", a, "signed out");
        }
        gone.is_some()
    }

    /// Keep an OSC input so its counters can be shown.
    pub fn add_input(&self, input: Arc<dyn inputs::InputInfo>) {
        self.inputs.lock().unwrap().push(input);
    }

    pub fn inputs_json(&self) -> Value {
        Value::Array(self.inputs.lock().unwrap().iter().map(|i| i.json()).collect())
    }

    pub fn roster_active(&self) -> bool {
        !self.roster.lock().unwrap().is_empty()
    }

    /// Whether a key (the operator key or a roster) is needed to change anything.
    pub fn is_locked(&self) -> bool {
        self.edit_key.lock().unwrap().is_some() || self.roster_active()
    }

    /// Who a caller really is. `declared` is what they said about themselves (the actor header): an agent stays an
    /// agent whatever key it holds; a roster key gives that person's name and role (their own label is ignored);
    /// the operator key gives an operator; with nothing valid, a locked show sees nobody and an open show a person.
    pub fn identify(&self, key: Option<&str>, declared: Actor) -> Actor {
        self.identify_with(None, key, declared)
    }

    /// As `identify`, but a live session token (from `sign_in`) counts first: it is the person who signed in.
    pub fn identify_with(&self, session: Option<&str>, key: Option<&str>, declared: Actor) -> Actor {
        if declared.is_agent() {
            return declared;
        }
        if let Some(a) = session.and_then(|t| self.session_actor(t)) {
            return a;
        }
        if let Some(k) = key {
            if let Ok(Some(found)) = self.roster.lock().unwrap().find_at(k, now_ms()) {
                return found;
            }
            if self.edit_key.lock().unwrap().as_deref() == Some(k) {
                return declared;
            }
        }
        if self.is_locked() {
            Actor::Anonymous
        } else {
            declared
        }
    }

    /// May `actor` do `perm`? `device_dept` is the department of the device it concerns: the laser safety officer
    /// may only try or test lasers.
    pub fn allow(&self, actor: &Actor, perm: Perm, device_dept: Option<&str>) -> bool {
        match actor {
            Actor::Agent(_) => false,
            Actor::Member { role, .. } => governance::role_allows(role, perm) && (role != "lso" || !matches!(perm, Perm::Try | Perm::Test) || device_dept == Some("laser")),
            Actor::Person(_) => true,
            // an operator-key-only show leaves GO open to the button box; a roster does not
            Actor::Anonymous => perm == Perm::Go && !self.roster_active(),
        }
    }

    /// The department of a rig device, if there is one with that id.
    pub fn device_dept(&self, id: &str) -> Option<String> {
        self.output.lock().unwrap().as_ref().and_then(|o| o.devices().iter().find(|d| d.id == id).map(|d| d.dept.clone()))
    }

    /// Whether `provided` may edit: always when no key is set.
    pub fn edit_ok(&self, provided: Option<&str>) -> bool {
        match &*self.edit_key.lock().unwrap() {
            None => true,
            Some(k) => provided == Some(k.as_str()),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }

    /// Full picture for a client that just connected.
    pub fn snapshot(&self) -> Value {
        let out = self.output.lock().unwrap().clone();
        let g = self.inner.lock().unwrap();
        let c = &g.engine.chart;
        json!({
            "type": "hello",
            "chart": { "id": c.id, "name": c.name, "version": c.version },
            "definition": c,
            "state": g.engine.current_state,
            "context": g.engine.context,
            "timers": g.engine.pending_timers(),
            "readiness": self.readiness_map(),
            "devices": out.as_ref().map(|o| Self::devices_json(o)).unwrap_or(Value::Null),
            "output": out.as_ref().map(|o| if o.sending() { "live" } else { "dry-run" }),
            // the server's clock now, so a client can tell how old each readiness report is without trusting its own clock
            "server_ts": now_ms(),
            "edit_locked": self.is_locked(),
            "roster": self.roster_active(),
            "inputs": self.inputs_json(),
            "timecode": self.timecode_json(),
            "captures": Value::Array(self.captures.lock().unwrap().iter().cloned().collect()),
            "polls": self.polls_json(),
            "resumed": self.resumed.lock().unwrap().clone(),
            "role": self.role_json(),
            "undo_depth": self.history.lock().unwrap().len(),
            "governed": self.gov.log.get().is_some(),
            "proposals": self.gov.pending_json(),
            // elapsed, not a timestamp: the client counts from its own clock
            "elapsed_ms": (now_ms() - g.entered_at).max(0),
        })
    }

    /// The rig for clients: each device, and for lasers whether they are armed.
    fn devices_json(out: &Output) -> Value {
        json!(out.devices().iter().map(|d| {
            let mut v = json!(d);
            if d.dept == "laser" {
                v["armed"] = json!(out.is_armed(&d.id));
            }
            if let Some(l) = out.link_state(d) {
                v["link"] = l;
            }
            v
        }).collect::<Vec<_>>())
    }

    fn readiness_map(&self) -> Value {
        json!(*self.readiness.lock().unwrap())
    }

    /// A department says it is (or is no longer) standing by. The name is
    /// normalised (lower case; letters, digits, - and _) so "Lighting " and
    /// "lighting" are the same department.
    pub fn set_ready(&self, dept: &str, ready: bool, note: &str, source: &str) -> Result<(), String> {
        let dept = dept.trim().to_lowercase();
        if dept.is_empty() || dept.len() > 32 || !dept.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("department must be 1-32 characters: letters, digits, - or _".into());
        }
        let note: String = note.chars().take(200).collect();
        let ts = now_ms();
        // Saying the same thing again (a department page checking in every few
        // seconds) refreshes the time but is not news: it is not written to the log.
        let heartbeat;
        {
            let mut r = self.readiness.lock().unwrap();
            // a stray client cannot grow the list forever
            if !r.contains_key(&dept) && r.len() >= 64 {
                return Err("too many departments".into());
            }
            heartbeat = r.get(&dept).is_some_and(|o| o["ready"] == json!(ready) && o["note"] == json!(note) && o["source"] == json!(source));
            r.insert(dept.clone(), json!({"ready": ready, "note": note, "ts": ts, "source": source}));
        }
        let msg = json!({"type": "readiness", "ts": ts, "dept": dept, "ready": ready, "note": note, "source": source, "heartbeat": heartbeat});
        if !heartbeat {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, source, Some(&show_id), "show.ready", &msg);
        }
        self.publish(msg);
        Ok(())
    }

    /// The log as records (for a recording).
    pub fn records(&self) -> Vec<pchi_eventstore::EventRecord> {
        self.inner.lock().unwrap().store.all().unwrap_or_default()
    }

    pub fn events(&self) -> Vec<Value> {
        let g = self.inner.lock().unwrap();
        g.store
            .all()
            .unwrap_or_default()
            .into_iter()
            .map(|r| json!({"id": r.id, "ts": r.timestamp, "source": r.source, "target": r.target, "type": r.event_type, "payload": r.payload}))
            .collect()
    }

    fn publish(&self, v: Value) {
        let _ = self.tx.send(v); // no listeners is fine
    }

    /// Feed an event to the show. Returns whether it caused a transition.
    pub fn apply(self: &Arc<Self>, source: &str, event: &str) -> bool {
        matches!(self.apply_checked(source, event), Applied::Moved)
    }

    /// Like `apply`, but says why nothing happened when the scene has not yet run its minimum: an input (not the
    /// show's own timer) that would move the scene on too early is HELD, meaning ignored, recorded and reported.
    pub fn apply_checked(self: &Arc<Self>, source: &str, event: &str) -> Applied {
        if self.is_fenced() {
            return Applied::Ignored; // another runtime is running the show
        }
        if source != "timer" && source != "timecode" {
            let held = {
                let g = self.inner.lock().unwrap();
                let min = g.engine.chart.state(&g.engine.current_state).and_then(|s| s.min_ms);
                let elapsed = (now_ms() - g.entered_at).max(0) as u64;
                match min {
                    Some(min) if elapsed < min && g.engine.clone().try_dispatch(event).is_some() => Some((min - elapsed, min, g.engine.current_state.clone())),
                    _ => None,
                }
            };
            if let Some((remaining_ms, min_ms, scene)) = held {
                let ts = now_ms();
                {
                    let g = self.inner.lock().unwrap();
                    let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
                    let _ = g.store.append(ts, source, Some(&show_id), "show.held", &json!({"event": event, "scene": scene, "remaining_ms": remaining_ms, "min_ms": min_ms}));
                }
                self.publish(json!({"type": "held", "ts": ts, "event": event, "scene": scene, "remaining_ms": remaining_ms, "min_ms": min_ms, "source": source}));
                return Applied::Held { scene, remaining_ms, min_ms };
            }
        }
        if self.apply_inner(source, event) {
            Applied::Moved
        } else {
            Applied::Ignored
        }
    }

    fn apply_inner(self: &Arc<Self>, source: &str, event: &str) -> bool {
        let ts = now_ms();
        let mut out = Vec::new();
        let mut armed = None;
        let mut routed: Vec<Action> = Vec::new();
        let applied;
        {
            let mut g = self.inner.lock().unwrap();
            let from = g.engine.current_state.clone();
            let inner = &mut *g;
            match recorder::record_input(&inner.store, &mut inner.engine, source, event, ts) {
                Ok(res) => {
                    applied = res.is_some();
                    out.push(json!({"type": "transition", "ts": ts, "source": source, "event": event,
                                    "from": from, "to": g.engine.current_state, "applied": applied}));
                    for a in res.unwrap_or_default() {
                        out.push(json!({"type": "action", "ts": ts, "event": a.to_event()}));
                        routed.push(a);
                    }
                    if applied {
                        g.generation += 1;
                        g.entered_at = ts;
                        armed = Some(g.generation);
                    }
                }
                Err(e) => {
                    applied = false;
                    out.push(json!({"type": "error", "message": e.to_string()}));
                }
            }
        }
        out.into_iter().for_each(|m| self.publish(m));
        for a in &routed {
            self.route(ts, a, None, None, None, None);
        }
        if let Some(generation) = armed {
            self.arm(generation);
        }
        applied
    }

    pub fn set_context(&self, key: &str, value: Value) {
        let ts = now_ms();
        let res = {
            let mut g = self.inner.lock().unwrap();
            let inner = &mut *g;
            recorder::record_context(&inner.store, &mut inner.engine, key, value.clone(), ts)
        };
        match res {
            Ok(()) => self.publish(json!({"type": "context", "ts": ts, "key": key, "value": value})),
            Err(e) => self.publish(json!({"type": "error", "message": e.to_string()})),
        }
    }

    /// Start a fresh recorded run from the initial state.
    pub fn reset(self: &Arc<Self>) {
        let generation = {
            let mut g = self.inner.lock().unwrap();
            let chart = g.engine.chart.clone();
            let ts = now_ms();
            let Ok(engine) = ShowEngine::new(chart) else { return };
            let _ = recorder::begin_run(&g.store, &engine, ts);
            let _ = recorder::record_start(&g.store, &engine, ts);
            g.engine = engine;
            g.generation += 1;
            g.entered_at = ts;
            g.generation
        };
        self.publish(self.snapshot());
        self.arm(generation);
    }

    /// Schedules everything the current state does over time, measured from now
    /// (its entry): one task per timed transition and one per timeline cue. Each
    /// task does nothing if the show has left the state it was armed for.
    fn arm(self: &Arc<Self>, generation: u64) {
        self.arm_from(generation, 0)
    }

    /// As `arm`, for a state that has already been running for `elapsed` ms (after a live edit):
    /// what is already past is not fired late, and the rest keeps its original time.
    fn arm_from(self: &Arc<Self>, generation: u64, elapsed: u64) {
        let (timers, cues) = {
            let g = self.inner.lock().unwrap();
            // a scene locked to timecode has its cues fired by the timecode (see `timecode_update`), not by this clock
            let locked = g.engine.chart.state(&g.engine.current_state).and_then(|s| s.timeline.as_ref()).is_some_and(|t| t.tc_start.is_some());
            (g.engine.pending_timers(), if locked { Vec::new() } else { g.engine.pending_cues() })
        };
        for ms in timers.into_iter().filter(|&ms| elapsed == 0 || ms > elapsed) {
            let show = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms - elapsed)).await;
                if show.inner.lock().unwrap().generation == generation {
                    show.apply("timer", &timer_event(ms));
                }
            });
        }
        for cue in cues.into_iter().filter(|c| elapsed == 0 || c.at_ms > elapsed) {
            let show = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(cue.at_ms - elapsed)).await;
                if show.inner.lock().unwrap().generation == generation {
                    show.fire_cue(&cue);
                }
            });
        }
    }

    /// Swap in an edited show while the current one runs. With `apply` false it only
    /// reports what would change. The running scene, its clock and the context are
    /// kept; timers and cues are re-armed for the new definition from where the scene is now.
    /// Refused (Err) if the new show is invalid or no longer has the running scene.
    pub fn replace(self: &Arc<Self>, chart: Showchart, devices: Option<Vec<rig::Device>>, apply: bool) -> Result<Value, (u16, Value)> {
        self.replace_with(chart, devices, apply, false)
    }

    /// Put the show back as it was before the last send. `apply` false previews what would change.
    pub fn undo(self: &Arc<Self>, apply: bool) -> Result<Value, (u16, Value)> {
        let (chart, devices) = self.history.lock().unwrap().last().cloned().ok_or((409u16, json!({"error": "nothing to undo"})))?;
        let r = self.replace_with(chart, devices, apply, true)?;
        if apply {
            self.history.lock().unwrap().pop();
            let mut r = r;
            r["undo_depth"] = json!(self.history.lock().unwrap().len());
            return Ok(r);
        }
        Ok(r)
    }

    fn replace_with(self: &Arc<Self>, chart: Showchart, devices: Option<Vec<rig::Device>>, apply: bool, is_undo: bool) -> Result<Value, (u16, Value)> {
        let candidate = ShowEngine::new(chart.clone()).map_err(|errs| (422u16, json!({"error": errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")})))?;
        drop(candidate);
        let old_dev: Vec<rig::Device> = self.output.lock().unwrap().as_ref().map(|o| o.devices().to_vec()).unwrap_or_default();
        let new_dev = devices.clone().unwrap_or_else(|| old_dev.clone());
        let (d, generation, elapsed) = {
            let mut g = self.inner.lock().unwrap();
            let elapsed = (now_ms() - g.entered_at).max(0) as u64;
            let mut d = diff::diff(&g.engine.chart, &chart, &old_dev, &new_dev, &g.engine.current_state, elapsed);
            // edited cues still to fire in the running scene: is the device they go to ready?
            {
                let ready = self.readiness.lock().unwrap();
                let now = now_ms();
                let state = g.engine.current_state.clone();
                for (scene, lane_id, cue_id) in &d.touched {
                    if *scene != state {
                        continue;
                    }
                    let Some(lane) = chart.state(scene).and_then(|s| s.timeline.as_ref()).and_then(|t| t.lanes.iter().find(|l| l.id == *lane_id)) else { continue };
                    let Some(cue) = lane.cues.iter().find(|c| c.id == *cue_id) else { continue };
                    if cue.at_ms <= elapsed {
                        continue;
                    }
                    let dept = lane.dept.clone().unwrap_or_else(|| rig::dept_of_kind(&cue.action.kind).to_string());
                    if dept == "other" || new_dev.is_empty() {
                        continue;
                    }
                    let Ok(dev) = rig::resolve(&new_dev, &dept, lane.device.as_deref()) else { continue };
                    let word = ready.get(&dev.id).or_else(|| ready.get(&dev.dept));
                    let fresh = word.filter(|w| !(w["source"] == "dept-page" && now - w["ts"].as_i64().unwrap_or(0) > STALE_MS));
                    let what = cue.label.clone().unwrap_or_else(|| cue.action.kind.clone());
                    match fresh {
                        Some(w) if w["ready"] == json!(false) => d.notes.push(format!("{} is on hold: the edited cue \"{what}\" at {:.1}s will still be sent to it", dev.label, cue.at_ms as f64 / 1000.0)),
                        None => d.notes.push(format!("{} has not reported READY: the edited cue \"{what}\" at {:.1}s goes to it anyway", dev.label, cue.at_ms as f64 / 1000.0)),
                        _ => {}
                    }
                }
            }
            if !d.blockers.is_empty() {
                return Err((409, json!({"error": d.blockers.join("; "), "diff": d})));
            }
            if !apply {
                return Ok(json!({"applied": false, "diff": d}));
            }
            if !is_undo {
                let mut h = self.history.lock().unwrap();
                h.push((g.engine.chart.clone(), self.output.lock().unwrap().as_ref().map(|o| o.devices().to_vec())));
                if h.len() > UNDO_DEPTH {
                    h.remove(0);
                }
            }
            g.engine.chart = chart;
            g.generation += 1;
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let ts = now_ms();
            let _ = g.store.append(ts, "studio", Some(&show_id), "show.edit", &json!({"changes": d.changes, "notes": d.notes, "undo": is_undo, "chart": g.engine.chart, "devices": new_dev}));
            (d, g.generation, elapsed)
        };
        if devices.is_some() {
            let sending = self.output.lock().unwrap().as_ref().is_some_and(|o| o.sending());
            let mut o = self.output.lock().unwrap();
            let next = match o.as_ref() {
                Some(old) => old.with_devices(new_dev),
                None => Output::new(new_dev, sending),
            };
            *o = Some(Arc::new(next));
        }
        self.watch_links();
        let hello = self.snapshot();
        self.publish(json!({"type": "show", "ts": now_ms(), "definition": hello["definition"], "devices": hello["devices"], "output": hello["output"], "undo_depth": hello["undo_depth"], "diff": d}));
        self.supersede();
        self.arm_from(generation, elapsed);
        Ok(json!({"applied": true, "diff": d, "undo_depth": self.history.lock().unwrap().len()}))
    }

    /// Send one harmless message (`/pchi/test`) to a device, so the operator can check the link before doors.
    /// Reported like any delivery. In a dry run nothing is sent.
    pub async fn test_fire(self: &Arc<Self>, id: &str) -> Result<Value, String> {
        let out = self.output.lock().unwrap().clone().ok_or("this show has no rig to test")?;
        let dev = out.devices().iter().find(|d| d.id == id).cloned().ok_or_else(|| format!("no device '{id}' in the rig"))?;
        let mut m = json!({"type": "delivery", "ts": now_ms(), "kind": "test", "lane": null, "cue": null,
                           "device": dev.id, "label": dev.label, "dept": dev.dept, "target": dev.address()});
        if dev.dept == "laser" && !out.is_armed(&dev.id) {
            m["status"] = json!("held");
            m["error"] = json!(format!("{} is not armed: arm it first", dev.label));
        } else if !out.sending() {
            m["status"] = json!("dry-run");
            m["note"] = json!("sending is off: start the runtime with --send");
        } else {
            match out.deliver(&dev, &Action::new("test", json!({}))).await {
                Ok(_) => m["status"] = json!("sent"),
                Err(e) => {
                    m["status"] = json!("failed");
                    m["error"] = json!(e);
                }
            }
        }
        self.report(m.clone());
        Ok(m)
    }

    /// Arm or disarm a laser device. Disarming is always allowed; arming is the operator's decision (the
    /// HTTP route needs the operator key). A laser gets no cues until armed. Arming never switches the
    /// laser's own output on: that stays at the laser software.
    pub fn set_armed(&self, id: &str, armed: bool) -> Result<(), String> {
        let out = self.output.lock().unwrap().clone().ok_or("this show has no rig")?;
        if !out.set_armed(id, armed) {
            return Err(format!("no laser device '{id}' in the rig"));
        }
        self.publish(json!({"type": "arming", "ts": now_ms(), "device": id, "armed": armed}));
        Ok(())
    }

    /// The hard stop: disarm every laser (so nothing more is sent) and, for tools with a known safe
    /// message (BEYOND: blackout and disable output), tell them to go dark. Anyone with the feed token may use it.
    pub fn laser_stop(self: &Arc<Self>) -> Value {
        let Some(out) = self.output.lock().unwrap().clone() else { return json!({"stopped": [], "notes": ["this show has no rig"]}) };
        let ts = now_ms();
        let mut stopped = Vec::new();
        let mut notes = Vec::new();
        for dev in out.lasers() {
            out.set_armed(&dev.id, false);
            self.publish(json!({"type": "arming", "ts": ts, "device": dev.id, "armed": false}));
            stopped.push(dev.id.clone());
            let msgs = profiles::stop_messages(dev.profile_key());
            let base = json!({"type": "delivery", "ts": ts, "kind": "laser.stop", "lane": null, "cue": null, "device": dev.id, "label": dev.label, "dept": dev.dept, "target": dev.address()});
            if msgs.is_empty() {
                notes.push(format!("{}: disarmed here, but no stop message is known for this tool: use its own stop", dev.label));
            } else if !out.sending() {
                let mut m = base;
                m["status"] = json!("dry-run");
                m["note"] = json!("hard stop: would send blackout and disable output");
                self.report(m);
            } else {
                let show = Arc::clone(self);
                let out = Arc::clone(&out);
                tokio::spawn(async move {
                    let mut m = base;
                    match out.send_packets(&dev, msgs).await {
                        Ok(_) => m["status"] = json!("sent"),
                        Err(e) => {
                            m["status"] = json!("failed");
                            m["error"] = json!(e);
                        }
                    }
                    show.report(m);
                });
            }
        }
        json!({"stopped": stopped, "notes": notes})
    }

    /// The doors check: every cue against its device and profile, every device's address, link,
    /// readiness and (for lasers) arming, as a list the operator can read in a minute.
    pub fn preflight(&self) -> Value {
        let out = self.output.lock().unwrap().clone();
        let (chart, state) = {
            let g = self.inner.lock().unwrap();
            (g.engine.chart.clone(), g.engine.current_state.clone())
        };
        let ready: BTreeMap<String, Value> = self.readiness.lock().unwrap().clone();
        let mut checks = preflight::run(&chart, out.as_deref(), &ready, &state);
        let uses_tc = chart.states.iter().any(|s| s.timeline.as_ref().is_some_and(|t| t.tc_start.is_some())) || chart.transitions.iter().any(|t| t.event.as_deref().is_some_and(|e| e.starts_with("tc:")));
        if uses_tc {
            let tc = self.timecode_json();
            let text = match tc["state"].as_str() {
                Some("running") => format!("timecode {} is running ({} fps, from {})", tc["position"].as_str().unwrap_or("?"), tc["fps"].as_str().unwrap_or("?"), tc["source"].as_str().unwrap_or("?")),
                Some("stopped") => format!("timecode is stopped at {} (or its source went quiet): cues locked to it will not fire until it runs", tc["position"].as_str().unwrap_or("?")),
                _ => "this show locks cues or scene changes to timecode, but no timecode has been heard: start the source, or add an MTC or OSC timecode input".to_string(),
            };
            let level = if tc["state"] == "running" { "ok" } else { "warn" };
            checks.push(preflight::Check { level, area: "timecode", text, device: None, scene: None });
        }
        json!({"summary": preflight::summary(&checks), "checks": checks})
    }

    /// Send ONE cue to ONE device through its profile, without any show: the quickest way to find out whether
    /// a tool answers to what PCHI sends. Returns exactly what was (or in a dry run would be) sent.
    /// Lasers are held until armed, as everywhere else.
    pub async fn try_cue(self: &Arc<Self>, id: &str, value: &str) -> Result<Value, String> {
        let out = self.output.lock().unwrap().clone().ok_or("this show has no rig")?;
        let dev = out.devices().iter().find(|d| d.id == id).cloned().ok_or_else(|| format!("no device '{id}' in the rig"))?;
        let value = value.trim();
        if value.is_empty() || value.len() > 200 {
            return Err("give a value, for example a cue number".into());
        }
        let (kind, field) = match dev.dept.as_str() {
            "visual" => ("scene.activate", "scene"),
            "lighting" => ("lighting.look", "look"),
            "laser" => ("laser.cue", "cue"),
            _ => ("audio.snapshot", "snapshot"),
        };
        let action = Action::new(kind, json!({ field: value }));
        let sent_as: Vec<Value> = output::wire_lines(&dev, &action)?.into_iter().map(|(a, args)| json!({"address": a, "args": args})).collect();
        let packets = if dev.is_direct() { Vec::new() } else { output::packets_for(&dev, &action)? };
        let mut m = json!({"type": "delivery", "ts": now_ms(), "kind": kind, "lane": null, "cue": null, "device": dev.id, "label": dev.label, "dept": dev.dept, "target": dev.address(), "sent_as": sent_as, "manual": true});
        if dev.dept == "laser" && !out.is_armed(&dev.id) {
            m["status"] = json!("held");
            m["error"] = json!(format!("{} is not armed: arm it first", dev.label));
        } else if !out.sending() {
            m["status"] = json!("dry-run");
            m["note"] = json!("sending is off: this is exactly what WOULD be sent");
        } else {
            match if dev.is_direct() { out.deliver(&dev, &action).await } else { out.send_packets(&dev, packets).await } {
                Ok(_) => m["status"] = json!("sent"),
                Err(e) => {
                    m["status"] = json!("failed");
                    m["error"] = json!(e);
                }
            }
        }
        self.report(m.clone());
        Ok(m)
    }

    pub fn current_state(&self) -> String {
        self.inner.lock().unwrap().engine.current_state.clone()
    }

    /// Play `chart` against the run recorded so far, on a virtual clock, and say what would differ. Changes nothing.
    /// With no rig of its own the candidate is checked against the running rig.
    pub fn rehearse(&self, chart: Showchart, devices: Option<Vec<rig::Device>>) -> Result<Value, String> {
        let dev = devices.unwrap_or_else(|| self.output.lock().unwrap().as_ref().map(|o| o.devices().to_vec()).unwrap_or_default());
        let records = self.inner.lock().unwrap().store.all().map_err(|e| e.to_string())?;
        rehearse::run(&records, &chart, &dev, now_ms())
    }

    /// A message a tool sent back. If its profile says that message means "I fired this cue", remember it as a capture:
    /// where in the show it was heard, and the cue that would fire the same thing. Studio offers to add it to the show.
    pub fn note_capture(&self, m: &Value) {
        if m["type"] != "feedback" {
            return;
        }
        let (Some(id), Some(address)) = (m["device"].as_str(), m["address"].as_str()) else { return };
        let Some(out) = self.output.lock().unwrap().clone() else { return };
        let Some(dev) = out.devices().iter().find(|d| d.id == id) else { return };
        let Some((kind, value, basis)) = profiles::capture(dev.profile_key(), address) else { return };
        let ts = now_ms();
        let (scene, at_ms) = {
            let g = self.inner.lock().unwrap();
            (g.engine.current_state.clone(), (ts - g.entered_at).max(0))
        };
        let mut caps = self.captures.lock().unwrap();
        // a tool that repeats itself is one cue, not several
        if caps.back().is_some_and(|c| c["device"] == json!(id) && c["kind"] == json!(kind) && c["value"] == json!(value) && ts - c["ts"].as_i64().unwrap_or(0) < 400) {
            return;
        }
        let n = caps.back().and_then(|c| c["id"].as_u64()).unwrap_or(0) + 1;
        let rec = json!({"type": "capture", "id": n, "ts": ts, "device": dev.id, "label": dev.label, "dept": dev.dept, "kind": kind, "value": value,
                         "address": address, "scene": scene, "at_ms": at_ms, "basis": basis.word()});
        caps.push_back(rec.clone());
        while caps.len() > 30 {
            caps.pop_front();
        }
        drop(caps);
        self.publish(rec);
    }

    pub fn captures_json(&self) -> Value {
        Value::Array(self.captures.lock().unwrap().iter().cloned().collect())
    }

    /// The token the audience's gateway votes with. Without one, voting is off. It opens `POST /vote` and nothing else.
    pub fn set_audience_token(&self, t: Option<String>) {
        *self.audience_token.lock().unwrap() = t.filter(|t| !t.is_empty());
    }

    pub fn audience_ok(&self, provided: Option<&str>) -> bool {
        matches!(&*self.audience_token.lock().unwrap(), Some(t) if provided == Some(t.as_str()))
    }

    pub fn audience_enabled(&self) -> bool {
        self.audience_token.lock().unwrap().is_some()
    }

    pub fn polls_json(&self) -> Value {
        Value::Array(self.votes.lock().unwrap().all().iter().map(audience::tally_json).collect())
    }

    /// Put every poll's tally where the show can read it, as one object: `vote.<poll>.<option>`, `.total`, `.winner` (the option's key,
    /// or ""), `.open`. A guard's dotted names walk into objects, so `vote.next.winner == "hype"` works.
    fn publish_poll(&self, t: &audience::Tally) {
        let mut all = serde_json::Map::new();
        for p in self.votes.lock().unwrap().all() {
            let mut o = serde_json::Map::new();
            for (opt, n) in &p.counts {
                o.insert(audience::key_of(opt), json!(n));
            }
            o.insert("total".into(), json!(p.total));
            o.insert("winner".into(), json!(p.winner.as_deref().map(audience::key_of).unwrap_or_default()));
            o.insert("open".into(), json!(p.open));
            all.insert(p.id.clone(), Value::Object(o));
        }
        self.set_context("vote", Value::Object(all));
        self.publish(audience::tally_json(t));
    }

    /// A person opens a poll.
    pub fn poll_open(&self, actor: &Actor, id: &str, options: &[String]) -> Result<Value, String> {
        let t = self.votes.lock().unwrap().open(id, options)?;
        self.poll_pub.lock().unwrap().remove(id);
        self.publish_poll(&t);
        self.receipt("poll.open", "ALLOW", actor, &format!("opened poll '{id}' with {}", options.join(", ")));
        Ok(audience::tally_json(&t))
    }

    /// A person closes it: the result stays in the context for the show to read.
    pub fn poll_close(&self, actor: &Actor, id: &str) -> Result<Value, String> {
        let t = self.votes.lock().unwrap().close(id)?;
        self.poll_pub.lock().unwrap().remove(id);
        self.publish_poll(&t);
        let said = t.counts.iter().map(|(o, n)| format!("{o} {n}")).collect::<Vec<_>>().join(", ");
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(now_ms(), "runtime", Some(&show_id), "show.poll", &json!({"poll": id, "total": t.total, "winner": t.winner, "counts": said}));
        }
        self.receipt("poll.close", "ALLOW", actor, &format!("closed poll '{id}': {said}; {}", t.winner.as_deref().map_or("no winner".to_string(), |w| format!("winner {w}"))));
        Ok(audience::tally_json(&t))
    }

    /// One audience vote, from the gateway. Votes are counted at once but put into the show's context at most four times a second.
    pub fn cast_vote(self: &Arc<Self>, poll: &str, option: &str, voter: &str) -> Result<Value, (u16, String)> {
        {
            let now = now_ms();
            let mut w = self.vote_window.lock().unwrap();
            if now - w.0 >= 1000 {
                *w = (now, 0);
            }
            w.1 += 1;
            if w.1 > 300 {
                return Err((429, "too many votes a second: slow down".into()));
            }
        }
        let t = self.votes.lock().unwrap().vote(poll, option, voter).map_err(|e| (if e.contains("no poll") { 404 } else if e.contains("closed") { 409 } else { 400 }, e))?;
        let now = now_ms();
        let wait = {
            let mut m = self.poll_pub.lock().unwrap();
            let e = m.entry(poll.to_string()).or_insert((0, false));
            if now - e.0 >= 250 {
                e.0 = now;
                Some(0)
            } else if !e.1 {
                e.1 = true;
                Some(250 - (now - e.0))
            } else {
                None
            }
        };
        match wait {
            Some(0) => self.publish_poll(&t),
            Some(ms) => {
                let show = Arc::clone(self);
                let id = poll.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
                    if let Some(e) = show.poll_pub.lock().unwrap().get_mut(&id) {
                        e.0 = now_ms();
                        e.1 = false;
                    }
                    let latest = show.votes.lock().unwrap().tally(&id);
                    if let Some(t) = latest {
                        show.publish_poll(&t);
                    }
                });
            }
            None => {}
        }
        Ok(json!({"ok": true, "total": t.total}))
    }

    /// What the timecode is doing, for clients: {state: none|running|stopped, position, fps, source, heard_ms_ago}.
    pub fn timecode_json(&self) -> Value {
        let c = self.tc.lock().unwrap();
        let st = c.status(now_ms());
        let position = st.fps.map(|f| {
            let frames = (st.position_ms / f.frame_ms()).round().max(0.0) as u64;
            timecode::Tc::from_frames(frames, f).display()
        });
        json!({"state": st.state, "position_ms": st.position_ms, "position": position, "fps": st.fps.map(|f| f.label()), "source": c.source, "heard_ms_ago": st.heard_ms_ago})
    }

    /// A new timecode position from a source (MIDI Time Code, an OSC message). Cues of the running scene that is locked to
    /// timecode, and `tc:` transitions out of it, fire as the position passes them. A jump (a locate) fires nothing: it is
    /// reported, with how many cues were jumped over, and going back lets cues fire again as the position passes them.
    pub fn timecode_update(self: &Arc<Self>, position_ms: f64, fps: Fps, source: &str) {
        let now = now_ms();
        let step = self.tc.lock().unwrap().observe(position_ms, fps, now, source);
        match step {
            TcStep::Forward { from, to } => self.tc_advance(from, to, fps),
            TcStep::Jump { from, to } => {
                let skipped = if to > from { self.tc_due(from, to, fps, false).len() } else { 0 };
                let (scene, show_id) = {
                    let g = self.inner.lock().unwrap();
                    (g.engine.current_state.clone(), g.engine.chart.id.clone().unwrap_or_else(|| "show".into()))
                };
                let m = json!({"type": "tc-jump", "ts": now, "from_ms": from, "to_ms": to, "scene": scene, "skipped": skipped, "source": source});
                let _ = self.inner.lock().unwrap().store.append(now, source, Some(&show_id), "show.tc_jump", &m);
                self.publish(m);
            }
            _ => {}
        }
        // clients get the position at most five times a second
        let last = self.tc_published.load(std::sync::atomic::Ordering::Relaxed);
        if now - last >= 200 && self.tc_published.compare_exchange(last, now, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed).is_ok() {
            let mut m = self.timecode_json();
            m["type"] = json!("timecode");
            m["ts"] = json!(now);
            self.publish(m);
        }
    }

    /// What the timecode passing `(from, to]` makes due in the running scene, earliest first, cues before transitions at the same
    /// position. With `inclusive` the start counts too (for a scene just entered at that position).
    fn tc_due(&self, from: f64, to: f64, fps: Fps, inclusive: bool) -> Vec<(f64, TcDue)> {
        let g = self.inner.lock().unwrap();
        let scene = g.engine.current_state.clone();
        let inside = |p: f64| (p > from || (inclusive && p >= from)) && p <= to;
        let mut due: Vec<(f64, TcDue)> = Vec::new();
        if let Some(tl) = g.engine.chart.state(&scene).and_then(|s| s.timeline.as_ref()) {
            if let Some(start) = tl.tc_start.as_deref().and_then(|t| timecode::written_to_ms(t, fps).ok()) {
                for c in g.engine.pending_cues() {
                    let p = start + c.at_ms as f64;
                    if inside(p) {
                        due.push((p, TcDue::Cue(c, start)));
                    }
                }
            }
        }
        for t in g.engine.chart.transitions.iter().filter(|t| t.source == scene) {
            if let Some(ev) = t.event.as_deref().filter(|e| e.starts_with("tc:")) {
                if let Ok(p) = timecode::written_to_ms(&ev[3..], fps) {
                    if inside(p) {
                        due.push((p, TcDue::Event(ev.to_string())));
                    }
                }
            }
        }
        due.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| matches!(a.1, TcDue::Event(_)).cmp(&matches!(b.1, TcDue::Event(_)))));
        due
    }

    /// The position moved forward normally from `from` to `to`: fire what it passed, following the show if it changes scene.
    fn tc_advance(self: &Arc<Self>, from: f64, to: f64, fps: Fps) {
        let (mut lo, mut inclusive) = (from, false);
        for _ in 0..50 {
            let mut moved_at = None;
            for (pos, item) in self.tc_due(lo, to, fps, inclusive) {
                match item {
                    TcDue::Cue(cue, start) => self.fire_cue_at(&cue, Some((pos - start).round() as i64)),
                    TcDue::Event(ev) => {
                        if self.apply_checked("timecode", &ev) == Applied::Moved {
                            moved_at = Some(pos);
                            break;
                        }
                    }
                }
            }
            match moved_at {
                // the scene changed at `pos`: the new scene's timecode cues from that moment on are due too
                Some(pos) => (lo, inclusive) = (pos, true),
                None => return,
            }
        }
    }

    /// Turn on signed receipts: every decision from now on is written to `log`.
    pub fn set_governance(&self, log: primeswarm_pchi::GovernanceLog) {
        let _ = self.gov.log.set(log);
    }

    /// Hash of the show as it is now (scene, definition, which lasers are armed): what a receipt is decided against.
    fn state_hash(&self) -> String {
        let out = self.output.lock().unwrap().clone();
        let armed: Vec<String> = out.map(|o| o.lasers().into_iter().filter(|d| o.is_armed(&d.id)).map(|d| d.id).collect()).unwrap_or_default();
        let g = self.inner.lock().unwrap();
        primeswarm_pchi::governance::hash_state(&json!({"scene": g.engine.current_state, "show": primeswarm_pchi::governance::hash_state(&json!(g.engine.chart)), "armed": armed}))
    }

    /// Hash of the show and its rig, to tell whether a proposal still applies to what is running.
    fn base_hash(&self) -> String {
        let dev: Vec<rig::Device> = self.output.lock().unwrap().as_ref().map(|o| o.devices().to_vec()).unwrap_or_default();
        let g = self.inner.lock().unwrap();
        primeswarm_pchi::governance::hash_state(&json!({"show": g.engine.chart, "rig": dev}))
    }

    /// Write one decision to the signed log: `gate` is ALLOW, DENY or ESCALATE (waiting on a person).
    /// Returns the receipt id, or None when receipts are not turned on. Never call with the inner lock held.
    pub fn receipt(&self, rule: &str, gate: &str, actor: &Actor, text: &str) -> Option<String> {
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(now_ms(), "runtime", Some(&show_id), "show.decision", &json!({"ts": now_ms(), "rule": rule, "gate": gate, "by": actor.name(), "text": text}));
        }
        let log = self.gov.log.get()?;
        Some(log.record(rule, gate, &format!("{}: {}", actor.name(), text), &self.state_hash()).receipt_id)
    }

    /// The receipts so far, in the format `scripts/verify_governance_chain.py` checks offline.
    pub fn governance_export(&self) -> Value {
        match self.gov.log.get() {
            Some(l) => l.export(),
            None => json!({"receipts": [], "count": 0, "verifying_key": null, "note": "receipts are off: start the runtime with --governance <folder>"}),
        }
    }

    pub fn proposals(&self) -> Value {
        self.gov.all_json()
    }

    /// An agent's edit: checked like any send, but only stored. A person approves or rejects it.
    pub fn propose(self: &Arc<Self>, actor: &Actor, chart: Showchart, devices: Option<Vec<rig::Device>>, said: &str) -> Result<Value, (u16, Value)> {
        let preview = self.replace(chart.clone(), devices.clone(), false)?;
        let strings = |k: &str| preview["diff"][k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
        let (changes, notes) = (strings("changes"), strings("notes"));
        if changes.is_empty() {
            return Err((409, json!({"error": "nothing to propose: that is the show already running"})));
        }
        let base = self.base_hash();
        let ts = now_ms();
        // how it would have gone against the run so far, so the person deciding sees the effect, not just the edit
        let rehearsal = self.rehearse(chart.clone(), devices.clone()).map(|r| json!({"different": r["summary"]["different"], "verdict": r["summary"]["verdict"], "lines": r["summary"]["lines"]})).unwrap_or(Value::Null);
        let (id, view) = {
            let mut ps = self.gov.proposals.lock().unwrap();
            let id = ps.iter().map(|p| p.id).max().unwrap_or(0) + 1;
            let receipt = self.receipt("show.propose", "ESCALATE", actor, &format!("proposed edit #{id} ({} change(s)){}; waiting for a person", changes.len(), said_suffix(said)));
            ps.push(Proposal { id, actor: actor.name(), ts, status: "pending".into(), changes, notes, receipt, said: said.to_string(), rehearsal, base, chart, devices });
            while ps.len() > governance::MAX_PROPOSALS {
                match ps.iter().position(|p| p.status != "pending") {
                    Some(i) => {
                        ps.remove(i);
                    }
                    None => break,
                }
            }
            (id, ps.iter().find(|p| p.id == id).map(Proposal::json).unwrap_or(Value::Null))
        };
        self.publish(json!({"type": "proposal", "ts": ts, "proposal": view}));
        Ok(json!({"proposed": true, "id": id, "status": "pending", "proposal": view, "diff": preview["diff"]}))
    }

    /// A person approves a proposal: it is applied now, exactly as a send. Refused if it is no longer pending.
    pub fn approve(self: &Arc<Self>, id: u32, actor: &Actor) -> Result<Value, (u16, Value)> {
        let (chart, devices, by, changes, said) = {
            let mut ps = self.gov.proposals.lock().unwrap();
            let p = ps.iter_mut().find(|p| p.id == id).ok_or((404u16, json!({"error": format!("no proposal #{id}")})))?;
            if p.status != "pending" {
                return Err((409, json!({"error": format!("proposal #{id} is {}, not pending", p.status), "status": p.status})));
            }
            p.status = "approving".into();
            (p.chart.clone(), p.devices.clone(), p.actor.clone(), p.changes.len(), p.said.clone())
        };
        let set = |status: &str| {
            if let Some(p) = self.gov.proposals.lock().unwrap().iter_mut().find(|p| p.id == id) {
                p.status = status.into();
            }
        };
        match self.replace(chart, devices, true) {
            Ok(mut v) => {
                set("approved");
                v["approved"] = json!(id);
                v["receipt"] = json!(self.receipt("show.edit", "ALLOW", actor, &format!("approved proposal #{id} from {by} and applied it ({changes} change(s)){}", said_suffix(&said))));
                let view = self.gov.proposals.lock().unwrap().iter().find(|p| p.id == id).map(Proposal::json);
                self.publish(json!({"type": "proposal", "ts": now_ms(), "proposal": view}));
                Ok(v)
            }
            Err(e) => {
                set("pending");
                self.receipt("show.edit", "DENY", actor, &format!("could not apply proposal #{id} from {by}: {}", e.1["error"].as_str().unwrap_or("refused")));
                Err(e)
            }
        }
    }

    /// A person turns a proposal down.
    pub fn reject(&self, id: u32, actor: &Actor) -> Result<Value, (u16, Value)> {
        let by = {
            let mut ps = self.gov.proposals.lock().unwrap();
            let p = ps.iter_mut().find(|p| p.id == id).ok_or((404u16, json!({"error": format!("no proposal #{id}")})))?;
            if p.status != "pending" {
                return Err((409, json!({"error": format!("proposal #{id} is {}, not pending", p.status), "status": p.status})));
            }
            p.status = "rejected".into();
            p.actor.clone()
        };
        let receipt = self.receipt("show.reject", "DENY", actor, &format!("rejected proposal #{id} from {by}"));
        let view = self.gov.proposals.lock().unwrap().iter().find(|p| p.id == id).map(Proposal::json);
        self.publish(json!({"type": "proposal", "ts": now_ms(), "proposal": view}));
        Ok(json!({"rejected": id, "receipt": receipt}))
    }

    /// After any edit: proposals made against an older show no longer apply, so they are marked superseded.
    fn supersede(&self) {
        let base = self.base_hash();
        let mut changed = Vec::new();
        for p in self.gov.proposals.lock().unwrap().iter_mut() {
            if p.status == "pending" && p.base != base {
                p.status = "superseded".into();
                changed.push(p.json());
            }
        }
        for view in changed {
            self.publish(json!({"type": "proposal", "ts": now_ms(), "proposal": view}));
        }
    }

    // ---- failover (see the `failover` module) ----

    /// The secret a standby proves itself with, to read the log (`/replicate`) and to tell this runtime another has taken over (`/fence`).
    pub fn set_failover_key(&self, key: Option<String>) {
        *self.failover_key.lock().unwrap() = key.filter(|k| !k.is_empty());
    }

    pub fn failover_enabled(&self) -> bool {
        self.failover_key.lock().unwrap().is_some()
    }

    pub fn failover_ok(&self, provided: Option<&str>) -> bool {
        match (self.failover_key.lock().unwrap().as_deref(), provided) {
            (Some(k), Some(p)) => k.len() == p.len() && k.bytes().zip(p.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0,
            _ => false,
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_epoch(&self, e: u64) {
        self.epoch.store(e, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_fenced(&self) -> bool {
        self.fenced.lock().unwrap().is_some()
    }

    /// Whether this runtime is the one running the show, and which takeover it is on.
    pub fn role_json(&self) -> Value {
        let fenced = self.fenced.lock().unwrap().clone();
        json!({"role": if fenced.is_some() { "fenced" } else { "active" }, "epoch": self.epoch(), "fenced_by": fenced, "failover": self.failover_enabled(), "ts": now_ms()})
    }

    /// Another runtime (a standby) says it has taken over at `epoch`. If that is newer than this one's, this runtime stops: it
    /// sends nothing more, streams no lighting, disarms its lasers, ignores inputs and timers, and says so to every client.
    /// Stopping is the only safe answer to "someone else is running the show"; the person decides what to do next.
    pub fn fence(self: &Arc<Self>, epoch: u64, by: &str) -> Value {
        if epoch <= self.epoch() {
            return json!({"fenced": self.is_fenced(), "epoch": self.epoch(), "note": "not newer than this runtime's own epoch: ignored"});
        }
        if self.is_fenced() {
            self.set_epoch(epoch);
            return json!({"fenced": true, "epoch": epoch});
        }
        let ts = now_ms();
        let info = json!({"by": by, "epoch": epoch, "ts": ts});
        *self.fenced.lock().unwrap() = Some(info.clone());
        self.set_epoch(epoch);
        let out = self.output.lock().unwrap().clone();
        if let Some(out) = out {
            for l in out.lasers() {
                out.set_armed(&l.id, false);
            }
            tokio::spawn(async move { out.silence().await });
        }
        self.inner.lock().unwrap().generation += 1; // timers armed before this do nothing
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, "runtime", Some(&show_id), "show.fenced", &info);
        }
        self.publish(json!({"type": "fenced", "ts": ts, "by": by, "epoch": epoch}));
        self.receipt("failover.fenced", "ALLOW", &Actor::Person(None), &format!("{by} took over the show (takeover {epoch}): this runtime stopped sending and disarmed its lasers"));
        json!({"fenced": true, "epoch": epoch})
    }

    /// The log after record `after`, for a standby. `last_ts` is the timestamp the standby holds for record `after`: if the log
    /// here has a different one, this is not the log the standby has been following and it must start again.
    pub fn replicate_json(&self, after: i64, last_ts: Option<i64>) -> Result<Value, (u16, String)> {
        let g = self.inner.lock().unwrap();
        if after > 0 {
            match (g.store.timestamp_of(after).map_err(|e| (500u16, e.to_string()))?, last_ts) {
                (Some(t), Some(l)) if t == l => {}
                (None, _) => return Err((409, format!("this runtime's log has no record {after}: the standby follows a different run"))),
                _ => return Err((409, "the standby's copy no longer matches this runtime's log: start the standby again with an empty database".into())),
            }
        }
        let recs = g.store.after(after, 500).map_err(|e| (500u16, e.to_string()))?;
        let last = g.store.last_id().map_err(|e| (500u16, e.to_string()))?;
        let list: Vec<Value> = recs.iter().map(|r| json!({"id": r.id, "ts": r.timestamp, "source": r.source, "target": r.target, "type": r.event_type, "payload": r.payload})).collect();
        Ok(json!({"records": list, "last_id": last, "epoch": self.epoch(), "state": g.engine.current_state, "ts": now_ms(), "fenced": self.is_fenced()}))
    }

    /// Whether a restarted runtime carried on a run (see `resume`).
    pub fn was_resumed(&self) -> bool {
        self.resumed.lock().unwrap().is_some()
    }

    /// The run so far: every cue's planned and actual time, deliveries that went wrong, and edits made mid-show.
    pub fn run_report(&self) -> Value {
        let events = self.events();
        let of = |t: &str| events.iter().filter(|e| e["type"] == t).map(|e| e["payload"].clone()).collect::<Vec<_>>();
        let cues = of("show.cue");
        let deliveries = of("show.delivery");
        let problems: Vec<&Value> = deliveries.iter().filter(|d| matches!(d["status"].as_str(), Some("failed") | Some("no-device"))).collect();
        let worst = cues.iter().map(|c| c["drift_ms"].as_i64().unwrap_or(0).abs()).max().unwrap_or(0);
        json!({
            "cues": cues,
            "problems": problems,
            "edits": of("show.edit"),
            "decisions": of("show.decision"),
            "summary": {
                "cues_fired": cues.len(),
                "deliveries": deliveries.len(),
                "failed": deliveries.iter().filter(|d| d["status"] == "failed").count(),
                "no_device": deliveries.iter().filter(|d| d["status"] == "no-device").count(),
                "worst_drift_ms": worst,
            },
        })
    }

    /// Publish (and record) the action of a timeline cue that has come due.
    fn fire_cue(self: &Arc<Self>, cue: &ScheduledCue) {
        self.fire_cue_at(cue, None)
    }

    /// `tc_actual_ms`: for a cue fired by timecode, how far into the scene the timecode was (the run report's "actual").
    fn fire_cue_at(self: &Arc<Self>, cue: &ScheduledCue, tc_actual_ms: Option<i64>) {
        if self.is_fenced() {
            return;
        }
        let ts = now_ms();
        let event = cue.action.to_event();
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, &show_id, None, &cue.action.kind, &event);
            // what the show planned against what happened, for the run report
            let actual = tc_actual_ms.unwrap_or_else(|| (ts - g.entered_at).max(0));
            let rec = json!({"scene": g.engine.current_state, "lane": cue.lane, "cue": cue.cue, "num": cue.num, "kind": cue.action.kind,
                             "planned_ms": cue.at_ms, "actual_ms": actual, "drift_ms": actual - cue.at_ms as i64, "timecode": tc_actual_ms.is_some()});
            let _ = g.store.append(ts, "runtime", Some(&show_id), "show.cue", &rec);
        }
        self.publish(json!({"type": "action", "ts": ts, "event": event, "lane": cue.lane, "cue": cue.cue}));
        self.route(ts, &cue.action, Some(&cue.lane), Some(&cue.cue), cue.dept.as_deref(), cue.device.as_deref());
    }

    /// Send an action to the device that handles it and report what happened as a
    /// `delivery` message: status "sent", "dry-run", "no-device" or "failed". Never blocks the show.
    fn route(self: &Arc<Self>, ts: i64, action: &Action, lane: Option<&str>, cue: Option<&str>, dept: Option<&str>, device: Option<&str>) {
        let Some(out) = self.output.lock().unwrap().clone() else { return };
        if self.is_fenced() {
            return;
        }
        let base = json!({"type": "delivery", "ts": ts, "kind": action.kind, "lane": lane, "cue": cue});
        fn with(base: &Value, extra: Value) -> Value {
            let mut m = base.clone();
            m.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
            m
        }
        match out.plan(&action.kind, dept, device) {
            Plan::NotRouted => {}
            Plan::NoDevice { dept, reason } => self.report(with(&base, json!({"status": "no-device", "dept": dept, "error": reason}))),
            Plan::To(dev) => {
                let ident = json!({"device": dev.id, "label": dev.label, "dept": dev.dept, "target": dev.address()});
                if dev.dept == "laser" && !out.is_armed(&dev.id) {
                    let mut m = ident;
                    m["status"] = json!("held");
                    m["error"] = json!(format!("{} is not armed: arm it on the Live tab to let cues through", dev.label));
                    return self.report(with(&base, m));
                }
                if !out.sending() {
                    let mut m = ident;
                    m["status"] = json!("dry-run");
                    m["note"] = json!("sending is off: start the runtime with --send");
                    return self.report(with(&base, m));
                }
                let show = Arc::clone(self);
                let action = action.clone();
                let out = Arc::clone(&out);
                tokio::spawn(async move {
                    let result = out.deliver(&dev, &action).await;
                    let mut m = ident;
                    match result {
                        Ok(_) => m["status"] = json!("sent"),
                        Err(e) => {
                            m["status"] = json!("failed");
                            m["error"] = json!(e);
                        }
                    }
                    show.report(with(&base, m));
                });
            }
        }
    }

    /// Record a delivery result in the event log and tell every listener.
    fn report(&self, msg: Value) {
        let ts = msg["ts"].as_i64().unwrap_or_else(now_ms);
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, "runtime", Some(&show_id), "show.delivery", &msg);
        }
        self.publish(msg);
    }
}

#[derive(Clone)]
struct App {
    show: Arc<Show>,
    token: Option<String>,
}

pub fn router(show: Arc<Show>) -> Router {
    router_with_token(show, None)
}

/// Like `router`, but every request must present `token` (as `?token=` or a Bearer header).
pub fn router_with_token(show: Arc<Show>, token: Option<String>) -> Router {
    Router::new()
        .route("/state", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.snapshot()))
        }))
        .route("/events", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!([]))); }
            (StatusCode::OK, Json(json!(a.show.events())))
        }))
        .route("/ready", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.readiness_map()))
        }))
        .route("/report", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.run_report()))
        }))
        .route("/show", post(post_show))
        .route("/show/undo", post(post_undo))
        .route("/preflight", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.preflight()))
        }))
        .route("/whoami", get(get_whoami))
        .route("/signin", post(post_signin))
        .route("/signout", post(post_signout))
        .route("/vote", post(post_vote))
        .route("/polls", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.polls_json()))
        }))
        .route("/polls/:id/:verb", post(post_poll))
        .route("/fixtures", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(fixtures::summary()))
        }))
        .route("/captures", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.captures_json()))
        }))
        .route("/profiles", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(profiles::summary()))
        }))
        .route("/inputs", get(|State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap| async move {
            if !authorized(&a.token, &q, &h) { return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"}))); }
            (StatusCode::OK, Json(a.show.inputs_json()))
        }))
        .route("/rehearse", post(post_rehearse))
        .route("/proposals", get(get_proposals))
        .route("/proposals/:id/:verb", post(post_decide))
        .route("/governance/export", get(get_governance))
        .route("/devices/:id/try", post(post_try))
        .route("/laser/stop", post(post_laser_stop))
        .route("/devices/:id/arm", post(post_arm))
        .route("/devices/:id/test", post(post_test))
        .route("/ready/:dept", post(post_ready))
        .route("/input", post(post_input))
        .route("/recording", get(get_recording))
        .route("/replay", get(get_replay).post(post_replay).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)))
        .route("/role", get(get_role))
        .route("/replicate", get(get_replicate))
        .route("/fence", post(post_fence))
        .route("/ws", get(ws_upgrade))
        .with_state(App { show, token })
}

fn authorized(token: &Option<String>, q: &HashMap<String, String>, h: &HeaderMap) -> bool {
    let Some(t) = token else { return true };
    let bearer = h.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    q.get("token").map(String::as_str) == Some(t.as_str()) || bearer == Some(t.as_str())
}

type Reply = (StatusCode, Json<Value>);

/// What a client says about itself: `X-Pchi-Actor: agent:<name>` (or `?actor=`), otherwise a person.
/// A declaration only: who they really are comes from `Show::identify`.
fn actor_of(q: &HashMap<String, String>, h: &HeaderMap) -> Actor {
    Actor::from_headers(h, q.get("actor").map(String::as_str))
}

/// Who is really asking: the roster person or operator the key belongs to, an agent, or nobody.
fn who(a: &App, q: &HashMap<String, String>, h: &HeaderMap) -> Actor {
    a.show.identify_with(session_of(q, h), edit_key_of(q, h), actor_of(q, h))
}

/// The session token, from `X-Pchi-Session` or `?session=` (a WebSocket cannot set headers).
fn session_of<'a>(q: &'a HashMap<String, String>, h: &'a HeaderMap) -> Option<&'a str> {
    h.get("x-pchi-session").and_then(|v| v.to_str().ok()).or_else(|| q.get("session").map(String::as_str))
}

/// `POST /signin` {"key": "..."}: trade a key for a session token (see `Show::sign_in`). `POST /signout` ends it.
async fn post_signin(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    match a.show.sign_in(body["key"].as_str().unwrap_or("")) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, e)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::FORBIDDEN), Json(json!({"error": e}))),
    }
}

async fn post_signout(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let ended = session_of(&q, &h).is_some_and(|t| a.show.sign_out(t));
    (StatusCode::OK, Json(json!({"signed_out": ended})))
}

/// Refuse, and write the refusal down. Says why in the words the person needs.
fn refuse(show: &Show, actor: &Actor, rule: &str, what: &str) -> Reply {
    match actor {
        Actor::Agent(_) => {
            let receipt = show.receipt(rule, "DENY", actor, &format!("refused: agents may not {what}"));
            (StatusCode::FORBIDDEN, Json(json!({"error": format!("agents may not {what}: they can propose a show edit for a person to approve"), "denied": true, "agent": true, "receipt": receipt})))
        }
        Actor::Anonymous => {
            show.receipt(rule, "DENY", actor, &format!("refused: {what} needs a key"));
            locked()
        }
        _ => {
            let receipt = show.receipt(rule, "DENY", actor, &format!("refused: {} may not {what}", actor.role()));
            (StatusCode::FORBIDDEN, Json(json!({"error": format!("{} may not {what}", actor.name()), "denied": true, "role": actor.role(), "receipt": receipt})))
        }
    }
}

/// Let `actor` through, or refuse (and record it).
fn gate(a: &App, actor: &Actor, perm: Perm, rule: &str, what: &str, device: Option<&str>) -> Result<(), Reply> {
    let dept = device.and_then(|d| a.show.device_dept(d));
    if a.show.allow(actor, perm, dept.as_deref()) {
        Ok(())
    } else {
        Err(refuse(&a.show, actor, rule, what))
    }
}

fn unauthorized() -> Reply {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"})))
}

/// `GET /whoami`: who the runtime takes the caller to be, and what they may do. Reads the same key as everything else.
async fn get_whoami(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let can: Vec<&str> = governance::ALL_PERMS.iter().filter(|(p, _)| a.show.allow(&actor, *p, Some("laser"))).map(|(_, n)| *n).collect();
    (StatusCode::OK, Json(json!({"name": actor.name(), "role": actor.role(), "signed_in": !matches!(actor, Actor::Anonymous), "locked": a.show.is_locked(), "can": can})))
}

/// `POST /show` (a show file, as Studio saves it) replaces the running show's definition;
/// `?dry=1` only reports what would change. See `Show::replace`.
/// From an agent a real send is stored as a proposal for a person to approve.
async fn post_show(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, body: String) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let dry = q.get("dry").is_some_and(|v| v == "1" || v == "true");
    if !actor.is_agent() {
        if let Err(r) = gate(&a, &actor, Perm::Edit, "show.edit", "send a show", None) {
            return r;
        }
    }
    let (chart, devices) = match rig::load(&body) {
        Ok(x) => x,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    };
    // what the sender said they wanted (a typed or narrated line, or an agent's reason): kept with the proposal and in the record
    let said = governance::clean_said(q.get("said").map(String::as_str));
    // anyone who may edit can also ask for a second pair of eyes: `?propose=1` stores the edit instead of applying it
    let propose = q.get("propose").is_some_and(|v| v == "1" || v == "true");
    let result = if dry {
        a.show.replace(chart, devices, false)
    } else if actor.is_agent() || propose {
        a.show.propose(&actor, chart, devices, &said)
    } else {
        a.show.replace(chart, devices, true).inspect(|v| {
            let n = v["diff"]["changes"].as_array().map_or(0, |c| c.len());
            a.show.receipt("show.edit", "ALLOW", &actor, &format!("sent an edited show ({n} change(s)){}", said_suffix(&said)));
        })
    };
    match result {
        Ok(v) if v["proposed"] == json!(true) => (StatusCode::ACCEPTED, Json(v)),
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, v)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(v)),
    }
}

/// The caller's key, from `X-Pchi-Edit-Key` or `?edit_key=`.
fn edit_key_of<'a>(q: &'a HashMap<String, String>, h: &'a HeaderMap) -> Option<&'a str> {
    h.get("x-pchi-edit-key").and_then(|v| v.to_str().ok()).or_else(|| q.get("edit_key").map(String::as_str))
}

fn locked() -> Reply {
    (StatusCode::FORBIDDEN, Json(json!({"error": "a key is required for this", "locked": true})))
}

/// `POST /show/undo` (`?dry=1` to preview): put the show back as it was before the last send.
async fn post_undo(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let dry = q.get("dry").is_some_and(|v| v == "1" || v == "true");
    if !(dry && actor.is_agent()) {
        if let Err(r) = gate(&a, &actor, Perm::Undo, "show.undo", "undo a send", None) {
            return r;
        }
    }
    match a.show.undo(!dry) {
        Ok(v) => {
            if !dry {
                a.show.receipt("show.undo", "ALLOW", &actor, "put the show back to before the last send");
            }
            (StatusCode::OK, Json(v))
        }
        Err((code, v)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(v)),
    }
}

/// `POST /devices/<id>/try` {"value": "3"}: one cue to one device through its profile.
async fn post_try(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    if let Err(r) = gate(&a, &actor, Perm::Try, "device.try", "send a cue to a device", Some(&id)) {
        return r;
    }
    let value = body["value"].as_str().unwrap_or("");
    match a.show.try_cue(&id, value).await {
        Ok(v) => {
            a.show.receipt("device.try", "ALLOW", &actor, &format!("tried '{value}' on {id}: {}", v["status"].as_str().unwrap_or("?")));
            (StatusCode::OK, Json(v))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

/// `POST /laser/stop`: the hard stop. Needs the feed token (if any) but no key and no role: anyone on the crew can stop, agents included.
async fn post_laser_stop(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let v = a.show.laser_stop();
    a.show.receipt("laser.stop", "ALLOW", &actor, &format!("hard stop: disarmed {} laser(s)", v["stopped"].as_array().map_or(0, |s| s.len())));
    (StatusCode::OK, Json(v))
}

/// `POST /devices/<id>/arm?armed=1|0`: arming needs the right (an operator, or the laser safety officer); disarming never does.
async fn post_arm(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let armed = q.get("armed").is_some_and(|v| v == "1" || v == "true");
    if armed {
        if let Err(r) = gate(&a, &actor, Perm::Arm, "laser.arm", "arm a laser", None) {
            return r;
        }
    }
    match a.show.set_armed(&id, armed) {
        Ok(()) => {
            a.show.receipt(if armed { "laser.arm" } else { "laser.disarm" }, "ALLOW", &actor, &format!("{} {id}", if armed { "armed" } else { "disarmed" }));
            (StatusCode::OK, Json(json!({"ok": true, "armed": armed})))
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e}))),
    }
}

/// `POST /devices/<id>/test`: one `/pchi/test` message to that device.
async fn post_test(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    if let Err(r) = gate(&a, &actor, Perm::Test, "device.test", "test-fire a device", Some(&id)) {
        return r;
    }
    match a.show.test_fire(&id).await {
        Ok(v) => {
            a.show.receipt("device.test", "ALLOW", &actor, &format!("test-fired {id}: {}", v["status"].as_str().unwrap_or("?")));
            (StatusCode::OK, Json(v))
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e}))),
    }
}

/// `POST /rehearse` (a show file): what the edited show would have done with the run so far. Read-only, so it needs no
/// operator key and agents may use it: it is how an agent checks an idea before proposing it.
async fn post_rehearse(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, body: String) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let (chart, devices) = match rig::load(&body) {
        Ok(x) => x,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    };
    match a.show.rehearse(chart, devices) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": e}))),
    }
}

/// `POST /vote` {"poll", "option", "voter"}: one audience vote. Needs the audience token (`--audience-token`), which opens this and
/// nothing else, so it is not the feed token and does not need one.
async fn post_vote(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !a.show.audience_enabled() {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "audience voting is not turned on: start the runtime with --audience-token"})));
    }
    let bearer = h.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let token = h.get("x-pchi-audience").and_then(|v| v.to_str().ok()).or(bearer).or_else(|| q.get("token").map(String::as_str));
    if !a.show.audience_ok(token) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "the audience token is wrong"})));
    }
    let (poll, option, voter) = (body["poll"].as_str().unwrap_or(""), body["option"].as_str().unwrap_or(""), body["voter"].as_str().unwrap_or(""));
    match a.show.cast_vote(poll, option, voter) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, e)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(json!({"error": e}))),
    }
}

/// `POST /polls/<id>/open` {"options": [...]} and `POST /polls/<id>/close`: a person (or a button) with the right to run the show.
async fn post_poll(State(a): State<App>, Path((id, verb)): Path<(String, String)>, q: Query<HashMap<String, String>>, h: HeaderMap, body: String) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let (rule, what) = match verb.as_str() {
        "open" => ("poll.open", "open an audience poll"),
        "close" => ("poll.close", "close an audience poll"),
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error": "use open or close"}))),
    };
    if let Err(r) = gate(&a, &actor, Perm::Go, rule, what, None) {
        return r;
    }
    let result = if verb == "open" {
        let options: Vec<String> = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["options"].as_array().map(|o| o.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default();
        a.show.poll_open(&actor, &id, &options)
    } else {
        a.show.poll_close(&actor, &id)
    };
    match result {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

/// `GET /proposals`: every proposal (pending first in the Studio's list; settled ones stay for the record).
async fn get_proposals(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    (StatusCode::OK, Json(a.show.proposals()))
}

/// `POST /proposals/<id>/approve|reject`: a person with the right (an operator or an editor) decides.
async fn post_decide(State(a): State<App>, Path((id, verb)): Path<(u32, String)>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = who(&a, &q, &h);
    let (rule, what) = match verb.as_str() {
        "approve" => ("show.edit", "approve a proposal"),
        "reject" => ("show.reject", "reject a proposal"),
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error": "use approve or reject"}))),
    };
    if let Err(r) = gate(&a, &actor, Perm::Approve, rule, what, None) {
        return r;
    }
    let r = if verb == "approve" { a.show.approve(id, &actor) } else { a.show.reject(id, &actor) };
    match r {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, v)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(v)),
    }
}

/// `GET /governance/export`: the signed receipts, for `scripts/verify_governance_chain.py`.
async fn get_governance(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    (StatusCode::OK, Json(a.show.governance_export()))
}

async fn post_ready(
    State(a): State<App>,
    Path(dept): Path<String>,
    q: Query<HashMap<String, String>>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"})));
    }
    let ready = body["ready"].as_bool().unwrap_or(true);
    match a.show.set_ready(&dept, ready, body["note"].as_str().unwrap_or(""), "http") {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

async fn post_input(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"})));
    }
    let Some(event) = body["event"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "send {\"event\": \"NAME\"}"})));
    };
    let actor = who(&a, &q, &h);
    if let Err(r) = gate(&a, &actor, Perm::Go, "show.go", "run the show (send an input)", None) {
        return r;
    }
    let outcome = a.show.apply_checked("http", event);
    match &outcome {
        Applied::Moved => {
            a.show.receipt("show.go", "ALLOW", &actor, &format!("input '{event}' moved the show to {}", a.show.current_state()));
        }
        Applied::Held { remaining_ms, min_ms, .. } => {
            a.show.receipt("show.go", "DENY", &actor, &format!("input '{event}' {}", outcome.held_text().unwrap_or_default()));
            return (StatusCode::OK, Json(json!({"applied": false, "held": {"remaining_ms": remaining_ms, "min_ms": min_ms}})));
        }
        Applied::Ignored => {}
    }
    (StatusCode::OK, Json(json!({"applied": outcome == Applied::Moved})))
}

/// `GET /recording`: the latest run as a file to keep (see `recording`).
async fn get_recording(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    match recording::export(&a.show.records()) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e}))),
    }
}

/// `GET /replay`: the latest run laid out for scrubbing, with a check that it replays. `POST /replay` with a recording as the body
/// does the same for a recording from a file; it changes nothing in the running show.
async fn get_replay(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    replay_reply(&a.show.records())
}

async fn post_replay(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, body: String) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    match recording::parse(&body) {
        Ok(records) => replay_reply(&records),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

fn replay_reply(records: &[pchi_eventstore::EventRecord]) -> Reply {
    match recording::view(records) {
        Ok(mut v) => {
            v["verify"] = recording::verify(records).unwrap_or_else(|e| json!({"matches": false, "verdict": e}));
            (StatusCode::OK, Json(v))
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e}))),
    }
}

/// `GET /role`: whether this runtime is the one running the show (see `Show::role_json`).
async fn get_role(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    (StatusCode::OK, Json(a.show.role_json()))
}

fn failover_key_of(h: &HeaderMap) -> Option<&str> {
    h.get("x-pchi-failover-key").and_then(|v| v.to_str().ok())
}

/// `GET /replicate?after=N&last_ts=T`: the log after record N, for a standby (needs the failover key).
async fn get_replicate(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    if !a.show.failover_enabled() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "failover is off: start the runtime with --failover-key"})));
    }
    if !a.show.failover_ok(failover_key_of(&h)) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "the failover key is wrong"})));
    }
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0i64);
    let last_ts = q.get("last_ts").and_then(|v| v.parse().ok());
    match a.show.replicate_json(after, last_ts) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, e)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), Json(json!({"error": e}))),
    }
}

/// `POST /fence` {"epoch": N, "by": "name"}: a standby says it has taken over (needs the failover key).
async fn post_fence(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    if !a.show.failover_ok(failover_key_of(&h)) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "the failover key is wrong or failover is off"})));
    }
    let Some(epoch) = body["epoch"].as_u64() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "send {\"epoch\": N, \"by\": \"name\"}"})));
    };
    let by = body["by"].as_str().unwrap_or("a standby");
    (StatusCode::OK, Json(a.show.fence(epoch, &by.chars().take(60).collect::<String>())))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let actor = who(&a, &q, &h);
    let show = a.show;
    ws.on_upgrade(move |socket| client(socket, show, actor))
}

async fn client(socket: WebSocket, show: Arc<Show>, actor: Actor) {
    let (mut sink, mut stream) = socket.split();
    // subscribe before the snapshot so nothing between the two is missed
    let mut rx = show.subscribe();
    if sink.send(Message::Text(show.snapshot().to_string())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let msg = match ev {
                    Ok(v) => v,
                    // slow client: tell it, and send a fresh snapshot so it can resync
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let _ = sink.send(Message::Text(json!({"type":"lagged","missed":n}).to_string())).await;
                        show.snapshot()
                    }
                    Err(_) => break,
                };
                if sink.send(Message::Text(msg.to_string())).await.is_err() { break; }
            }
            m = stream.next() => {
                let Some(Ok(m)) = m else { break };
                if let Message::Text(t) = m {
                    handle_client(&show, &actor, &t);
                }
            }
        }
    }
}

fn handle_client(show: &Arc<Show>, actor: &Actor, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    let kind = v["type"].as_str();
    // anyone may report readiness, but running the show needs the right to
    if matches!(kind, Some("input") | Some("context") | Some("reset")) && !show.allow(actor, Perm::Go, None) {
        show.receipt("show.go", "DENY", actor, &format!("refused: {} may not run the show ({})", actor.role(), kind.unwrap_or("")));
        let why = if actor.is_agent() { "agents may not run the show: they can propose a show edit for a person to approve".to_string() } else if matches!(actor, Actor::Anonymous) { "sign in with your key to run the show".to_string() } else { format!("{} may not run the show", actor.name()) };
        show.publish(json!({"type": "error", "message": why}));
        return;
    }
    match kind {
        Some("input") => {
            if let Some(e) = v["event"].as_str() {
                match show.apply_checked("studio", e) {
                    Applied::Moved => {
                        show.receipt("show.go", "ALLOW", actor, &format!("input '{e}' moved the show to {}", show.current_state()));
                    }
                    held @ Applied::Held { .. } => {
                        show.receipt("show.go", "DENY", actor, &format!("input '{e}' {}", held.held_text().unwrap_or_default()));
                    }
                    Applied::Ignored => {}
                }
            }
        }
        Some("context") => {
            if let Some(k) = v["key"].as_str() {
                show.set_context(k, v["value"].clone());
            }
        }
        Some("reset") => {
            show.reset();
            show.receipt("show.reset", "ALLOW", actor, "started a fresh run");
        }
        Some("ready") => {
            if let Some(d) = v["dept"].as_str() {
                // a client may name itself ("dept-page"); anything odd falls back to plain "ws"
                let source = v["source"].as_str().filter(|s| !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).unwrap_or("ws");
                if let Err(e) = show.set_ready(d, v["ready"].as_bool().unwrap_or(true), v["note"].as_str().unwrap_or(""), source) {
                    show.publish(json!({"type": "error", "message": e}));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message as TMsg};

    fn festival() -> Showchart {
        Showchart::from_json(include_str!("../../pchi-showchart/examples/festival.pchi.json")).unwrap()
    }

    #[tokio::test]
    async fn websocket_client_gets_hello_then_live_transitions_and_actions() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router(show)).await.unwrap();
        });

        let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let next = |m: Option<Result<TMsg, _>>| -> Value {
            match m.unwrap().unwrap() {
                TMsg::Text(t) => serde_json::from_str(&t).unwrap(),
                other => panic!("unexpected frame {other:?}"),
            }
        };
        let hello = next(ws.next().await);
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["state"], "PRESHOW");
        assert_eq!(hello["chart"]["id"], "festival-demo");
        assert_eq!(hello["definition"]["states"].as_array().unwrap().len(), 4);

        ws.send(TMsg::Text(r#"{"type":"input","event":"START"}"#.into())).await.unwrap();
        let t = next(ws.next().await);
        assert_eq!((t["type"].as_str(), t["from"].as_str(), t["to"].as_str(), t["applied"].as_bool()),
                   (Some("transition"), Some("PRESHOW"), Some("INTRO"), Some(true)));
        let a = next(ws.next().await);
        assert_eq!(a["event"], json!({"type": "scene.activate", "scene": "intro"}));

        // a second client joining late sees the current state in its hello
        let (mut late, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        assert_eq!(next(late.next().await)["state"], "INTRO");
    }

    #[tokio::test]
    async fn ignored_events_and_context_changes_are_reported() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        assert!(!show.apply("studio", "BOGUS"));
        let t = rx.recv().await.unwrap();
        assert_eq!((t["type"].as_str(), t["applied"].as_bool()), (Some("transition"), Some(false)));
        show.set_context("energy", json!(0.1));
        assert_eq!(rx.recv().await.unwrap()["key"], "energy");
        assert_eq!(show.snapshot()["context"]["energy"], json!(0.1));
    }

    #[tokio::test(start_paused = true)]
    async fn timers_run_server_side_and_are_cancelled_by_leaving_the_state() {
        // fires: INTRO --after 10000ms--> PORTAL
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(10_500)).await;
        assert_eq!(show.snapshot()["state"], "PORTAL");
        let mut sources = vec![];
        while let Ok(m) = rx.try_recv() {
            if m["type"] == "transition" {
                sources.push(m["source"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(sources, ["studio", "timer"]);

        // cancelled: leave INTRO manually before the timer
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        show.apply("studio", "OPEN_PORTAL");
        tokio::time::sleep(Duration::from_millis(60_000)).await;
        let n = std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["source"] == "timer").count();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn every_input_is_recorded_and_the_session_replays() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        show.set_context("energy", json!(0.2));
        show.apply("studio", "OPEN_PORTAL");
        let types: Vec<_> = show.events().iter().map(|e| e["type"].as_str().unwrap().to_string()).collect();
        assert!(types.contains(&"run.start".to_string()) && types.contains(&"show.input".to_string()));
        // replay straight from the same store
        let g = show.inner.lock().unwrap();
        let r = pchi_eventstore::replay_latest(&g.store).unwrap();
        assert!(r.matches_recording(), "{:?}", r.divergences);
        assert_eq!(r.final_state, "CALM");
    }

    #[tokio::test(start_paused = true)]
    async fn hello_reports_how_long_the_current_state_has_been_active() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        // the system clock is not paused by tokio, so compare loosely: just entered
        let fresh = show.snapshot()["elapsed_ms"].as_i64().unwrap();
        assert!((0..2_000).contains(&fresh), "elapsed_ms was {fresh}");
        // a state that has been active for a while reports it
        show.inner.lock().unwrap().entered_at -= 4_000;
        let later = show.snapshot()["elapsed_ms"].as_i64().unwrap();
        assert!((4_000..6_000).contains(&later), "elapsed_ms was {later}");
        // entering a new state restarts the clock
        show.apply("studio", "OPEN_PORTAL");
        assert!(show.snapshot()["elapsed_ms"].as_i64().unwrap() < 2_000);
    }

    fn timelines() -> Showchart {
        Showchart::from_json(include_str!("../../pchi-showchart/examples/timelines.pchi.json")).unwrap()
    }

    fn actions(rx: &mut broadcast::Receiver<Value>) -> Vec<(String, Option<String>)> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|m| m["type"] == "action")
            .map(|m| (m["event"]["type"].as_str().unwrap().to_string(), m["lane"].as_str().map(String::from)))
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn timeline_cues_fire_at_their_offsets_in_their_lanes() {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START"); // INTRO: cues at 0s, 2s, 4s
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(actions(&mut rx), [("scene.activate".to_string(), Some("visual".to_string()))]);
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        assert_eq!(actions(&mut rx), [("lighting.look".to_string(), Some("lighting".to_string()))]);
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        assert_eq!(actions(&mut rx), [("laser.cue".to_string(), Some("laser".to_string()))]);
    }

    fn device(id: &str, kind: &str, port: u16) -> rig::Device {
        rig::Device { id: id.into(), label: id.into(), kind: kind.into(), dept: rig::dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: None }
    }

    fn deliveries(rx: &mut broadcast::Receiver<Value>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["type"] == "delivery").collect()
    }

    #[tokio::test(start_paused = true)]
    async fn cues_are_routed_to_their_devices_as_a_dry_run_and_gaps_are_reported() {
        // visual and lighting have a device, lasers do not
        let out = Output::new(vec![device("v1", "notch", 9001), device("d1", "lighting-desk", 9002)], false);
        let show = Show::start_with(timelines(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START"); // INTRO: visual 0s, lighting 2s, laser 4s
        tokio::time::sleep(Duration::from_millis(4_001)).await;
        let d = deliveries(&mut rx);
        let summary: Vec<_> = d.iter().map(|m| (m["kind"].as_str().unwrap(), m["status"].as_str().unwrap(), m["device"].as_str())).collect();
        assert_eq!(summary, [("scene.activate", "dry-run", Some("v1")), ("lighting.look", "dry-run", Some("d1")), ("laser.cue", "no-device", None)]);
        assert_eq!(d[0]["target"], "127.0.0.1:9001");
        assert!(d[2]["error"].as_str().unwrap().contains("no laser device"));
        // the run is on the record, so a replay shows what went where
        assert!(show.events().iter().any(|e| e["type"] == "show.delivery"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_show_without_a_rig_routes_nothing() {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(5_000)).await;
        assert!(deliveries(&mut rx).is_empty());
        assert_eq!(show.snapshot()["devices"], Value::Null);
    }

    #[tokio::test]
    async fn with_sending_on_the_cue_really_arrives_and_is_reported_sent() {
        let rx_sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = rx_sock.local_addr().unwrap().port();
        let out = Output::new(vec![device("v1", "notch", port), device("d1", "lighting-desk", 1), device("l1", "laser", 1)], true);
        let show = Show::start_with(timelines(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        assert_eq!(show.snapshot()["output"], "live");
        let mut rx = show.subscribe();
        show.apply("studio", "START"); // visual cue at 0s
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(3), rx_sock.recv_from(&mut buf)).await.expect("packet").unwrap();
        let (_, pkt) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        let rosc::OscPacket::Message(m) = pkt else { panic!() };
        assert_eq!(m.addr, "/pchi/scene/activate");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let d = deliveries(&mut rx);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0]["status"], "sent");
        assert_eq!(d[0]["device"], "v1");
    }

    #[tokio::test]
    async fn a_device_that_cannot_be_reached_is_reported_failed_and_the_show_goes_on() {
        let mut bad = device("v1", "notch", 9);
        bad.host = Some("no-such-host.invalid".into());
        let show = Show::start_with(timelines(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![bad], true))).unwrap();
        let mut rx = show.subscribe();
        assert!(show.apply("studio", "START"));
        tokio::time::sleep(Duration::from_millis(1_800)).await;
        let d = deliveries(&mut rx);
        assert_eq!(d[0]["status"], "failed");
        assert!(d[0]["error"].as_str().unwrap().contains("no-such-host.invalid"));
        assert_eq!(show.snapshot()["state"], "INTRO", "a failed send never stops the show");
    }

    fn move_cue(c: &mut Showchart, scene: &str, lane: &str, to_ms: u64) {
        let l = c.states.iter_mut().find(|s| s.id == scene).unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == lane).unwrap();
        l.cues[0].at_ms = to_ms;
    }

    /// INTRO with its three cues pulled in to 0 / 0.4 / 0.8 s, so real-clock tests stay short.
    /// (Elapsed time comes from the wall clock, which a paused test clock does not move.)
    fn quick() -> Showchart {
        let mut c = timelines();
        move_cue(&mut c, "INTRO", "visual", 0);
        move_cue(&mut c, "INTRO", "lighting", 400);
        move_cue(&mut c, "INTRO", "laser", 800);
        c
    }

    #[tokio::test]
    async fn a_live_edit_keeps_the_running_scene_and_re_arms_its_cues() {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START"); // INTRO: visual 0, lighting 0.4 s, laser 0.8 s
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = actions(&mut rx);

        // a preview changes nothing
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 600);
        let dry = show.replace(edited.clone(), None, false).unwrap();
        assert_eq!(dry["applied"], false);
        assert!(dry["diff"]["changes"][0].as_str().unwrap().contains("moved 0.4s → 0.6s"), "{dry}");

        // applying it: lighting now fires at 0.6 s (not 0.4 s), the laser still at 0.8 s, and the scene is unchanged
        let done = show.replace(edited, None, true).unwrap();
        assert_eq!(done["applied"], true);
        assert_eq!(show.snapshot()["state"], "INTRO");
        tokio::time::sleep(Duration::from_millis(350)).await; // ~0.5 s into the scene
        assert!(actions(&mut rx).is_empty(), "the old 0.4 s lighting cue must not fire");
        tokio::time::sleep(Duration::from_millis(250)).await; // ~0.75 s
        assert_eq!(actions(&mut rx), [("lighting.look".to_string(), Some("lighting".to_string()))]);
        tokio::time::sleep(Duration::from_millis(250)).await; // ~1.0 s
        assert_eq!(actions(&mut rx), [("laser.cue".to_string(), Some("laser".to_string()))]);

        // the edit is on the record
        assert_eq!(show.run_report()["edits"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_cue_edited_to_a_time_already_past_does_not_fire_late() {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = actions(&mut rx);
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "laser", 100); // moved to before "now"
        let r = show.replace(edited, None, true).unwrap();
        assert!(r["diff"]["notes"][0].as_str().unwrap().contains("will not fire this time"), "{r}");
        tokio::time::sleep(Duration::from_millis(900)).await;
        let kinds: Vec<_> = actions(&mut rx).into_iter().map(|a| a.0).collect();
        assert_eq!(kinds, ["lighting.look"], "the laser cue is skipped, the lighting cue still fires");
    }

    #[tokio::test(start_paused = true)]
    async fn a_live_edit_that_removes_the_running_scene_is_refused() {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        let mut gone = timelines();
        gone.states.retain(|s| s.id != "INTRO");
        gone.transitions.retain(|t| t.source != "INTRO" && t.target != "INTRO");
        gone.initial = gone.states[0].id.clone();
        let err = show.replace(gone, None, true).unwrap_err();
        assert_eq!(err.0, 409);
        assert!(err.1["error"].as_str().unwrap().contains("running scene INTRO"));
        assert_eq!(show.snapshot()["state"], "INTRO", "the show is untouched");
    }

    #[tokio::test]
    async fn the_report_gives_planned_and_actual_cue_times() {
        let out = Output::new(vec![device("v1", "notch", 9001)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let r = show.run_report();
        let cues = r["cues"].as_array().unwrap();
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[1]["planned_ms"], 400);
        assert!(cues[1]["drift_ms"].as_i64().unwrap().abs() < 100, "{}", cues[1]);
        assert_eq!(r["summary"]["cues_fired"], 3);
        assert_eq!(r["summary"]["no_device"], 2, "lighting and laser have no device in this rig");
        assert_eq!(r["problems"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_fire_reaches_a_device_or_reports_why_not() {
        let rx_sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = rx_sock.local_addr().unwrap().port();
        // dry run: reported, nothing sent
        let dry = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("l1", "laser", port)], false))).unwrap();
        assert_eq!(dry.test_fire("l1").await.unwrap()["status"], "held", "a laser is held until armed");
        dry.set_armed("l1", true).unwrap();
        assert_eq!(dry.test_fire("l1").await.unwrap()["status"], "dry-run");
        assert!(tokio::time::timeout(Duration::from_millis(200), rx_sock.recv_from(&mut [0u8; 64])).await.is_err(), "nothing may leave in a dry run");
        // live: the packet arrives
        let live = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("l1", "laser", port)], true))).unwrap();
        live.set_armed("l1", true).unwrap();
        let r = live.test_fire("l1").await.unwrap();
        assert_eq!(r["status"], "sent");
        let mut buf = [0u8; 64];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), rx_sock.recv_from(&mut buf)).await.unwrap().unwrap();
        let rosc::OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1 else { panic!() };
        assert_eq!(m.addr, "/pchi/test");
        assert!(live.test_fire("nope").await.unwrap_err().contains("no device"));
        assert!(Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap().test_fire("l1").await.unwrap_err().contains("no rig"));
    }

    #[tokio::test(start_paused = true)]
    async fn leaving_a_state_cancels_its_unfired_cues() {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(1)).await;
        let _ = actions(&mut rx); // the cue at 0s
        show.apply("studio", "DROP"); // leave INTRO before its 2s and 4s cues
        tokio::time::sleep(Duration::from_millis(10_000)).await;
        let kinds: Vec<_> = actions(&mut rx).into_iter().map(|a| a.0).collect();
        // DROP's own enter-less state fires only its own visual cue at 0s
        assert_eq!(kinds, ["scene.activate"]);
    }

    #[tokio::test(start_paused = true)]
    async fn cue_actions_are_recorded_in_the_event_log() {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(5_000)).await;
        let kinds: Vec<_> = show.events().iter().map(|e| e["type"].as_str().unwrap().to_string()).collect();
        for k in ["lighting.look", "laser.cue"] {
            assert!(kinds.contains(&k.to_string()), "{k} should be in the log: {kinds:?}");
        }
        // and the recorded run still replays identically
        let g = show.inner.lock().unwrap();
        assert!(pchi_eventstore::replay_latest(&g.store).unwrap().matches_recording());
    }

    #[tokio::test]
    async fn reset_starts_a_fresh_run() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        show.reset();
        assert_eq!(show.snapshot()["state"], "PRESHOW");
    }

    // ---- departments reporting in, and the network front door ----

    async fn serve(token: Option<&str>) -> (Arc<Show>, std::net::SocketAddr) {
        let show = Show::start(timelines(), EventStore::open_in_memory().unwrap()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), token.map(String::from));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (show, addr)
    }

    /// Minimal HTTP/1.1 client, so the tests need no extra dependency.
    async fn http(addr: std::net::SocketAddr, method: &str, path: &str, body: &str, extra: &str) -> (u16, Value) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let status: u16 = out.split_whitespace().nth(1).unwrap().parse().unwrap();
        let json = out.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b.trim()).ok()).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn departments_report_readiness_over_http_and_websocket() {
        let (show, addr) = serve(None).await;
        let mut rx = show.subscribe();
        let (st, _) = http(addr, "POST", "/ready/Lighting", r#"{"ready":true,"note":"cue 12 loaded"}"#, "").await;
        assert_eq!(st, 200);
        let m = rx.recv().await.unwrap();
        assert_eq!((m["type"].as_str(), m["dept"].as_str(), m["ready"].as_bool()), (Some("readiness"), Some("lighting"), Some(true)));
        assert_eq!(m["source"], "http");

        // a client that connects later is told who is ready, in the hello
        let (mut ws, _) = connect_async(format!("ws://{addr}/ws")).await.unwrap();
        let hello: Value = match ws.next().await.unwrap().unwrap() {
            TMsg::Text(t) => serde_json::from_str(&t).unwrap(),
            o => panic!("{o:?}"),
        };
        assert_eq!(hello["readiness"]["lighting"]["note"], "cue 12 loaded");

        // websocket clients can report too, and bad names are refused
        ws.send(TMsg::Text(r#"{"type":"ready","dept":"laser","ready":false,"note":"safety check"}"#.into())).await.unwrap();
        ws.send(TMsg::Text(r#"{"type":"ready","dept":"bad name!","ready":true}"#.into())).await.unwrap();
        let mut saw = (false, false);
        for _ in 0..4 {
            let Ok(Some(Ok(TMsg::Text(t)))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await else { break };
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "readiness" && v["dept"] == "laser" && v["ready"] == false { saw.0 = true }
            if v["type"] == "error" { saw.1 = true }
        }
        assert_eq!(saw, (true, true));

        let (_, all) = http(addr, "GET", "/ready", "", "").await;
        assert_eq!(all["laser"]["ready"], false);
        assert!(show.events().iter().any(|e| e["type"] == "show.ready"), "readiness is in the event log");
    }

    #[tokio::test]
    async fn repeating_the_same_readiness_is_a_heartbeat_and_stays_out_of_the_log() {
        let (show, _addr) = serve(None).await;
        let mut rx = show.subscribe();
        show.set_ready("lighting", true, "standing by", "dept-page").unwrap();
        show.set_ready("lighting", true, "standing by", "dept-page").unwrap();
        show.set_ready("lighting", false, "standing by", "dept-page").unwrap(); // a real change
        let m: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(m.iter().map(|x| x["heartbeat"].as_bool().unwrap()).collect::<Vec<_>>(), [false, true, false]);
        let logged = show.events().iter().filter(|e| e["type"] == "show.ready").count();
        assert_eq!(logged, 2, "the heartbeat is not recorded");
        assert!(show.snapshot()["server_ts"].as_i64().unwrap() > 0);
    }

    async fn recv_osc(sock: &tokio::net::UdpSocket) -> String {
        let mut buf = [0u8; 256];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await.expect("a packet").unwrap();
        let rosc::OscPacket::Message(m) = rosc::decoder::decode_udp(&buf[..n]).unwrap().1 else { panic!() };
        m.addr
    }

    #[tokio::test]
    async fn lasers_get_nothing_until_armed_and_the_hard_stop_goes_dark() {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let out = Output::new(vec![device("b1", "beyond", port)], true);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        let dev = |show: &Arc<Show>| show.snapshot()["devices"][0].clone();
        assert_eq!(dev(&show)["armed"], false, "a laser starts disarmed");

        // the scene's laser cue (0.8 s) is held, not sent
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let held: Vec<Value> = deliveries(&mut rx).into_iter().filter(|d| d["kind"] == "laser.cue").collect();
        assert_eq!(held[0]["status"], "held");
        assert!(held[0]["error"].as_str().unwrap().contains("not armed"));
        assert!(tokio::time::timeout(Duration::from_millis(200), recv_osc(&sock)).await.is_err(), "nothing reached the laser");
        assert_eq!(show.test_fire("b1").await.unwrap()["status"], "held");

        // armed: a message goes through
        show.set_armed("b1", true).unwrap();
        assert_eq!(dev(&show)["armed"], true);
        assert_eq!(show.test_fire("b1").await.unwrap()["status"], "sent");
        assert_eq!(recv_osc(&sock).await, "/pchi/test");

        // hard stop: disarmed, and BEYOND is told to black out and disable output
        let r = show.laser_stop();
        assert_eq!(r["stopped"], json!(["b1"]));
        assert_eq!(recv_osc(&sock).await, "/beyond/general/BlackOut");
        assert_eq!(recv_osc(&sock).await, "/beyond/general/DisableLaserOutput");
        assert_eq!(dev(&show)["armed"], false);
        assert_eq!(show.test_fire("b1").await.unwrap()["status"], "held");
        assert!(show.set_armed("nope", true).is_err());
    }

    #[tokio::test]
    async fn arming_keeps_across_a_live_edit_and_a_tool_with_no_stop_message_is_said_so() {
        let out = Output::new(vec![device("l1", "laser", 9), device("b1", "beyond", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.set_armed("b1", true).unwrap();
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 500);
        let devices = vec![device("l1", "laser", 9), device("b1", "beyond", 9)];
        show.replace(edited, Some(devices), true).unwrap();
        assert_eq!(show.snapshot()["devices"][1]["armed"], true, "a send must not quietly disarm or arm anything");
        let r = show.laser_stop();
        assert_eq!(r["stopped"].as_array().unwrap().len(), 2);
        assert!(r["notes"][0].as_str().unwrap().contains("no stop message is known"), "{r}");
    }

    #[tokio::test]
    async fn arming_needs_the_key_but_stopping_never_does() {
        let out = Output::new(vec![device("b1", "beyond", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.set_edit_key(Some("k".into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), None);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=1", "", "").await.0, 403);
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=1", "", "X-Pchi-Edit-Key: k\r\n").await.0, 200);
        assert_eq!(show.snapshot()["devices"][0]["armed"], true);
        // disarming and the hard stop are open to anyone with the feed token
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=0", "", "").await.0, 200);
        assert_eq!(show.snapshot()["devices"][0]["armed"], false);
        show.set_armed("b1", true).unwrap();
        let (st, v) = http(addr, "POST", "/laser/stop", "", "").await;
        assert_eq!((st, v["stopped"].clone()), (200, json!(["b1"])));
        assert_eq!(show.snapshot()["devices"][0]["armed"], false);
        assert_eq!(http(addr, "POST", "/devices/zz/arm?armed=1", "", "X-Pchi-Edit-Key: k\r\n").await.0, 404);
    }

    fn osc_bytes(addr: &str) -> Vec<u8> {
        rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: addr.into(), args: vec![rosc::OscType::String("X32".into())] })).unwrap()
    }

    /// Wait (up to 3 s) for a message that matches.
    async fn wait_for(rx: &mut broadcast::Receiver<Value>, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        let end = tokio::time::Instant::now() + Duration::from_secs(3);
        while let Ok(Ok(m)) = tokio::time::timeout_at(end, rx.recv()).await {
            if pred(&m) {
                return Some(m);
            }
        }
        None
    }

    async fn free_port() -> u16 {
        tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port()
    }

    fn timing() -> (Duration, Duration) {
        (Duration::from_millis(100), Duration::from_millis(700))
    }

    #[tokio::test]
    async fn a_tool_that_answers_its_probe_is_linked_and_is_lost_when_it_stops() {
        let fake = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let port = fake.local_addr().unwrap().port();
        // a fake X32: answers every /info to whoever asked
        let answering = {
            let fake = Arc::clone(&fake);
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while let Ok((_, from)) = fake.recv_from(&mut buf).await {
                    let _ = fake.send_to(&osc_bytes("/info"), from).await;
                }
            })
        };
        let (every, timeout) = timing();
        let out = Output::new(vec![device("x1", "x32", port)], true).with_timing(every, timeout);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();

        let fb = wait_for(&mut rx, |m| m["type"] == "feedback").await.expect("feedback from the tool");
        assert_eq!((fb["address"].as_str(), fb["device"].as_str(), fb["args"][0].as_str()), (Some("/info"), Some("x1"), Some("X32")));
        // the link is announced, and is in the device list for anyone who joins later
        assert!(wait_for(&mut rx, |m| m["type"] == "link" && m["state"] == "linked").await.is_some());
        assert_eq!(show.snapshot()["devices"][0]["link"]["state"], "linked");

        // the tool goes quiet: after the timeout the link is lost
        answering.abort();
        let lost = wait_for(&mut rx, |m| m["type"] == "link" && m["state"] == "lost").await.expect("lost");
        assert_eq!(lost["device"], "x1");
        assert_eq!(show.snapshot()["devices"][0]["link"]["state"], "lost");
    }

    #[tokio::test]
    async fn a_tool_we_only_listen_to_is_heard_and_other_senders_are_ignored() {
        let listen = free_port().await;
        let mut d = device("e1", "eos", 9);
        d.listen_port = Some(listen);
        let (every, timeout) = timing();
        let out = Output::new(vec![d], true).with_timing(every, timeout);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        // expected to talk, has not yet
        assert!(wait_for(&mut rx, |m| m["type"] == "link" && m["state"] == "silent").await.is_some());

        // a stranger on another address is not the device
        let stranger = tokio::net::UdpSocket::bind("127.0.0.2:0").await.unwrap();
        stranger.send_to(&osc_bytes("/eos/out/ping"), format!("127.0.0.1:{listen}")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(show.snapshot()["devices"][0]["link"]["state"], "silent");

        // the device itself (127.0.0.1) is heard
        let from_device = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        from_device.send_to(&osc_bytes("/eos/out/active/cue/1/2"), format!("127.0.0.1:{listen}")).await.unwrap();
        let fb = wait_for(&mut rx, |m| m["type"] == "feedback").await.expect("feedback");
        assert_eq!(fb["address"], "/eos/out/active/cue/1/2");
        assert!(wait_for(&mut rx, |m| m["type"] == "link" && m["state"] == "heard").await.is_some());
        // a tool we do not probe never becomes "lost" just because it is quiet
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        assert_eq!(show.snapshot()["devices"][0]["link"]["state"], "heard");
    }

    #[tokio::test]
    async fn a_chatty_tool_is_rate_limited_and_a_port_that_is_taken_is_reported() {
        let listen = free_port().await;
        let mut d = device("e1", "eos", 9);
        d.listen_port = Some(listen);
        let out = Output::new(vec![d], true).with_timing(timing().0, timing().1);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        assert!(wait_for(&mut rx, |m| m["type"] == "link" && m["state"] == "silent").await.is_some());
        let tool = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for _ in 0..80 {
            tool.send_to(&osc_bytes("/eos/out/meter"), format!("127.0.0.1:{listen}")).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let got = std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["type"] == "feedback").count();
        assert!(got <= 20 && got > 0, "got {got}");

        // a second show wanting a listen port that is already taken cannot have it, and the operator is told
        let mut e1 = device("e1", "eos", 9);
        e1.listen_port = Some(listen);
        let mut e2 = device("e2", "eos", 9);
        e2.listen_port = Some(listen);
        let show2 = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![e1, e2], true).with_timing(timing().0, timing().1))).unwrap();
        let mut rx2 = show2.subscribe();
        // the first show still holds the port, so BOTH devices of show2 fail to listen on it
        let err = wait_for(&mut rx2, |m| m["type"] == "error").await.expect("an error message");
        assert!(err["message"].as_str().unwrap().contains("cannot listen on port"), "{err}");
    }

    #[tokio::test]
    async fn tools_that_give_no_feedback_show_none_and_a_dry_run_shows_no_link_at_all() {
        let live = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("n1", "notch", 9)], true))).unwrap();
        assert_eq!(live.snapshot()["devices"][0]["link"]["state"], "none");
        let dry = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("n1", "notch", 9)], false))).unwrap();
        assert!(dry.snapshot()["devices"][0].get("link").is_none());
    }

    /// A measurement, not a pass/fail test: run it with
    /// `cargo test -p pchi-studio-api --lib soak -- --ignored --nocapture`.
    /// 600 cues spread over 20 s plus a burst of 200 at the same instant, across four real devices on
    /// loopback. It reports how late cues fire and whether every packet arrives.
    #[tokio::test]
    #[ignore]
    async fn soak_timing_report() {
        use std::time::Instant;
        let kinds = [("visual", "scene.activate", "scene", "2/5", "resolume"), ("lighting", "lighting.look", "look", "402", "grandma3"), ("laser", "laser.cue", "cue", "sweep", "beyond"), ("audio", "audio.snapshot", "snapshot", "3", "x32")];
        let (spread, burst, span_ms, burst_at) = (150u64, 50u64, 20_000u64, 1_000u64);
        // the chart: PRE -START-> MAIN, four lanes of cues
        let mut lanes = Vec::new();
        for (dept, kind, field, val, _) in kinds {
            let mut cues = Vec::new();
            for i in 0..spread {
                cues.push(json!({"id": format!("{dept}-{i}"), "at_ms": 1_500 + i * (span_ms / spread), "action": {"kind": kind, "payload": {field: val}}}));
            }
            for i in 0..burst {
                cues.push(json!({"id": format!("{dept}-b{i}"), "at_ms": burst_at, "action": {"kind": kind, "payload": {field: val}}}));
            }
            lanes.push(json!({"id": dept, "name": dept, "dept": dept, "cues": cues}));
        }
        let chart = Showchart::from_json(&json!({"initial": "PRE", "states": [{"id": "PRE"}, {"id": "MAIN", "timeline": {"lanes": lanes}}], "transitions": [{"source": "PRE", "target": "MAIN", "event": "START"}]}).to_string()).unwrap();

        // four listeners, one per device, noting when each packet arrives
        let mut devices = Vec::new();
        let mut counters = Vec::new();
        for (dept, _, _, _, tool) in kinds {
            let sock = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let port = sock.local_addr().unwrap().port();
            devices.push(device(&format!("{tool}-1"), tool, port));
            let arrivals = Arc::new(Mutex::new(Vec::<Instant>::new()));
            let (s, a) = (Arc::clone(&sock), Arc::clone(&arrivals));
            tokio::spawn(async move {
                let mut buf = [0u8; 512];
                while let Ok((n, _)) = s.recv_from(&mut buf).await {
                    // the X32 probe (/info every few seconds) is the runtime checking the link, not a cue
                    if !buf[..n].starts_with(b"/info") {
                        a.lock().unwrap().push(Instant::now());
                    }
                }
            });
            counters.push((dept, arrivals));
        }
        let out = Output::new(devices, true);
        let show = Show::start_with(chart, EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.set_armed("beyond-1", true).unwrap();
        let t0 = Instant::now();
        show.apply("soak", "START");
        tokio::time::sleep(Duration::from_millis(1_500 + span_ms + 1_500)).await;

        // when each cue fired, against when it was due
        let r = show.run_report();
        let mut drift: Vec<i64> = r["cues"].as_array().unwrap().iter().map(|c| c["drift_ms"].as_i64().unwrap()).collect();
        drift.sort_unstable();
        let pct = |v: &[i64], p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
        let expected = (spread + burst) as usize;
        println!("\n=== SOAK: {} cues fired (expected {}), debug build, loopback ===", drift.len(), expected * 4);
        println!("fire drift (actual - planned), ms: p50 {}  p95 {}  p99 {}  max {}", pct(&drift, 0.5), pct(&drift, 0.95), pct(&drift, 0.99), drift.last().unwrap());
        assert_eq!(drift.len(), expected * 4, "every cue must fire exactly once");
        // when each packet arrived, against the planned time of the cue it belongs to
        let mut planned: Vec<u64> = (0..spread).map(|i| 1_500 + i * (span_ms / spread)).chain((0..burst).map(|_| burst_at)).collect();
        planned.sort_unstable();
        for (dept, arrivals) in &counters {
            let mut a: Vec<u128> = arrivals.lock().unwrap().iter().map(|t| t.duration_since(t0).as_millis()).collect();
            a.sort_unstable();
            let late: Vec<i64> = a.iter().zip(&planned).map(|(x, p)| *x as i64 - *p as i64).collect();
            let mut l = late.clone();
            l.sort_unstable();
            println!("{dept:<9} packets {:>3}/{}  arrival lateness ms: p50 {}  p99 {}  max {}", a.len(), expected, pct(&l, 0.5), pct(&l, 0.99), l.last().unwrap());
            assert_eq!(a.len(), expected, "{dept}: every packet must arrive on loopback");
        }
        println!("failed or held deliveries: {}\n", r["summary"]["failed"]);
        assert_eq!(r["summary"]["failed"], 0);
        assert!(*drift.last().unwrap() < 250, "no cue may fire more than a quarter second late");
    }

    fn temp_db(tag: &str) -> String {
        std::env::temp_dir().join(format!("pchi-{tag}-{}-{}.db", std::process::id(), now_ms())).to_string_lossy().into_owned()
    }

    /// Stand in for a crash: the old show's pending cues and timers never fire, and it writes nothing more.
    fn crash(show: Arc<Show>) {
        show.inner.lock().unwrap().generation += 1_000;
    }

    #[tokio::test]
    async fn a_restarted_runtime_resumes_in_the_same_scene_and_fires_only_what_is_still_ahead() {
        let path = temp_db("resume");
        let before = Output::new(vec![device("b1", "beyond", 9)], false);
        let a = Show::start_with(quick(), EventStore::open(&path).unwrap(), Some(before)).unwrap();
        a.apply("studio", "START"); // INTRO: visual 0, lighting 0.4 s, laser 0.8 s
        a.set_context("energy", json!(0.9));
        a.set_armed("b1", true).unwrap();
        tokio::time::sleep(Duration::from_millis(550)).await;
        crash(a);

        let (b, info) = Show::resume(quick(), EventStore::open(&path).unwrap(), Some(Output::new(vec![device("b1", "beyond", 9)], false))).unwrap();
        assert_eq!(info["state"], "INTRO");
        assert!(info["skipped_cues"].as_array().unwrap().is_empty(), "{info}");
        assert_eq!(b.snapshot()["state"], "INTRO");
        assert_eq!(b.snapshot()["context"]["energy"], 0.9, "the context comes back");
        assert!(b.was_resumed());
        assert_eq!(b.snapshot()["resumed"]["state"], "INTRO");
        assert_eq!(b.snapshot()["devices"][0]["armed"], false, "arming is never restored: lasers start disarmed");
        // the cues already fired are not fired again; the one still ahead is
        let mut rx = b.subscribe();
        let m = wait_for(&mut rx, |m| m["type"] == "action").await.expect("the laser cue");
        assert_eq!(m["event"]["type"], "laser.cue");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let more = std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["type"] == "action").count();
        assert_eq!(more, 0, "nothing fires twice");
        // the log is one continuous run, with the resume noted
        let ev = b.events();
        assert_eq!(ev.iter().filter(|e| e["type"] == "run.start").count(), 1);
        assert_eq!(ev.iter().filter(|e| e["type"] == "show.resume").count(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn cues_that_came_due_while_the_runtime_was_down_are_skipped_and_listed_not_fired_late() {
        let path = temp_db("downtime");
        let a = Show::start(quick(), EventStore::open(&path).unwrap()).unwrap();
        a.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(500)).await;
        crash(a);
        tokio::time::sleep(Duration::from_millis(700)).await; // the laser cue (0.8 s) comes due while down
        let (b, info) = Show::resume(quick(), EventStore::open(&path).unwrap(), None).unwrap();
        assert_eq!(info["skipped_cues"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<Vec<_>>(), ["laser.cue at 0.8s"], "{info}");
        assert!(info["downtime_ms"].as_i64().unwrap() >= 600);
        let mut rx = b.subscribe();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["type"] == "action").count(), 0, "a skipped cue is not fired late");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn resume_refuses_when_there_is_nothing_to_resume_or_the_show_no_longer_matches() {
        let path = temp_db("refuse");
        // nothing recorded yet
        assert!(Show::resume(quick(), EventStore::open(&path).unwrap(), None).err().expect("nothing to resume").contains("no earlier run"));
        let a = Show::start(quick(), EventStore::open(&path).unwrap()).unwrap();
        a.apply("studio", "START");
        crash(a);
        // the START transition is gone from the file now given
        let mut changed = quick();
        changed.transitions.retain(|t| !(t.source == "PRESHOW" && t.event.as_deref() == Some("START")));
        let e = Show::resume(changed, EventStore::open(&path).unwrap(), None).err().expect("refused");
        assert!(e.contains("no longer matches"), "{e}");
        // but a change that does not touch the path the run took is fine (a cue edited)
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 2_000);
        assert!(Show::resume(edited, EventStore::open(&path).unwrap(), None).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn try_a_cue_shows_exactly_what_is_sent_and_respects_dry_run_arming_and_the_key() {
        // dry run: nothing leaves, but the exact message is shown
        let dry = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("q1", "qlab", 9)], false))).unwrap();
        let r = dry.try_cue("q1", "12").await.unwrap();
        assert_eq!(r["status"], "dry-run");
        assert_eq!(r["sent_as"][0]["address"], "/cue/12/start");
        // a value the tool cannot use is an error that says why
        let e = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("x1", "x32", 9)], false))).unwrap().try_cue("x1", "intro").await.unwrap_err();
        assert!(e.contains("scene number"), "{e}");

        // live: it really arrives, and a laser is held until armed
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let live = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("e1", "eos", port), device("b1", "beyond", port)], true))).unwrap();
        assert_eq!(live.try_cue("e1", "3/9").await.unwrap()["status"], "sent");
        assert_eq!(recv_osc(&sock).await, "/eos/cue/3/9/fire");
        assert_eq!(live.try_cue("b1", "sweep").await.unwrap()["status"], "held");
        live.set_armed("b1", true).unwrap();
        assert_eq!(live.try_cue("b1", "sweep").await.unwrap()["status"], "sent");
        assert_eq!(recv_osc(&sock).await, "/beyond/general/StartCue");
        assert!(live.try_cue("e1", "  ").await.is_err());
        assert!(live.try_cue("nope", "1").await.is_err());

        // over HTTP it needs the operator key
        live.set_edit_key(Some("k".into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&live), None);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(http(addr, "POST", "/devices/e1/try", r#"{"value":"1"}"#, "").await.0, 403);
        let (st, v) = http(addr, "POST", "/devices/e1/try", r#"{"value":"1"}"#, "X-Pchi-Edit-Key: k\r\n").await;
        assert_eq!((st, v["sent_as"][0]["address"].as_str()), (200, Some("/eos/cue/1/fire")));
    }

    #[tokio::test]
    async fn the_doors_check_over_http_lists_what_will_and_will_not_work() {
        // the demo show has visual, lighting and laser cues; the rig covers visuals and audio only
        let out = Output::new(vec![device("n1", "notch", 9), device("x1", "x32", 10023)], false);
        let show = Show::start_with(timelines(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), Some("tok".into()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(http(addr, "GET", "/preflight", "", "").await.0, 401);
        let (st, v) = http(addr, "GET", "/preflight?token=tok", "", "").await;
        assert_eq!(st, 200);
        assert_eq!(v["summary"]["go"], false);
        let fails: Vec<&str> = v["checks"].as_array().unwrap().iter().filter(|c| c["level"] == "fail").map(|c| c["text"].as_str().unwrap()).collect();
        assert!(fails.iter().any(|t| t.contains("no lighting device")), "{fails:?}");
        assert!(fails.iter().any(|t| t.contains("no laser device")), "{fails:?}");
        assert!(v["checks"].as_array().unwrap().iter().any(|c| c["area"] == "output" && c["level"] == "warn"), "dry run is a warning");
    }

    #[tokio::test]
    async fn the_operator_key_guards_sending_undoing_and_testing() {
        let (show, addr) = serve(None).await;
        let body = serde_json::to_string(&quick()).unwrap();
        // no key set: open, as before
        assert_eq!(http(addr, "POST", "/show?dry=1", &body, "").await.0, 200);
        show.set_edit_key(Some("s3cret".into()));
        assert_eq!(show.snapshot()["edit_locked"], true);
        assert!(!show.edit_ok(None) && !show.edit_ok(Some("nope")) && show.edit_ok(Some("s3cret")));
        let (st, v) = http(addr, "POST", "/show", &body, "").await;
        assert_eq!((st, v["locked"].as_bool()), (403, Some(true)));
        assert_eq!(http(addr, "POST", "/show/undo", "", "X-Pchi-Edit-Key: wrong\r\n").await.0, 403);
        assert_eq!(http(addr, "POST", "/devices/x/test", "", "").await.0, 403);
        // with the key in the header or the query it goes through
        assert_eq!(http(addr, "POST", "/show?dry=1", &body, "X-Pchi-Edit-Key: s3cret\r\n").await.0, 200);
        assert_eq!(http(addr, "POST", "/show?dry=1&edit_key=s3cret", &body, "").await.0, 200);
        // departments and viewers can still read and report READY
        assert_eq!(http(addr, "GET", "/state", "", "").await.0, 200);
        assert_eq!(http(addr, "POST", "/ready/lighting", r#"{"ready":true}"#, "").await.0, 200);
    }

    #[tokio::test]
    async fn a_send_can_be_undone_and_undo_has_a_limit() {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        assert_eq!(show.snapshot()["undo_depth"], 0);
        assert_eq!(show.undo(true).unwrap_err().0, 409, "nothing to undo yet");

        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 3_000);
        show.replace(edited.clone(), None, true).unwrap();
        assert_eq!(show.snapshot()["undo_depth"], 1);
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(&edited).unwrap());

        // a preview of the undo changes nothing; applying it restores the earlier show and keeps the scene
        let dry = show.undo(false).unwrap();
        assert_eq!(dry["applied"], false);
        assert!(dry["diff"]["changes"][0].as_str().unwrap().contains("moved 3.0s → 0.4s"), "{dry}");
        assert_eq!(show.snapshot()["undo_depth"], 1);
        let done = show.undo(true).unwrap();
        assert_eq!(done["undo_depth"], 0);
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(quick()).unwrap());
        assert_eq!(show.snapshot()["state"], "INTRO");

        // only the last UNDO_DEPTH sends are kept
        for i in 0..(UNDO_DEPTH + 3) {
            let mut c = quick();
            move_cue(&mut c, "INTRO", "lighting", 3_000 + i as u64 * 10);
            show.replace(c, None, true).unwrap();
        }
        assert_eq!(show.snapshot()["undo_depth"], UNDO_DEPTH);
    }

    #[tokio::test]
    async fn an_edited_cue_for_a_device_that_is_not_ready_is_flagged_before_sending() {
        let out = Output::new(vec![device("d1", "lighting-desk", 9002), device("l1", "laser", 9003)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.apply("studio", "START");
        show.set_ready("lighting", false, "fixing a fixture", "http").unwrap(); // on hold
        // the laser never reported at all

        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 5_000); // later than "now": still to fire
        move_cue(&mut edited, "INTRO", "laser", 6_000);
        let r = show.replace(edited, None, false).unwrap();
        let notes: Vec<&str> = r["diff"]["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        assert!(notes.iter().any(|n| n.contains("d1 is on hold")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("l1 has not reported READY")), "{notes:?}");

        // once they are ready, the same edit carries no such warning
        show.set_ready("lighting", true, "", "http").unwrap();
        show.set_ready("l1", true, "", "http").unwrap();
        let mut again = quick();
        move_cue(&mut again, "INTRO", "lighting", 5_000);
        move_cue(&mut again, "INTRO", "laser", 6_000);
        let r = show.replace(again, None, false).unwrap();
        assert!(r["diff"]["notes"].as_array().unwrap().is_empty(), "{r}");
    }

    #[tokio::test]
    async fn http_input_moves_the_show() {
        let (show, addr) = serve(None).await;
        let (st, body) = http(addr, "POST", "/input", r#"{"event":"START"}"#, "").await;
        assert_eq!((st, body["applied"].as_bool()), (200, Some(true)));
        assert_eq!(show.snapshot()["state"], "INTRO");
        assert_eq!(http(addr, "POST", "/input", r#"{"nope":1}"#, "").await.0, 400);
        assert_eq!(http(addr, "POST", "/ready/", r#"{}"#, "").await.0, 404);
    }

    #[tokio::test]
    async fn a_token_guards_every_door() {
        let (show, addr) = serve(Some("s3cret")).await;
        assert_eq!(http(addr, "GET", "/state", "", "").await.0, 401);
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, "").await.0, 401);
        assert_eq!(http(addr, "POST", "/ready/audio", r#"{}"#, "").await.0, 401);
        assert_eq!(show.snapshot()["state"], "PRESHOW", "nothing got through");
        assert!(connect_async(format!("ws://{addr}/ws")).await.is_err());
        assert!(connect_async(format!("ws://{addr}/ws?token=wrong")).await.is_err());

        assert_eq!(http(addr, "GET", "/state?token=s3cret", "", "").await.0, 200);
        assert_eq!(http(addr, "POST", "/ready/audio", r#"{}"#, "Authorization: Bearer s3cret\r\n").await.0, 200);
        assert!(connect_async(format!("ws://{addr}/ws?token=s3cret")).await.is_ok());
    }

    // ---- governance: who may do what, and the signed record ----

    /// A show with a (dry-run) laser, the operator key "k", and receipts written to a fresh temp folder.
    async fn governed(name: &str) -> (Arc<Show>, std::net::SocketAddr) {
        let dir = std::env::temp_dir().join(format!("pchi-gov-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = Output::new(vec![device("b1", "beyond", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.set_edit_key(Some("k".into()));
        let log = primeswarm_pchi::GovernanceLog::open(dir.join("key.hex").to_str().unwrap(), dir.join("log.jsonl").to_str().unwrap()).unwrap();
        show.set_governance(log);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), None);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (show, addr)
    }

    fn receipts(show: &Show) -> Vec<(String, String, String)> {
        show.governance_export()["receipts"].as_array().unwrap().iter().map(|r| (r["rule_name"].as_str().unwrap().into(), r["gate_state"].as_str().unwrap().into(), r["message"].as_str().unwrap().into())).collect()
    }

    const AGENT: &str = "X-Pchi-Actor: agent:nova\r\n";
    const KEY: &str = "X-Pchi-Edit-Key: k\r\n";

    #[tokio::test]
    async fn an_agent_may_propose_but_not_act_and_every_decision_is_written_down() {
        let (show, addr) = governed("agent").await;
        let body = |c: &Showchart| serde_json::to_string(c).unwrap();
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 3_000);
        let before = show.snapshot()["definition"].clone();

        // an agent may preview, even without the key
        assert_eq!(http(addr, "POST", "/show?dry=1", &body(&edited), AGENT).await.0, 200);
        // its real send is only a proposal: 202, nothing changes, and it waits for a person
        let (st, v) = http(addr, "POST", "/show", &body(&edited), AGENT).await;
        assert_eq!((st, v["proposed"].as_bool(), v["id"].as_u64()), (202, Some(true), Some(1)), "{v}");
        assert_eq!(show.snapshot()["definition"], before, "an agent's send must not change the running show");
        assert_eq!(show.snapshot()["proposals"].as_array().unwrap().len(), 1);
        assert!(v["diff"]["changes"][0].as_str().unwrap().contains("moved"), "{v}");
        // the same agent holding the key is still an agent
        let (st, _) = http(addr, "POST", "/devices/b1/arm?armed=1", "", &format!("{AGENT}{KEY}")).await;
        assert_eq!(st, 403);
        assert_eq!(http(addr, "POST", "/show/undo", "", &format!("{AGENT}{KEY}")).await.0, 403);
        assert_eq!(http(addr, "POST", "/devices/b1/try", r#"{"value":"x"}"#, &format!("{AGENT}{KEY}")).await.0, 403);
        assert_eq!(http(addr, "POST", "/devices/b1/test", "", &format!("{AGENT}{KEY}")).await.0, 403);
        // nor may it press GO, over HTTP or the WebSocket
        let (st, v) = http(addr, "POST", "/input", r#"{"event":"START"}"#, AGENT).await;
        assert_eq!((st, v["denied"].as_bool()), (403, Some(true)));
        let (mut ws, _) = connect_async(format!("ws://{addr}/ws?actor=agent:nova")).await.unwrap();
        ws.send(TMsg::Text(r#"{"type":"input","event":"START"}"#.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(show.snapshot()["state"], "PRESHOW", "an agent must not move the show");
        // the safe direction is open to it: disarm and the hard stop
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=0", "", AGENT).await.0, 200);
        assert_eq!(http(addr, "POST", "/laser/stop", "", AGENT).await.0, 200);

        let r = receipts(&show);
        let has = |rule: &str, gate: &str| r.iter().any(|(ru, g, m)| ru == rule && g == gate && m.contains("agent:nova"));
        assert!(has("show.propose", "ESCALATE") && has("laser.arm", "DENY") && has("show.undo", "DENY") && has("device.try", "DENY") && has("device.test", "DENY") && has("show.go", "DENY"), "{r:?}");
        assert!(has("laser.disarm", "ALLOW") && has("laser.stop", "ALLOW"), "{r:?}");
        assert!(!r.iter().any(|(ru, g, _)| ru == "show.edit" && g == "ALLOW"), "no edit was allowed");
    }

    #[tokio::test]
    async fn a_person_approves_or_rejects_a_proposal_and_stale_ones_are_superseded() {
        let (show, addr) = governed("approve").await;
        let body = |c: &Showchart| serde_json::to_string(c).unwrap();
        let (mut a, mut b, mut c) = (quick(), quick(), quick());
        move_cue(&mut a, "INTRO", "lighting", 3_000);
        move_cue(&mut b, "INTRO", "visual", 1_000);
        move_cue(&mut c, "INTRO", "laser", 2_000);
        for chart in [&a, &b, &c] {
            assert_eq!(http(addr, "POST", "/show", &body(chart), AGENT).await.0, 202);
        }
        assert_eq!(show.snapshot()["proposals"].as_array().unwrap().len(), 3);

        // deciding needs a person with the key
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", "").await.0, 403);
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", &format!("{AGENT}{KEY}")).await.0, 403, "an agent cannot approve its own proposal");
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(quick()).unwrap());
        // reject one
        let (st, _) = http(addr, "POST", "/proposals/3/reject", "", KEY).await;
        assert_eq!(st, 200);
        // approve another: it is applied, and the one made against the old show is superseded
        let (st, v) = http(addr, "POST", "/proposals/1/approve", "", KEY).await;
        assert_eq!((st, v["applied"].as_bool()), (200, Some(true)), "{v}");
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(&a).unwrap());
        let list = http(addr, "GET", "/proposals", "", "").await.1;
        let status = |id: u64| list.as_array().unwrap().iter().find(|p| p["id"] == id).unwrap()["status"].as_str().unwrap().to_string();
        assert_eq!((status(1).as_str(), status(2).as_str(), status(3).as_str()), ("approved", "superseded", "rejected"));
        assert_eq!(http(addr, "POST", "/proposals/2/approve", "", KEY).await.0, 409, "a superseded proposal cannot be approved");
        assert_eq!(http(addr, "POST", "/proposals/9/approve", "", KEY).await.0, 404);
        assert_eq!(show.snapshot()["proposals"].as_array().unwrap().len(), 0, "nothing is left waiting");

        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.edit" && g == "ALLOW" && m.contains("approved proposal #1 from agent:nova")), "{r:?}");
        assert!(r.iter().any(|(ru, g, _)| ru == "show.reject" && g == "DENY"), "{r:?}");
    }

    #[tokio::test]
    async fn the_receipts_form_a_signed_chain_that_names_who_did_what() {
        use primeswarm_pchi::governance::{verify_receipt, GovernanceReceipt};
        let (show, addr) = governed("chain").await;
        let body = serde_json::to_string(&quick()).unwrap();
        // a person's actions
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=1", "", "X-Pchi-Edit-Key: k\r\nX-Pchi-Actor: person:Alex\r\n").await.0, 200);
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 3_000);
        assert_eq!(http(addr, "POST", "/show", &serde_json::to_string(&edited).unwrap(), KEY).await.0, 200);
        assert_eq!(http(addr, "POST", "/show", &body, "").await.0, 403, "no key: refused, and written down");
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, KEY).await.0, 200);
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"NO_SUCH_EVENT"}"#, KEY).await.0, 200);

        let export = show.governance_export();
        let recs: Vec<GovernanceReceipt> = serde_json::from_value(export["receipts"].clone()).unwrap();
        let rules: Vec<&str> = recs.iter().map(|r| r.rule_name.as_str()).collect();
        assert_eq!(rules, ["laser.arm", "show.edit", "show.edit", "show.go"], "{recs:?}");
        assert_eq!(recs[0].message, "person:Alex: armed b1");
        assert_eq!(recs[2].gate_state, "DENY");
        assert!(recs[3].message.contains("moved the show to"), "only an input that moved the show is recorded");
        let key = export["verifying_key"].as_str().unwrap();
        let mut prev: Option<String> = None;
        for r in &recs {
            assert!(verify_receipt(r, key), "signature and hash of {}", r.receipt_id);
            assert_eq!(r.parent_decision_hash, prev, "chain link of {}", r.receipt_id);
            prev = Some(r.decision_hash.clone());
        }
        // each is decided against the state at that moment: the armed laser and the edited show change it
        assert_ne!(recs[0].state_snapshot_hash, recs[1].state_snapshot_hash);
        assert_ne!(recs[2].state_snapshot_hash, recs[3].state_snapshot_hash);
        // tampering with any receipt is detectable
        let mut forged = recs[2].clone();
        forged.gate_state = "ALLOW".into();
        assert!(!verify_receipt(&forged, key));
    }

    #[tokio::test]
    async fn without_receipts_agents_are_still_held_back_and_nothing_is_written() {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        assert!(show.receipt("x", "ALLOW", &Actor::Person(None), "y").is_none());
        let ex = show.governance_export();
        assert_eq!(ex["count"], 0);
        assert!(ex["note"].as_str().unwrap().contains("--governance"));
        assert_eq!(show.snapshot()["governed"], false);
        let (a, p) = (Actor::parse(Some("Agent:Nova")), Actor::parse(Some("person:Alex")));
        assert_eq!((a.name(), p.name(), Actor::parse(None).name()), ("agent:Nova".to_string(), "person:Alex".to_string(), "operator".to_string()));
        assert!(Actor::parse(Some("agent")).is_agent() && !Actor::parse(Some("robot")).is_agent());
    }

    #[tokio::test]
    async fn rehearsing_is_read_only_so_an_agent_may_try_an_idea_before_proposing_it() {
        let (show, addr) = governed("rehearse").await;
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let same = serde_json::to_string(&quick()).unwrap();
        // no operator key, and an agent: still allowed, and it changes nothing
        let (st, v) = http(addr, "POST", "/rehearse", &same, AGENT).await;
        assert_eq!(st, 200, "{v}");
        assert_eq!(v["recorded_path"], json!(["PRESHOW", "INTRO"]));
        assert_eq!(v["run"]["inputs"], 1);
        // a cue pushed far past anything the run has reached: it has not fired, and the show is untouched
        let mut later = quick();
        move_cue(&mut later, "INTRO", "lighting", 60_000);
        let (st, v) = http(addr, "POST", "/rehearse", &serde_json::to_string(&later).unwrap(), AGENT).await;
        assert_eq!(st, 200);
        assert_eq!(v["summary"]["different"], true, "{v}");
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(quick()).unwrap(), "a rehearsal must not change the show");
        assert!(show.snapshot()["proposals"].as_array().unwrap().is_empty());
        assert_eq!(http(addr, "POST", "/rehearse", "not json", AGENT).await.0, 400);
    }

    // ---- roles: named people, each with their own key and what that key allows ----

    const ROSTER: &str = r#"{"people": [
        {"name": "Alex", "role": "operator", "key": "alex-key-123"},
        {"name": "Eddie", "role": "editor", "key": "eddie-key-123"},
        {"name": "Sam", "role": "lso", "key": "sam-key-1234"},
        {"name": "Gus", "role": "guest", "key": "gus-key-1234"},
        {"name": "Nova", "role": "agent", "key": "nova-key-123"}]}"#;

    fn k(key: &str) -> String {
        format!("X-Pchi-Edit-Key: {key}\r\n")
    }

    /// A show with a laser (b1) and a lighting desk (e1), the roster above, the legacy operator key "legacy-key", and receipts.
    async fn rostered(name: &str) -> (Arc<Show>, std::net::SocketAddr) {
        let dir = std::env::temp_dir().join(format!("pchi-roles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = Output::new(vec![device("b1", "beyond", 9), device("e1", "eos", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        show.set_edit_key(Some("legacy-key".into()));
        show.set_roster(governance::Roster::parse(ROSTER).unwrap());
        show.set_governance(primeswarm_pchi::GovernanceLog::open(dir.join("key.hex").to_str().unwrap(), dir.join("log.jsonl").to_str().unwrap()).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), None);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (show, addr)
    }

    #[test]
    fn a_roster_must_be_sound() {
        let bad = |t: &str| governance::Roster::parse(t).err().unwrap_or_default();
        assert!(bad(r#"[{"name":"A","role":"boss","key":"12345678"}]"#).contains("role 'boss'"));
        assert!(bad(r#"[{"name":"A","role":"guest","key":"short"}]"#).contains("at least 8"));
        assert!(bad(r#"[{"name":"","role":"guest","key":"12345678"}]"#).contains("no name"));
        assert!(bad(r#"[{"name":"A","role":"guest","key":"12345678"},{"name":"a","role":"guest","key":"87654321"}]"#).contains("listed twice"));
        assert!(bad(r#"[{"name":"A","role":"guest","key":"12345678"},{"name":"B","role":"guest","key":"12345678"}]"#).contains("same key"));
        assert!(bad("nope").contains("not valid JSON"));
        let ok = governance::Roster::parse(ROSTER).unwrap();
        assert_eq!(ok.len(), 5);
        assert!(ok.has_role("lso") && ok.summary().contains("Eddie (editor)"));
        assert!(governance::Roster::parse(r#"[{"name":"A","role":"operator","key":"12345678"}]"#).is_ok(), "a bare list works too");
    }

    #[tokio::test]
    async fn each_role_may_do_what_it_is_given_and_no_more() {
        let (_show, addr) = rostered("matrix").await;
        let who = |key: &str| {
            let h = k(key);
            async move { http(addr, "GET", "/whoami", "", &h).await.1 }
        };
        let body = serde_json::to_string(&quick()).unwrap();
        let go = r#"{"event":"START"}"#;
        let status = |key: &'static str, method: &'static str, path: &'static str, b: &'static str| {
            let h = k(key);
            let b = b.to_string();
            async move { http(addr, method, path, &b, &h).await.0 }
        };

        // who the runtime takes each key to be
        assert_eq!(who("alex-key-123").await["name"], "Alex (operator)");
        assert_eq!(who("legacy-key").await["role"], "operator", "the single operator key still works");
        let nobody = http(addr, "GET", "/whoami", "", "").await.1;
        assert_eq!((nobody["signed_in"].clone(), nobody["can"].clone()), (json!(false), json!([])));
        assert_eq!(who("gus-key-1234").await["can"], json!([]));
        assert_eq!(who("eddie-key-123").await["can"], json!(["edit", "approve", "undo", "try", "test"]));
        assert_eq!(who("sam-key-1234").await["can"], json!(["arm", "try", "test"]));
        // a label on the request cannot change who you are
        let (_, w) = http(addr, "GET", "/whoami", "", "X-Pchi-Edit-Key: eddie-key-123\r\nX-Pchi-Actor: person:Alex\r\n").await;
        assert_eq!(w["name"], "Eddie (editor)");
        // and an agent stays an agent whatever key it holds
        let (_, w) = http(addr, "GET", "/whoami", "", &format!("{AGENT}{}", k("alex-key-123"))).await;
        assert_eq!(w["role"], "agent");

        // nobody signed in: nothing, and GO needs a key too once there is a roster
        assert_eq!(http(addr, "POST", "/input", go, "").await.0, 403);
        assert_eq!(http(addr, "POST", "/show", &body, "").await.0, 403);
        // a guest may look but not touch
        let (st, v) = http(addr, "POST", "/input", go, &k("gus-key-1234")).await;
        assert_eq!((st, v["role"].clone(), v["denied"].clone()), (403, json!("guest"), json!(true)), "{v}");
        assert_eq!(status("gus-key-1234", "POST", "/rehearse", "{}").await, 400, "rehearse is open to a guest (an empty body is just a bad request)");
        assert_eq!(status("gus-key-1234", "POST", "/devices/b1/arm?armed=0", "").await, 200, "anyone may disarm");
        assert_eq!(status("gus-key-1234", "POST", "/laser/stop", "").await, 200, "anyone may stop the lasers");

        // an editor edits and decides proposals, but does not run the show or arm a laser
        assert_eq!(http(addr, "POST", "/show", &body, &k("eddie-key-123")).await.0, 200);
        assert_eq!(status("eddie-key-123", "POST", "/input", go).await, 403);
        assert_eq!(status("eddie-key-123", "POST", "/devices/b1/arm?armed=1", "").await, 403);
        assert_eq!(http(addr, "POST", "/devices/e1/try", r#"{"value":"1"}"#, &k("eddie-key-123")).await.0, 200);

        // the laser safety officer arms and tests lasers, and nothing else
        assert_eq!(status("sam-key-1234", "POST", "/devices/b1/arm?armed=1", "").await, 200);
        assert_eq!(http(addr, "POST", "/devices/b1/try", r#"{"value":"x"}"#, &k("sam-key-1234")).await.0, 200);
        assert_eq!(http(addr, "POST", "/devices/e1/try", r#"{"value":"1"}"#, &k("sam-key-1234")).await.0, 403, "not a laser");
        assert_eq!(status("sam-key-1234", "POST", "/devices/e1/test", "").await, 403);
        assert_eq!(status("sam-key-1234", "POST", "/input", go).await, 403);
        assert_eq!(http(addr, "POST", "/show", &body, &k("sam-key-1234")).await.0, 403);

        // the operator does all of it
        assert_eq!(status("alex-key-123", "POST", "/input", go).await, 200);
        assert_eq!(status("alex-key-123", "POST", "/devices/b1/arm?armed=1", "").await, 200);
    }

    #[tokio::test]
    async fn a_named_agent_proposes_under_its_own_name_and_a_person_of_the_right_role_approves() {
        let (show, addr) = rostered("agent").await;
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 3_000);
        let (st, v) = http(addr, "POST", "/show", &serde_json::to_string(&edited).unwrap(), &k("nova-key-123")).await;
        assert_eq!((st, v["proposal"]["actor"].clone()), (202, json!("agent:Nova")), "{v}");
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, &k("nova-key-123")).await.0, 403);
        // a guest cannot approve; an editor can; the receipt says who
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", &k("gus-key-1234")).await.0, 403);
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", &k("eddie-key-123")).await.0, 200);
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(&edited).unwrap());
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.edit" && g == "ALLOW" && m.starts_with("Eddie (editor): approved proposal #1 from agent:Nova")), "{r:?}");
        assert!(r.iter().any(|(ru, g, m)| ru == "show.edit" && g == "DENY" && m.contains("Gus (guest)") || ru == "show.edit" && g == "DENY" && m.contains("guest may not")), "{r:?}");
    }

    #[tokio::test]
    async fn the_run_report_says_who_did_what() {
        let (show, addr) = rostered("report").await;
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, &k("alex-key-123")).await.0, 200);
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=1", "", &k("sam-key-1234")).await.0, 200);
        assert_eq!(http(addr, "POST", "/devices/b1/arm?armed=1", "", &k("gus-key-1234")).await.0, 403);
        let d = show.run_report()["decisions"].as_array().unwrap().clone();
        let line = |rule: &str, gate: &str| d.iter().find(|x| x["rule"] == rule && x["gate"] == gate).map(|x| x["by"].as_str().unwrap().to_string());
        assert_eq!(line("show.go", "ALLOW").as_deref(), Some("Alex (operator)"));
        assert_eq!(line("laser.arm", "ALLOW").as_deref(), Some("Sam (lso)"));
        assert_eq!(line("laser.arm", "DENY").as_deref(), Some("Gus (guest)"));
    }

    #[tokio::test]
    async fn a_narrated_line_travels_with_its_proposal_with_the_rehearsal_and_into_the_record() {
        let (show, addr) = rostered("said").await;
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, &k("alex-key-123")).await.0, 200);
        tokio::time::sleep(Duration::from_millis(700)).await; // the 0.4 s lighting cue has fired in the real run
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "lighting", 60_000);
        let body = serde_json::to_string(&edited).unwrap();

        // an editor asks for a second pair of eyes instead of applying; the line they said goes along
        let (st, v) = http(addr, "POST", "/show?propose=1&said=bring%20the%20lights%20in%20much%20later", &body, &k("eddie-key-123")).await;
        assert_eq!((st, v["proposal"]["actor"].clone(), v["proposal"]["said"].clone()), (202, json!("Eddie (editor)"), json!("bring the lights in much later")), "{v}");
        assert_eq!(show.snapshot()["definition"], serde_json::to_value(quick()).unwrap(), "proposed, not applied");
        // the person deciding sees how it would have played against the run so far
        let rehearsal = &v["proposal"]["rehearsal"];
        assert_eq!(rehearsal["different"], true, "{v}");
        assert!(rehearsal["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("would NOT fire")), "{rehearsal}");
        // someone with no right to edit cannot even propose
        assert_eq!(http(addr, "POST", "/show?propose=1", &body, &k("gus-key-1234")).await.0, 403);

        // an operator approves; the record carries the words
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", &k("alex-key-123")).await.0, 200);
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.propose" && g == "ESCALATE" && m.contains("— said: \"bring the lights in much later\"")), "{r:?}");
        assert!(r.iter().any(|(ru, g, m)| ru == "show.edit" && g == "ALLOW" && m.starts_with("Alex (operator): approved proposal #1 from Eddie (editor)") && m.contains("bring the lights in much later")), "{r:?}");

        // a direct send by someone allowed to carries the line too, and an agent's reason is kept the same way
        let (st, _) = http(addr, "POST", "/show?said=put%20it%20back", &serde_json::to_string(&quick()).unwrap(), &k("alex-key-123")).await;
        assert_eq!(st, 200);
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.edit" && g == "ALLOW" && m.contains("sent an edited show") && m.contains("— said: \"put it back\"")), "{r:?}");
        let (st, v) = http(addr, "POST", "/show?said=the%20drop%20needs%20more%20air", &body, &k("nova-key-123")).await;
        assert_eq!((st, v["proposal"]["said"].clone()), (202, json!("the drop needs more air")));
    }

    #[test]
    fn a_said_line_is_made_safe_to_keep() {
        assert_eq!(governance::clean_said(Some("  hello\nthere\t! ")), "hellothere!");
        assert_eq!(governance::clean_said(None), "");
        assert_eq!(governance::clean_said(Some(&"x".repeat(500))).len(), 300);
        assert_eq!(governance::said_suffix(""), "");
    }

    // ---- a scene's minimum: an input that would leave it too early is held ----

    fn with_min(ms: u64) -> Showchart {
        let mut c = quick();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().min_ms = Some(ms);
        c
    }

    #[test]
    fn a_timer_shorter_than_the_scene_minimum_is_a_mistake() {
        let mut c = with_min(5_000);
        let mut t = c.transitions.iter().find(|t| t.source == "INTRO").unwrap().clone();
        t.event = None;
        t.after_ms = Some(2_000);
        c.transitions.push(t);
        let errs = c.validate().iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        assert!(errs.contains("timer after 2000 ms but must run at least 5000 ms"), "{errs}");
        c.transitions.last_mut().unwrap().after_ms = Some(5_000);
        assert!(c.validate().is_empty(), "a timer at the minimum is fine");
    }

    #[tokio::test]
    async fn an_input_before_the_scene_minimum_is_held_and_the_scenes_own_timer_is_not() {
        let show = Show::start(with_min(700), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        // START leaves PRESHOW, which has no minimum
        assert_eq!(show.apply_checked("studio", "START"), Applied::Moved);
        // DROP at once: INTRO must run 0.7 s
        let held = show.apply_checked("studio", "DROP");
        let Applied::Held { remaining_ms, min_ms, ref scene } = held else { panic!("{held:?}") };
        assert!(remaining_ms <= 700 && remaining_ms > 400 && min_ms == 700 && scene == "INTRO", "{held:?}");
        assert!(held.held_text().unwrap().contains("INTRO must run"), "{held:?}");
        assert_eq!(show.snapshot()["state"], "INTRO", "a held press changes nothing");
        // it was reported to clients and written into the run
        let msgs: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(msgs.iter().any(|m| m["type"] == "held" && m["event"] == "DROP"), "{msgs:?}");
        assert!(show.events().iter().any(|e| e["type"] == "show.held"));
        // an event with no transition is just ignored, not "held"
        assert_eq!(show.apply_checked("studio", "NOPE"), Applied::Ignored);
        // after the minimum it goes through
        tokio::time::sleep(Duration::from_millis(750)).await;
        assert_eq!(show.apply_checked("studio", "DROP"), Applied::Moved);
        assert_eq!(show.snapshot()["state"], "DROP");
    }

    #[tokio::test]
    async fn the_http_input_says_how_long_to_wait_and_the_record_says_it_was_held() {
        let (show, addr) = governed("held").await;
        let mut c = with_min(5_000);
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().min_ms = Some(5_000);
        show.replace(c, None, true).unwrap();
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, KEY).await.0, 200);
        let (st, v) = http(addr, "POST", "/input", r#"{"event":"DROP"}"#, KEY).await;
        assert_eq!((st, v["applied"].clone(), v["held"]["min_ms"].clone()), (200, json!(false), json!(5000)), "{v}");
        assert!(v["held"]["remaining_ms"].as_u64().unwrap() > 4_000);
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.go" && g == "DENY" && m.contains("INTRO must run")), "{r:?}");
    }

    // ---- OSC input: a Stream Deck or tablet presses things, and only presses ----

    fn osc(addr: &str, args: Vec<rosc::OscType>) -> Vec<u8> {
        rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: addr.into(), args })).unwrap()
    }

    async fn deck(show: &Arc<Show>, allow: &str) -> (Arc<inputs::Input>, std::net::SocketAddr) {
        let text = format!(r#"[{{"name":"Stream Deck","listen":"127.0.0.1:0","allow":[{allow}],"bindings":[
            {{"address":"/go","event":"START"}},{{"address":"/drop","event":"DROP"}},
            {{"address":"/energy","context":"energy"}},{{"address":"/stop","action":"laser.stop"}}]}}]"#);
        let spec = inputs::parse(&text).unwrap().remove(0);
        let input = inputs::start(show, spec).await.unwrap();
        let addr = input.bound.lock().unwrap().unwrap();
        (input, addr)
    }

    async fn send_osc(to: std::net::SocketAddr, addr: &str, args: Vec<rosc::OscType>) {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.send_to(&osc(addr, args), to).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    #[tokio::test]
    async fn a_stream_deck_presses_events_sets_values_and_stops_lasers_and_the_record_names_it() {
        let (show, _) = governed("deck").await;
        show.set_edit_key(None);
        let (input, to) = deck(&show, "").await;
        // a press moves the show; the same press again finds no transition
        send_osc(to, "/go", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["state"], "INTRO");
        send_osc(to, "/go", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["state"], "INTRO");
        // releasing the button (0) is not a press
        send_osc(to, "/drop", vec![rosc::OscType::Int(0)]).await;
        assert_eq!(show.snapshot()["state"], "INTRO");
        send_osc(to, "/drop", vec![rosc::OscType::Float(1.0)]).await;
        assert_eq!(show.snapshot()["state"], "DROP");
        // a fader sets a context value
        send_osc(to, "/energy", vec![rosc::OscType::Float(0.8)]).await;
        assert!((show.snapshot()["context"]["energy"].as_f64().unwrap() - 0.8).abs() < 1e-6, "{}", show.snapshot()["context"]);
        // an address with no binding is counted, never acted on
        send_osc(to, "/arm", vec![rosc::OscType::Int(1)]).await;
        // STOP LASERS
        show.set_armed("b1", true).unwrap();
        send_osc(to, "/stop", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["devices"][0]["armed"], false, "the hard stop disarmed the laser");

        let j = input.json();
        assert_eq!((j["unbound"].clone(), j["last_address"].clone()), (json!(1), json!("/stop")), "the unbound /arm was counted, then /stop was the last heard: {j}");
        assert!(j["pressed"].as_u64().unwrap() >= 4, "{j}");
        assert_eq!(show.snapshot()["inputs"][0]["name"], "Stream Deck");
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.go" && g == "ALLOW" && m.starts_with("Stream Deck (button): /go sent 'START': moved the show to INTRO")), "{r:?}");
        assert!(r.iter().any(|(ru, g, m)| ru == "laser.stop" && g == "ALLOW" && m.starts_with("Stream Deck (button): hard stop from /stop")), "{r:?}");
    }

    #[tokio::test]
    async fn an_input_can_only_press_whatever_its_bindings_say() {
        let (show, _) = rostered("deckrole").await;
        let (input, _) = deck(&show, "").await;
        let a = input.actor();
        assert_eq!(a.name(), "Stream Deck (button)");
        assert!(show.allow(&a, governance::Perm::Go, None));
        for p in [governance::Perm::Edit, governance::Perm::Approve, governance::Perm::Undo, governance::Perm::Arm, governance::Perm::Try, governance::Perm::Test] {
            assert!(!show.allow(&a, p, Some("laser")), "{p:?}");
        }
        // an input cannot be bound to anything but a press, a value or the hard stop
        assert!(inputs::parse(r#"[{"name":"X","listen":"127.0.0.1:1","bindings":[{"address":"/a","action":"laser.arm"}]}]"#).is_err());
    }

    #[tokio::test]
    async fn datagrams_from_unlisted_senders_are_dropped_and_a_flood_is_capped() {
        let (show, _) = governed("deckallow").await;
        show.set_edit_key(None);
        let (input, to) = deck(&show, r#""10.9.9.9""#).await;
        send_osc(to, "/go", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["state"], "PRESHOW", "127.0.0.1 is not on the allow list");
        assert_eq!(input.json()["unlisted_sender"], 1);

        let (show2, _) = governed("deckflood").await;
        show2.set_edit_key(None);
        let (flooded, to2) = deck(&show2, "").await;
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for i in 0..300 {
            sock.send_to(&osc("/energy", vec![rosc::OscType::Float(i as f32)]), to2).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let j = flooded.json();
        assert!(j["dropped"].as_u64().unwrap() > 0, "more than 100 a second are dropped: {j}");
        assert!(j["received"].as_u64().unwrap() <= 100, "{j}");
    }

    #[tokio::test]
    async fn a_press_before_the_scene_minimum_is_held_for_a_button_too() {
        let (show, _) = governed("deckheld").await;
        show.set_edit_key(None);
        show.replace(with_min(5_000), None, true).unwrap();
        let (_input, to) = deck(&show, "").await;
        send_osc(to, "/go", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["state"], "INTRO");
        send_osc(to, "/drop", vec![rosc::OscType::Int(1)]).await;
        assert_eq!(show.snapshot()["state"], "INTRO", "INTRO must run 5 s first");
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.go" && g == "DENY" && m.contains("/drop sent 'DROP' held: INTRO must run")), "{r:?}");
    }

    // ---- timecode: cues and scene changes locked to an outside clock ----

    /// INTRO locked to 00:01:00:00 (cues at 0, 0.4 and 0.8 s), leaving on 00:01:05:00; DROP locked to the same moment.
    fn timecoded() -> Showchart {
        let mut c = quick();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().tc_start = Some("00:01:00:00".into());
        let drop = c.states.iter_mut().find(|s| s.id == "DROP").unwrap();
        drop.timeline.as_mut().unwrap().tc_start = Some("00:01:05:00".into());
        c.transitions.push(pchi_showchart::Transition::on("INTRO", "DROP", "tc:00:01:05:00"));
        c
    }

    fn cue_ids(rx: &mut broadcast::Receiver<Value>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok()).filter(|m| m["type"] == "action").filter_map(|m| m["cue"].as_str().map(String::from)).collect()
    }

    fn feed(show: &Arc<Show>, positions: &[f64]) {
        for p in positions {
            show.timecode_update(*p, Fps::F25, "test");
        }
    }

    #[tokio::test]
    async fn a_scene_locked_to_timecode_fires_its_cues_as_the_timecode_passes_them_not_on_the_runtime_clock() {
        let show = Show::start(timecoded(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        // the runtime's own clock would have fired all three by now: timecode has not moved, so nothing fires
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        assert!(cue_ids(&mut rx).is_empty(), "a locked scene waits for its timecode");
        // timecode arrives and plays through the cues
        feed(&show, &[59_960.0, 60_000.0]);
        assert_eq!(cue_ids(&mut rx), ["v1"], "the cue at the lock position fires as it is reached");
        feed(&show, &[60_040.0, 60_080.0, 60_120.0, 60_160.0, 60_200.0, 60_240.0, 60_280.0, 60_320.0, 60_360.0, 60_400.0, 60_400.0, 60_440.0]);
        assert_eq!(cue_ids(&mut rx), ["l1"], "once, at 0.4 s: repeating the same position fires nothing more");
        feed(&show, &[60_480.0, 60_520.0, 60_560.0, 60_600.0, 60_640.0, 60_680.0, 60_720.0, 60_760.0, 60_800.0]);
        assert_eq!(cue_ids(&mut rx), ["z1"]);
        // the run report says it was timecode, and how far into the scene the timecode was
        let report = show.run_report();
        let fired: Vec<&Value> = report["cues"].as_array().unwrap().iter().filter(|c| c["timecode"] == true).collect();
        assert_eq!(fired.len(), 3);
        assert!(fired.iter().all(|c| c["drift_ms"].as_i64().unwrap().abs() < 40), "{fired:?}");
    }

    #[tokio::test]
    async fn a_timecode_transition_changes_scene_and_the_new_scenes_cues_are_due_in_the_same_step() {
        let mut c = timecoded();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().min_ms = Some(60_000); // people must wait; the timecode need not
        let show = Show::start(c, EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        feed(&show, &[64_960.0, 65_040.0]);
        assert_eq!(show.snapshot()["state"], "DROP", "tc:00:01:05:00 moved the show");
        assert!(cue_ids(&mut rx).contains(&"v2".to_string()), "DROP's first cue is at its lock position, which this step passed");
        // the move is the show's own clock, so the scene minimum did not hold it
        assert!(!show.events().iter().any(|e| e["type"] == "show.held"));
    }

    #[tokio::test]
    async fn a_locate_fires_nothing_and_says_what_it_jumped_over_and_going_back_lets_cues_fire_again() {
        let show = Show::start(timecoded(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        feed(&show, &[59_960.0, 60_000.0]);
        assert_eq!(cue_ids(&mut rx), ["v1"]);
        // the operator locates the playback forward past two cues
        feed(&show, &[62_000.0]);
        assert!(cue_ids(&mut rx).is_empty(), "a jump fires nothing");
        let msgs: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let _ = msgs;
        assert!(show.events().iter().any(|e| e["type"] == "show.tc_jump" && e["payload"]["skipped"] == 2), "{:?}", show.events().iter().filter(|e| e["type"] == "show.tc_jump").collect::<Vec<_>>());
        // and back: playing through the cues again fires them
        feed(&show, &[60_300.0, 60_340.0, 60_380.0, 60_420.0]);
        assert_eq!(cue_ids(&mut rx), ["l1"]);
        feed(&show, &[60_000.0]);
        feed(&show, &[60_040.0]);
        let _ = cue_ids(&mut rx);
        // rewound to the start, v1 fires again as the position passes it
        feed(&show, &[59_700.0, 59_900.0, 60_000.0]);
        assert!(cue_ids(&mut rx).contains(&"v1".to_string()));
    }

    #[tokio::test]
    async fn the_show_says_whether_timecode_is_running_and_the_doors_check_asks_for_it() {
        let show = Show::start(timecoded(), EventStore::open_in_memory().unwrap()).unwrap();
        let level = |show: &Show| show.preflight()["checks"].as_array().unwrap().iter().find(|c| c["area"] == "timecode").map(|c| c["level"].as_str().unwrap().to_string());
        assert_eq!(show.snapshot()["timecode"]["state"], "none");
        assert_eq!(level(&show).as_deref(), Some("warn"), "a show that locks to timecode with none heard");
        feed(&show, &[1_000.0, 1_040.0]);
        let tc = show.snapshot()["timecode"].clone();
        assert_eq!((tc["state"].clone(), tc["fps"].clone(), tc["position"].clone()), (json!("running"), json!("25"), json!("00:00:01:01")), "{tc}");
        assert_eq!(level(&show).as_deref(), Some("ok"));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(show.snapshot()["timecode"]["state"], "stopped");
        assert_eq!(level(&show).as_deref(), Some("warn"));
        // a show that does not use timecode is not nagged about it
        let plain = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        assert_eq!(level(&plain), None);
    }

    #[test]
    fn timecode_written_in_a_show_must_be_readable() {
        let mut c = timecoded();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().tc_start = Some("one minute".into());
        c.transitions.last_mut().unwrap().event = Some("tc:1:2".into());
        let errs = c.validate().iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        assert!(errs.contains("'one minute' (INTRO timeline) is not a timecode") && errs.contains("'1:2' (transition INTRO -> DROP) is not a timecode"), "{errs}");
        assert!(timecoded().validate().is_empty());
    }

    // ---- MIDI and OSC as sources: presses, values and timecode ----

    fn midi_deck(show: &Arc<Show>, bindings: &str) -> Arc<midi::MidiInput> {
        let spec: midi::MidiSpec = serde_json::from_str(&format!(r#"{{"name":"Pedal","bindings":[{bindings}]}}"#)).unwrap();
        midi::check(std::slice::from_ref(&spec)).unwrap();
        let input = midi::MidiInput::new(spec);
        show.add_input(Arc::clone(&input) as Arc<dyn inputs::InputInfo>);
        input
    }

    #[tokio::test]
    async fn midi_notes_programs_and_controllers_press_set_values_and_stop_lasers() {
        let (show, _) = governed("midi").await;
        show.set_edit_key(None);
        let pedal = midi_deck(&show, r#"{"note":60,"channel":1,"event":"START"},{"note":61,"event":"DROP"},{"cc":7,"context":"volume","scale":[0,1]},{"cc":64,"event":"NOPE"},{"note":62,"action":"laser.stop"},{"pc":3,"event":"UNUSED"}"#);
        // wrong channel: not this binding
        midi::handle(&show, &pedal, &[0x91, 60, 100]);
        assert_eq!(show.snapshot()["state"], "PRESHOW");
        midi::handle(&show, &pedal, &[0x90, 60, 100]);
        assert_eq!(show.snapshot()["state"], "INTRO");
        // note-off and velocity-0 note-on are releases
        midi::handle(&show, &pedal, &[0x80, 61, 0]);
        midi::handle(&show, &pedal, &[0x95, 61, 0]);
        assert_eq!(show.snapshot()["state"], "INTRO");
        midi::handle(&show, &pedal, &[0x95, 61, 90]);
        assert_eq!(show.snapshot()["state"], "DROP", "any channel when none is given");
        // a controller is a value
        midi::handle(&show, &pedal, &[0xB0, 7, 127]);
        assert_eq!(show.snapshot()["context"]["volume"], json!(1.0));
        midi::handle(&show, &pedal, &[0xB0, 7, 0]);
        assert_eq!(show.snapshot()["context"]["volume"], json!(0.0));
        // the hard stop
        show.set_armed("b1", true).unwrap();
        midi::handle(&show, &pedal, &[0x90, 62, 127]);
        assert_eq!(show.snapshot()["devices"][0]["armed"], false);
        // unbound messages are counted
        midi::handle(&show, &pedal, &[0xB0, 99, 5]);
        let j = inputs::InputInfo::json(&*pedal);
        assert_eq!(j["unbound"], 2, "the wrong-channel note and the controller nobody bound");
        assert!(show.snapshot()["inputs"][0]["listen"].as_str().unwrap().starts_with("MIDI"));
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "show.go" && g == "ALLOW" && m.starts_with("Pedal (button): MIDI note 60 ch1 sent 'START'")), "{r:?}");
        assert!(r.iter().any(|(ru, _, m)| ru == "laser.stop" && m.contains("MIDI note 62")), "{r:?}");
    }

    #[tokio::test]
    async fn midi_time_code_drives_a_locked_scene() {
        let (show, _) = governed("mtc").await;
        show.set_edit_key(None);
        show.replace(timecoded(), None, true).unwrap();
        let mut rx = show.subscribe();
        // a pedal with no timecode binding ignores MTC
        let plain = midi_deck(&show, r#"{"note":60,"event":"START"}"#);
        for b in timecode::quarter_frames(timecode::Tc::new(0, 0, 59, 22, Fps::F25).unwrap()) {
            midi::handle(&show, &plain, &[0xF1, b]);
        }
        assert_eq!(show.snapshot()["timecode"]["state"], "none");
        // a playback machine sends MTC: each full sequence is a position two frames on
        let mtc = midi_deck(&show, r#"{"timecode":"mtc"}"#);
        show.apply("studio", "START");
        let mut at = timecode::Tc::new(0, 0, 59, 22, Fps::F25).unwrap();
        for _ in 0..12 {
            for b in timecode::quarter_frames(at) {
                midi::handle(&show, &mtc, &[0xF1, b]);
            }
            at = at.plus_frames(2);
        }
        let tc = show.snapshot()["timecode"].clone();
        assert_eq!((tc["state"].clone(), tc["fps"].clone(), tc["source"].clone()), (json!("running"), json!("25"), json!("mtc")), "{tc}");
        assert_eq!(cue_ids(&mut rx), ["v1", "l1", "z1"], "the three INTRO cues fired as MTC passed 00:01:00:00, :10 and :20");
        // a full-frame message locates: it is a jump, so nothing fires
        midi::handle(&show, &mtc, &[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20, 1, 4, 0, 0xF7]);
        assert!(cue_ids(&mut rx).is_empty());
        assert!(show.events().iter().any(|e| e["type"] == "show.tc_jump"));
    }

    #[tokio::test]
    async fn osc_timecode_in_text_numbers_or_seconds_drives_the_show_too() {
        let (show, _) = governed("osctc").await;
        show.set_edit_key(None);
        show.replace(timecoded(), None, true).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        let spec = inputs::parse(r#"[{"name":"Playback","listen":"127.0.0.1:0","bindings":[{"address":"/tc","timecode":true,"fps":25}]}]"#).unwrap().remove(0);
        let input = inputs::start(&show, spec).await.unwrap();
        let to = input.bound.lock().unwrap().unwrap();
        send_osc(to, "/tc", vec![rosc::OscType::String("00:00:59:24".into())]).await;
        send_osc(to, "/tc", vec![rosc::OscType::String("00:01:00:01".into())]).await;
        assert_eq!(cue_ids(&mut rx), ["v1"], "text form");
        send_osc(to, "/tc", vec![rosc::OscType::Int(0), rosc::OscType::Int(1), rosc::OscType::Int(0), rosc::OscType::Int(11)]).await;
        assert_eq!(cue_ids(&mut rx), ["l1"], "hours, minutes, seconds, frames");
        send_osc(to, "/tc", vec![rosc::OscType::Float(60.84)]).await;
        assert_eq!(cue_ids(&mut rx), ["z1"], "a number of seconds");
        assert_eq!(show.snapshot()["timecode"]["source"], "osc:Playback");
        // a message that is not a timecode changes nothing
        send_osc(to, "/tc", vec![rosc::OscType::String("nonsense".into())]).await;
        assert_eq!(show.snapshot()["timecode"]["position_ms"].as_f64().unwrap().round(), 60_840.0);
        // the binding file must be sound
        assert!(inputs::parse(r#"[{"name":"A","listen":"127.0.0.1:1","bindings":[{"address":"/t","timecode":true,"fps":12}]}]"#).is_err());
        assert!(inputs::parse(r#"[{"name":"A","listen":"127.0.0.1:1","bindings":[{"address":"/t","timecode":true,"event":"X"}]}]"#).is_err());
        let f = inputs::parse_file(r#"{"inputs":[{"name":"Deck","listen":"127.0.0.1:1","bindings":[{"address":"/a","event":"X"}]}],"midi":[{"name":"Pedal","bindings":[{"note":60,"event":"X"}]}]}"#).unwrap();
        assert_eq!((f.inputs.len(), f.midi.len()), (1, 1));
        assert!(inputs::parse_file(r#"{"inputs":[{"name":"Deck","listen":"127.0.0.1:1","bindings":[{"address":"/a","event":"X"}]}],"midi":[{"name":"deck","bindings":[{"note":60,"event":"X"}]}]}"#).is_err(), "names are unique across OSC and MIDI");
    }

    // ---- profiles as data, at the device and the Doors check ----

    #[test]
    fn a_device_can_name_a_profile_and_the_runtime_uses_it() {
        let dir = std::env::temp_dir().join(format!("pchi-profiles-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rack.json"), r#"{"id":"rackdesk","name":"Rack Desk","dept":"lighting","kinds":["lighting.look"],"hint":"a playback","rules":[{"match":"\\d+","send":"/rack/{0}/go","basis":"vendor","source":"manual"}]}"#).unwrap();
        profiles::load_dir(&dir).unwrap();
        let file = r#"{"showchart": {"initial":"A","states":[{"id":"A"}],"transitions":[]},
            "nodes":[{"id":"d1","type":"osc","label":"Rack","params":{"host":"127.0.0.1","port":9000,"profile":"rackdesk"}},
                     {"id":"d2","type":"osc","label":"Plain","params":{"host":"127.0.0.1","port":9000}}]}"#;
        let (_, devs) = rig::load(file).unwrap();
        let devs = devs.unwrap();
        assert_eq!(devs.len(), 1, "an osc device with no department and no profile has no job, as before");
        let d = &devs[0];
        assert_eq!((d.dept.as_str(), d.profile_key()), ("lighting", "rackdesk"), "the profile gives the department");
        let packets = output::packets_for(d, &Action::new("lighting.look", json!({"look": "7"}))).unwrap();
        assert_eq!(links::flatten(&packets[0])[0].0, "/rack/7/go");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_doors_check_says_which_tools_rest_on_less_than_the_vendors_word() {
        let show = |kind: &str| {
            let out = Output::new(vec![device("d1", kind, 9)], false);
            Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap()
        };
        let weak = |show: &Show| show.preflight()["checks"].as_array().unwrap().iter().filter(|c| c["text"].as_str().is_some_and(|t| t.contains("rest on"))).map(|c| c["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let x32 = weak(&show("x32"));
        assert_eq!(x32.len(), 1);
        assert!(x32[0].contains("1 of 1") && x32[0].contains("memory") && x32[0].contains("pchi-gearcheck"), "{x32:?}");
        assert!(weak(&show("eos")).is_empty(), "Eos rests on the vendor's own article");
        assert!(weak(&show("magicq"))[0].contains("community"));
    }

    #[tokio::test]
    async fn the_loaded_profiles_can_be_listed_with_what_each_rule_rests_on() {
        let (_show, addr) = serve(None).await;
        let (st, v) = http(addr, "GET", "/profiles", "", "").await;
        assert_eq!(st, 200);
        let all = v.as_array().unwrap();
        let eos = all.iter().find(|p| p["id"] == "eos").unwrap();
        assert_eq!((eos["built_in"].clone(), eos["rules"][0]["basis"].clone(), eos["rules"][0]["send"].clone()), (json!(true), json!("vendor"), json!("/eos/cue/{0}/fire")));
        assert!(all.iter().any(|p| p["id"] == "x32" && p["probe"] == "/info"));
        assert!(all.iter().any(|p| p["id"] == "beyond" && p["stop"].as_array().unwrap().len() == 2));
    }

    // ---- capture: cues the tools themselves fire, offered to Studio ----

    #[tokio::test]
    async fn a_cue_fired_on_the_console_is_captured_where_in_the_show_it_was_heard() {
        let out = Output::new(vec![device("e1", "eos", 9), device("r1", "resolume", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let fb = |dev: &str, addr: &str| json!({"type": "feedback", "device": dev, "address": addr, "args": []});
        show.note_capture(&fb("e1", "/eos/out/event/cue/3/9/fire"));
        let c = show.captures_json();
        assert_eq!(c.as_array().unwrap().len(), 1);
        let c0 = &c[0];
        assert_eq!((c0["kind"].clone(), c0["value"].clone(), c0["scene"].clone(), c0["dept"].clone(), c0["basis"].clone()), (json!("lighting.look"), json!("3/9"), json!("INTRO"), json!("lighting"), json!("community")), "{c0}");
        assert!(c0["at_ms"].as_i64().unwrap() >= 250, "heard about 0.3 s into INTRO: {c0}");
        assert_eq!(c0["id"], 1);
        // clients are told as it happens, and a newcomer gets the list
        let msgs: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(msgs.iter().any(|m| m["type"] == "capture" && m["value"] == "3/9"));
        assert_eq!(show.snapshot()["captures"][0]["value"], "3/9");
        // the same cue repeated at once is one cue; a different one is the next
        show.note_capture(&fb("e1", "/eos/out/event/cue/3/9/fire"));
        assert_eq!(show.captures_json().as_array().unwrap().len(), 1);
        show.note_capture(&fb("e1", "/eos/out/event/cue/3/10/fire"));
        assert_eq!(show.captures_json()[1]["id"], 2);
        // messages that are not cues, from tools with no capture rules, or from nobody, are not captured
        show.note_capture(&fb("e1", "/eos/out/event/cue/-1/0/stop"));
        show.note_capture(&fb("e1", "/eos/out/active/cue/3/9"));
        show.note_capture(&fb("r1", "/eos/out/event/cue/5/5/fire"));
        show.note_capture(&fb("ghost", "/eos/out/event/cue/5/5/fire"));
        show.note_capture(&json!({"type": "delivery", "device": "e1", "address": "/eos/out/event/cue/5/5/fire"}));
        assert_eq!(show.captures_json().as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn captures_are_listed_over_http_and_the_list_is_bounded() {
        let out = Output::new(vec![device("e1", "eos", 9)], false);
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        for n in 0..40 {
            show.note_capture(&json!({"type": "feedback", "device": "e1", "address": format!("/eos/out/event/cue/1/{n}/fire"), "args": []}));
        }
        let c = show.captures_json();
        assert_eq!(c.as_array().unwrap().len(), 30, "the newest thirty are kept");
        assert_eq!(c[29]["value"], "1/39");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_with_token(Arc::clone(&show), None);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (st, v) = http(addr, "GET", "/captures", "", "").await;
        assert_eq!((st, v.as_array().unwrap().len()), (200, 30));
    }

    // ---- DMX over the network: a lighting cue becomes Art-Net or sACN ----

    fn dmx_device(kind: &str, port: u16) -> rig::Device {
        rig::Device { id: "d1".into(), label: "Rack".into(), kind: kind.into(), dept: "lighting".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: Some(if kind == "sacn" { 1 } else { 0 }), patch: None, password: None }
    }

    fn lit(look: &str) -> Showchart {
        let mut c = quick();
        let lane = c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "lighting").unwrap();
        lane.cues[0].action.payload = json!({"look": look});
        c
    }

    #[test]
    fn dmx_devices_get_standard_ports_universes_and_a_department_from_the_rig_file() {
        let file = r#"{"showchart": {"initial":"A","states":[{"id":"A"}],"transitions":[]},
            "nodes":[{"id":"a","type":"artnet","params":{"host":"10.0.0.9"}},
                     {"id":"s","type":"sacn","params":{}},
                     {"id":"s2","type":"sacn","params":{"universe":"258","host":"10.0.0.7","port":5570}}]}"#;
        let devs = rig::load(file).unwrap().1.unwrap();
        let by = |id: &str| devs.iter().find(|d| d.id == id).unwrap();
        assert_eq!((by("a").dept.as_str(), by("a").port, by("a").universe, by("a").address()), ("lighting", Some(6454), Some(0), Some("10.0.0.9:6454".into())));
        assert_eq!((by("s").port, by("s").universe, by("s").address()), (Some(5568), Some(1), Some("239.255.0.1:5568".into())), "no host: the universe's multicast address");
        assert_eq!((by("s2").universe, by("s2").address()), (Some(258), Some("10.0.0.7:5570".into())));
        assert!(by("a").is_dmx() && !devs.is_empty());
    }

    #[tokio::test]
    async fn a_lighting_cue_in_a_scene_reaches_the_universe_as_artnet() {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let out = Output::new(vec![dmx_device("artnet", port)], true);
        let show = Show::start_with(lit("1-3=100%"), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        let mut buf = vec![0u8; 2048];
        let mut got = None;
        for _ in 0..40 {
            let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await else { continue };
            if n == 18 + 512 && buf[18] == 255 {
                got = Some(buf[..n].to_vec());
                break;
            }
        }
        let p = got.expect("the lighting cue arrived as an Art-Net frame with channels 1-3 at full");
        assert_eq!((&p[0..8], p[14], p[15], p[18], p[19], p[20], p[21]), (&b"Art-Net\0"[..], 0, 0, 255, 255, 255, 0));
        let sent: Vec<Value> = deliveries(&mut rx).into_iter().filter(|d| d["kind"] == "lighting.look").collect();
        assert!(sent.iter().any(|d| d["status"] == "sent"), "{sent:?}");
    }

    #[tokio::test]
    async fn the_doors_check_and_try_a_cue_understand_dmx() {
        // a cue written for a console ("blue") is a mistake for a DMX output, and the Doors check says so before the show
        let out = Output::new(vec![dmx_device("sacn", 9)], false);
        let show = Show::start_with(lit("blue"), EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let fails: Vec<String> = show.preflight()["checks"].as_array().unwrap().iter().filter(|c| c["level"] == "fail").map(|c| c["text"].as_str().unwrap().to_string()).collect();
        assert!(fails.iter().any(|t| t.contains("\"blue\"") && t.contains("use channels like")), "{fails:?}");
        let good = Output::new(vec![dmx_device("sacn", 9)], false);
        let show = Show::start_with(lit("1-3=100% over 2"), EventStore::open_in_memory().unwrap(), Some(good)).unwrap();
        assert!(!show.preflight()["checks"].as_array().unwrap().iter().any(|c| c["level"] == "fail" && c["text"].as_str().unwrap().contains("Rack")));
        // try a cue shows what would go out
        let m = show.try_cue("d1", "5=50% over 1").await.unwrap();
        assert_eq!(m["status"], "dry-run");
        assert_eq!(m["sent_as"][0]["address"], "sACN universe 1");
        assert_eq!(m["sent_as"][0]["args"][0], "5=50% over 1");
        assert!(show.try_cue("d1", "nonsense").await.is_err());
    }

    // ---- fixtures and the patch, from the rig file to the wire ----

    #[tokio::test]
    async fn a_patched_fixture_cue_reaches_the_universe_as_channels() {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let file = format!(r#"{{"showchart": {}, "nodes": [{{"id":"d1","type":"artnet","label":"Rack","params":{{"host":"127.0.0.1","port":{port},"universe":2,"patch":"wash=rgbw@1, spot=rgb-dimmer@10"}}}}]}}"#, serde_json::to_string(&lit("wash=blue spot=50%")).unwrap());
        let (chart, devs) = rig::load(&file).unwrap();
        assert_eq!(devs.as_ref().unwrap()[0].patch.as_deref(), Some("wash=rgbw@1, spot=rgb-dimmer@10"));
        let show = Show::start_with(chart, EventStore::open_in_memory().unwrap(), Some(Output::new(devs.unwrap(), true))).unwrap();
        show.apply("studio", "START");
        let mut buf = vec![0u8; 2048];
        let mut got = None;
        for _ in 0..40 {
            let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(300), sock.recv_from(&mut buf)).await else { continue };
            if n == 530 && buf[20] == 255 {
                got = Some(buf[18..530].to_vec());
                break;
            }
        }
        let d = got.expect("the cue arrived");
        assert_eq!((d[0], d[1], d[2], d[3]), (0, 0, 255, 0), "wash (rgbw at 1) is blue: red 0, green 0, blue 255, white 0");
        assert_eq!(d[9], 128, "spot (rgb-dimmer at 10): its dimmer is channel 10, at 50%");
    }

    #[tokio::test]
    async fn the_doors_check_reads_the_patch_and_the_fixture_cues() {
        let dev = |patch: &str| {
            let mut d = dmx_device("sacn", 9);
            d.patch = Some(patch.to_string());
            d
        };
        let check = |patch: &str, look: &str| {
            let show = Show::start_with(lit(look), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![dev(patch)], false))).unwrap();
            show.preflight()["checks"].as_array().unwrap().iter().filter(|c| c["level"] != "ok" || c["text"].as_str().unwrap().contains("patch places")).map(|c| (c["level"].as_str().unwrap().to_string(), c["text"].as_str().unwrap().to_string())).collect::<Vec<_>>()
        };
        let good = check("wash=rgb@1, par=dimmer@10", "wash=red");
        assert!(good.iter().any(|(l, t)| l == "ok" && t.contains("patch places 2 fixture(s): wash, par")), "{good:?}");
        assert!(!good.iter().any(|(l, t)| l == "fail" && (t.contains("Rack") || t.contains("Lighting"))), "{good:?}");
        // an overlap, a cue naming a fixture that is not patched, and a color a dimmer cannot show
        assert!(check("a=rgb@1, b=rgb@2", "1=1").iter().any(|(l, t)| l == "fail" && t.contains("the patch is wrong") && t.contains("overlaps")));
        assert!(check("wash=rgb@1", "ghost=red").iter().any(|(l, t)| l == "fail" && t.contains("'ghost' is not a patched fixture")));
        assert!(check("lamp=dimmer@1", "lamp=blue").iter().any(|(l, t)| l == "fail" && t.contains("cannot show a color")));
    }

    #[tokio::test]
    async fn fixtures_are_listed_over_http() {
        let (_show, addr) = serve(None).await;
        let (st, v) = http(addr, "GET", "/fixtures", "", "").await;
        assert_eq!(st, 200);
        let rgbw = v.as_array().unwrap().iter().find(|f| f["id"] == "rgbw").unwrap();
        assert_eq!((rgbw["built_in"].clone(), rgbw["modes"][0]["channels"].clone()), (json!(true), json!(4)));
    }

    #[tokio::test]
    async fn a_rule_someone_has_proven_on_the_real_tool_stops_the_doors_check_warning_about_it() {
        let dir = std::env::temp_dir().join(format!("pchi-proven-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |id: &str, proven: &str| format!(r#"{{"id":"{id}","name":"Tool {id}","dept":"lighting","kinds":["lighting.look"],"rules":[{{"match":"\\d+","send":"/go/{{0}}","basis":"memory","source":"from memory"{proven}}}]}}"#);
        std::fs::write(dir.join("a.json"), mk("unproven-tool", "")).unwrap();
        std::fs::write(dir.join("b.json"), mk("proven-tool", r#","proven":"2026-11-20, v3.2 offline, Alex""#)).unwrap();
        profiles::load_dir(&dir).unwrap();
        let warns = |id: &str| {
            let mut d = dmx_device("artnet", 9);
            d.kind = "osc".into();
            d.profile = Some(id.into());
            let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![d], false))).unwrap();
            show.preflight()["checks"].as_array().unwrap().iter().filter(|c| c["text"].as_str().is_some_and(|t| t.contains("rest on"))).count()
        };
        assert_eq!(warns("unproven-tool"), 1);
        assert_eq!(warns("proven-tool"), 0, "its rule says someone saw the real tool do the right thing");
        assert_eq!(profiles::get("proven-tool").unwrap().rules[0].proven.as_deref(), Some("2026-11-20, v3.2 offline, Alex"));
        assert_eq!(profiles::summary().as_array().unwrap().iter().find(|p| p["id"] == "proven-tool").unwrap()["rules"][0]["proven"], "2026-11-20, v3.2 offline, Alex");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- LTC: timecode from audio ----

    fn ltc_file(name: &str, first: timecode::Tc, frames: usize) -> std::path::PathBuf {
        let audio = ltc::synth(first, frames, 48_000, 0.5, false, 1.0);
        let bytes: Vec<u8> = audio.iter().flat_map(|s| ((s * 32767.0) as i16).to_le_bytes()).collect();
        let path = std::env::temp_dir().join(format!("pchi-ltc-{name}-{}.raw", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[tokio::test]
    async fn ltc_read_from_an_audio_file_drives_a_locked_scene() {
        let path = ltc_file("scene", timecode::Tc::new(0, 0, 59, 20, Fps::F25).unwrap(), 70);
        let show = Show::start(timecoded(), EventStore::open_in_memory().unwrap()).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        let spec: ltc::LtcSpec = serde_json::from_str(&format!(r#"{{"name":"Playback","file":{},"fps":25,"pace":false}}"#, serde_json::to_string(path.to_str().unwrap()).unwrap())).unwrap();
        let input = ltc::start(&show, spec).unwrap();
        let mut ids = Vec::new();
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ids.extend(cue_ids(&mut rx));
            if ids.len() >= 3 {
                break;
            }
        }
        assert_eq!(ids, ["v1", "l1", "z1"], "the three INTRO cues fired as the LTC passed 00:01:00:00, :10 and :20");
        let tc = show.snapshot()["timecode"].clone();
        assert_eq!(tc["source"], "ltc", "{tc}");
        assert_eq!(tc["fps"], "25");
        let j = inputs::InputInfo::json(&*input);
        assert!(j["received"].as_u64().unwrap() > 20, "{j}");
        assert!(j["listen"].as_str().unwrap().starts_with("LTC file"), "{j}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_ltc_input_must_be_sound() {
        let ok = |t: &str| ltc::check(&serde_json::from_str::<Vec<ltc::LtcSpec>>(t).unwrap());
        assert!(ok(r#"[{"name":"A","stdin":true}]"#).is_ok());
        assert!(ok(r#"[{"name":"A","file":"x.raw","channels":2,"channel":1,"format":"f32le","fps":29.97,"drop":true}]"#).is_ok());
        assert!(ok(r#"[{"name":"A"}]"#).unwrap_err().contains("exactly one of stdin, file or device"));
        assert!(ok(r#"[{"name":"A","stdin":true,"file":"x"}]"#).unwrap_err().contains("exactly one"));
        assert!(ok(r#"[{"name":"A","stdin":true,"sample_rate":100}]"#).unwrap_err().contains("sample_rate"));
        assert!(ok(r#"[{"name":"A","stdin":true,"format":"mp3"}]"#).unwrap_err().contains("format"));
        assert!(ok(r#"[{"name":"A","stdin":true,"channels":1,"channel":1}]"#).unwrap_err().contains("channel 1 does not exist"));
        assert!(ok(r#"[{"name":"A","stdin":true,"fps":12}]"#).unwrap_err().contains("fps 12"));
        assert!(ok(r#"[{"name":"","stdin":true}]"#).unwrap_err().contains("short name"));
        // a device in a build without audio support says how to get it, rather than failing silently
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        let spec: ltc::LtcSpec = serde_json::from_str(r#"{"name":"A","device":"Scarlett"}"#).unwrap();
        if !cfg!(feature = "audio") {
            let e = ltc::start(&show, spec).err().unwrap();
            assert!(e.contains("--features audio") && e.contains("stdin or file"), "{e}");
        }
        let f = inputs::parse_file(r#"{"ltc":[{"name":"Playback","stdin":true}],"inputs":[{"name":"Deck","listen":"127.0.0.1:1","bindings":[{"address":"/a","event":"X"}]}]}"#).unwrap();
        assert_eq!((f.ltc.len(), f.inputs.len()), (1, 1));
        assert!(inputs::parse_file(r#"{"ltc":[{"name":"deck","stdin":true}],"inputs":[{"name":"Deck","listen":"127.0.0.1:1","bindings":[{"address":"/a","event":"X"}]}]}"#).is_err(), "names are unique across inputs");
    }

    // ---- OBS as a device: a cue cuts the picture ----

    fn obs_device(port: u16) -> rig::Device {
        rig::Device { id: "obs1".into(), label: "OBS".into(), kind: "obs".into(), dept: "visual".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None, patch: None, password: None }
    }

    #[tokio::test]
    async fn a_scene_cue_to_obs_switches_the_program_scene_and_the_link_is_watched() {
        let (port, scenes) = obs::fake::fake_obs(None).await;
        let mut c = quick();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "visual").unwrap().cues[0].action.payload = json!({"scene": "Main Camera"});
        let out = Output::new(vec![obs_device(port)], true);
        let show = Show::start_with(c, EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let mut rx = show.subscribe();
        show.apply("studio", "START");
        let mut got = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if scenes.lock().unwrap().contains(&"Main Camera".to_string()) {
                got = true;
                break;
            }
        }
        assert!(got, "OBS was told to make Main Camera the program");
        let sent: Vec<Value> = deliveries(&mut rx).into_iter().filter(|d| d["kind"] == "scene.activate").collect();
        assert!(sent.iter().any(|d| d["status"] == "sent"), "{sent:?}");
        // the connection is the link: OBS shows as linked in the show's own state
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let dev = &show.snapshot()["devices"][0];
        assert_eq!(dev["link"]["state"], "linked", "{dev}");
        // the password is never in what clients are told
        assert!(!show.snapshot().to_string().contains("hunter2"));
    }

    #[tokio::test]
    async fn obs_cues_are_checked_before_the_show_and_try_a_cue_describes_them() {
        let out = Output::new(vec![obs_device(9)], false);
        let mut c = quick();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "visual").unwrap().cues[0].action.payload = json!({"scene": "x".repeat(150)});
        let show = Show::start_with(c, EventStore::open_in_memory().unwrap(), Some(out)).unwrap();
        let fails: Vec<String> = show.preflight()["checks"].as_array().unwrap().iter().filter(|c| c["level"] == "fail").map(|c| c["text"].as_str().unwrap().to_string()).collect();
        assert!(fails.iter().any(|t| t.contains("OBS") && t.contains("scene name is too long")), "{fails:?}");
        let m = show.try_cue("obs1", "Slides").await.unwrap();
        assert_eq!(m["status"], "dry-run");
        assert_eq!(m["sent_as"][0]["args"][0], "Slides");
        // a password in the rig file is read but never serialised
        let file = r#"{"showchart": {"initial":"A","states":[{"id":"A"}],"transitions":[]},"nodes":[{"id":"o","type":"obs","params":{"host":"127.0.0.1","port":4455,"password":"hunter2"}}]}"#;
        let devs = rig::load(file).unwrap().1.unwrap();
        assert_eq!((devs[0].dept.as_str(), devs[0].password.as_deref()), ("visual", Some("hunter2")));
        assert!(!serde_json::to_string(&devs[0]).unwrap().contains("hunter2"));
    }

    // ---- audience votes: the room decides ----

    const AUD: &str = "X-Pchi-Audience: aud-token\r\n";

    async fn vote(addr: std::net::SocketAddr, poll: &str, option: &str, voter: &str) -> (u16, Value) {
        http(addr, "POST", "/vote", &json!({"poll": poll, "option": option, "voter": voter}).to_string(), AUD).await
    }

    #[tokio::test]
    async fn the_audience_votes_and_the_show_waits_for_the_room() {
        let (show, addr) = governed("votes").await;
        // voting is off until there is a token, and a wrong token is refused
        assert_eq!(vote(addr, "next", "hype", "ann").await.0, 403);
        show.set_audience_token(Some("aud-token".into()));
        assert_eq!(http(addr, "POST", "/vote", r#"{"poll":"next","option":"hype","voter":"ann"}"#, "X-Pchi-Audience: nope\r\n").await.0, 403);
        assert_eq!(vote(addr, "next", "hype", "ann").await.0, 404, "no such poll yet");
        // an agent cannot open a poll; a person can (the audience token opening nothing else is the next test)
        let open = r#"{"options":["Calm","Hype"]}"#;
        assert_eq!(http(addr, "POST", "/polls/next/open", open, &format!("{AGENT}{KEY}")).await.0, 403, "an agent may not run the show");
        assert_eq!(http(addr, "POST", "/polls/next/open", open, KEY).await.0, 200);
        // a transition that waits for the room
        let mut c = quick();
        c.transitions.iter_mut().find(|t| t.source == "INTRO" && t.target == "DROP").unwrap().guard = Some("vote.next.winner == \"hype\"".into());
        show.replace(c, None, true).unwrap();
        show.apply("studio", "START");
        assert!(!show.apply("studio", "DROP"), "nobody has voted: the guard is not met");
        // the votes come in
        assert_eq!(vote(addr, "next", "hype", "ann").await.0, 200);
        assert_eq!(vote(addr, "next", "CALM", "bob").await.0, 200);
        assert_eq!(vote(addr, "next", "hype", "cy").await.0, 200);
        assert_eq!(vote(addr, "next", "weird", "dee").await.0, 400, "not an option");
        assert_eq!(http(addr, "POST", "/vote", r#"{"poll":"next","option":"hype"}"#, AUD).await.0, 400, "a vote needs a voter id");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let ctx = show.snapshot()["context"].clone();
        let v = &ctx["vote"]["next"];
        assert_eq!((v["hype"].clone(), v["calm"].clone(), v["total"].clone(), v["winner"].clone(), v["open"].clone()), (json!(2), json!(1), json!(3), json!("hype"), json!(true)), "{ctx}");
        assert!(show.apply("studio", "DROP"), "the room chose hype, so the guard is met");
        // closing it keeps the result and stops the votes
        assert_eq!(http(addr, "POST", "/polls/next/close", "", KEY).await.0, 200);
        assert_eq!(vote(addr, "next", "calm", "eve").await.0, 409);
        assert_eq!(show.snapshot()["context"]["vote"]["next"]["open"], json!(false));
        assert_eq!(show.snapshot()["polls"][0]["winner"], "Hype");
        let r = receipts(&show);
        assert!(r.iter().any(|(ru, g, m)| ru == "poll.open" && g == "ALLOW" && m.contains("opened poll 'next' with Calm, Hype")), "{r:?}");
        assert!(r.iter().any(|(ru, _, m)| ru == "poll.close" && m.contains("Hype 2") && m.contains("winner Hype")), "{r:?}");
        assert!(r.iter().any(|(ru, g, _)| ru == "poll.open" && g == "DENY"), "the refused openings were written down too");
        assert!(show.events().iter().any(|e| e["type"] == "show.poll" && e["payload"]["winner"] == "Hype"));
    }

    #[tokio::test]
    async fn the_audience_token_opens_voting_and_nothing_else() {
        let (show, addr) = serve(Some("feed-token")).await;
        show.set_audience_token(Some("aud-token".into()));
        // as a feed token it gets nothing
        assert_eq!(http(addr, "GET", "/state?token=aud-token", "", "").await.0, 401);
        assert_eq!(http(addr, "GET", "/polls?token=aud-token", "", "").await.0, 401);
        assert_eq!(http(addr, "POST", "/input?token=aud-token", r#"{"event":"START"}"#, "").await.0, 401);
        assert_eq!(http(addr, "POST", "/polls/x/open?token=aud-token", r#"{"options":["a","b"]}"#, "").await.0, 401);
        // and the feed token does not let anyone vote
        assert_eq!(http(addr, "POST", "/vote?token=feed-token", r#"{"poll":"x","option":"a","voter":"v"}"#, "").await.0, 403);
        show.poll_open(&Actor::Person(None), "x", &["a".into(), "b".into()]).unwrap();
        assert_eq!(http(addr, "POST", "/vote?token=aud-token", r#"{"poll":"x","option":"a","voter":"v"}"#, "").await.0, 200);
        assert_eq!(http(addr, "POST", "/vote", r#"{"poll":"x","option":"b","voter":"w"}"#, "Authorization: Bearer aud-token\r\n").await.0, 200, "a bearer header works too");
    }

    #[tokio::test]
    async fn a_flood_of_votes_is_capped() {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        show.set_audience_token(Some("aud-token".into()));
        show.poll_open(&Actor::Person(None), "p", &["a".into(), "b".into()]).unwrap();
        let mut refused = 0;
        for i in 0..400 {
            if let Err((code, _)) = show.cast_vote("p", "a", &format!("v{i}")) {
                assert_eq!(code, 429);
                refused += 1;
            }
        }
        assert!(refused >= 90, "more than 300 a second are refused: {refused}");
        // the context was not written 300 times: it is throttled
        let writes = show.events().iter().filter(|e| e["type"] == "show.context").count();
        assert!(writes < 60, "{writes} context writes for 300 votes");
    }

    // ---- safer secrets: hashed keys, end dates, sessions ----

    fn hashed_roster(iters: u32) -> (String, Vec<(String, String)>) {
        let people = [("Alex", "operator"), ("Eddie", "editor"), ("Gus", "guest")];
        let mut keys = Vec::new();
        let mut entries = Vec::new();
        for (n, r) in people {
            let (key, entry) = governance::new_entry(n, r, iters).unwrap();
            keys.push((n.to_string(), key));
            entries.push(entry);
        }
        (json!({"people": entries}).to_string(), keys)
    }

    #[test]
    fn pbkdf2_is_the_standard_one_and_a_made_entry_holds_no_key() {
        // a published PBKDF2-HMAC-SHA256 test vector: password "password", salt "salt", 1 iteration
        assert_eq!(hex::encode(governance::pbkdf2_key("password", b"salt", 1)), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        let (k1, e1) = governance::new_entry("Alex", "operator", 1000).unwrap();
        let (k2, _) = governance::new_entry("Alex", "operator", 1000).unwrap();
        assert_ne!(k1, k2, "every key is fresh");
        assert_eq!(k1.len(), 40);
        assert!(!e1.to_string().contains(&k1), "the entry must not contain the key it was made for");
        assert!(e1["key_hash"].as_str().unwrap().len() == 64 && e1["salt"].as_str().unwrap().len() == 32);
        let roster = governance::Roster::parse(&json!([e1]).to_string()).unwrap();
        assert!(matches!(roster.find(&k1), Some(Actor::Member { .. })));
        assert!(roster.find(&k2).is_none() && roster.find("not the key").is_none());
        assert_eq!(roster.plaintext_count(), 0);
        assert!(governance::new_entry("", "operator", 1000).is_err() && governance::new_entry("A", "boss", 1000).is_err());
    }

    #[test]
    fn a_roster_entry_is_one_kind_of_credential_and_may_have_an_end_date() {
        let (_, e) = governance::new_entry("Alex", "operator", 1000).unwrap();
        let err = |v: Value| governance::Roster::parse(&json!([v]).to_string()).err().unwrap_or_default();
        let mut both = e.clone();
        both["key"] = json!("12345678");
        assert!(err(both).contains("either"), "key and key_hash together");
        let mut neither = e.clone();
        neither.as_object_mut().unwrap().remove("key_hash");
        assert!(err(neither).contains("either"));
        let mut weak = e.clone();
        weak["iterations"] = json!(5);
        assert!(err(weak).contains("iterations from 1000"));
        let mut badsalt = e.clone();
        badsalt["salt"] = json!("zz");
        assert!(err(badsalt).contains("salt must be hex"));
        let mut when = e.clone();
        when["expires"] = json!("next tuesday");
        assert!(err(when).contains("write 2026-12-31"));
        assert_eq!(governance::governance_plain_count_for(r#"[{"name":"A","role":"guest","key":"12345678"}]"#), 1);
        // end dates are read as UTC: a date means the end of that day, a time is the time
        assert_eq!(governance::parse_expiry("2000-01-01").unwrap(), (10_957 * 86_400 + 86_399) * 1000);
        assert_eq!(governance::parse_expiry("2000-01-01T01:30").unwrap(), (10_957 * 86_400 + 5_400) * 1000);
        assert_eq!(gearcheck::date_of_ms(governance::parse_expiry("2026-12-31").unwrap()), "2026-12-31");
        assert!(governance::parse_expiry("2026-13-01").is_err() && governance::parse_expiry("2026-01-01T25:00").is_err());
    }

    #[test]
    fn a_key_stops_working_after_its_end_date_and_says_why() {
        let (key, mut e) = governance::new_entry("Temp", "editor", 1000).unwrap();
        e["expires"] = json!("2026-12-31T18:00");
        let roster = governance::Roster::parse(&json!([e]).to_string()).unwrap();
        let end = governance::parse_expiry("2026-12-31T18:00").unwrap();
        assert!(matches!(roster.find_at(&key, end - 1), Ok(Some(_))), "before the end it works");
        assert!(matches!(roster.find_at(&key, end), Ok(Some(_))), "at the end it still works");
        let e = roster.find_at(&key, end + 1).unwrap_err();
        assert!(e.contains("Temp") && e.contains("expired"), "{e}");
        assert!(matches!(roster.find_at("other", end + 1), Ok(None)), "an unknown key is just unknown");
        assert_eq!(roster.expiring().len(), 1);
    }

    #[tokio::test]
    async fn signing_in_trades_a_key_for_a_session_and_the_key_is_never_kept() {
        let (roster, keys) = hashed_roster(1000);
        let key_of = |n: &str| keys.iter().find(|k| k.0 == n).unwrap().1.clone();
        let (show, addr) = governed("session").await;
        show.set_edit_key(None);
        show.set_roster(governance::Roster::parse(&roster).unwrap());
        // a wrong key is refused and written down
        let (st, v) = http(addr, "POST", "/signin", r#"{"key":"wrong-key"}"#, "").await;
        assert_eq!((st, v["error"].clone()), (403, json!("that key is not known")));
        // the right key gives a token
        let (st, v) = http(addr, "POST", "/signin", &json!({"key": key_of("Alex")}).to_string(), "").await;
        assert_eq!((st, v["name"].clone(), v["role"].clone(), v["expires_in_s"].clone()), (200, json!("Alex (operator)"), json!("operator"), json!(8 * 3600)), "{v}");
        let token = v["token"].as_str().unwrap().to_string();
        assert_eq!(token.len(), 48);
        // from then on the token is who they are: no key needed
        let hdr = format!("X-Pchi-Session: {token}\r\n");
        let (_, w) = http(addr, "GET", "/whoami", "", &hdr).await;
        assert_eq!((w["name"].clone(), w["signed_in"].clone()), (json!("Alex (operator)"), json!(true)));
        assert_eq!(http(addr, "GET", &format!("/whoami?session={token}"), "", "").await.1["name"], "Alex (operator)", "a query works too, for a WebSocket");
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"START"}"#, &hdr).await.0, 200);
        assert_eq!(http(addr, "GET", "/whoami", "", "X-Pchi-Session: made-up\r\n").await.1["signed_in"], false, "a token nobody gave out is nobody");
        // a guest's session may look and nothing more
        let g = http(addr, "POST", "/signin", &json!({"key": key_of("Gus")}).to_string(), "").await.1;
        let ghdr = format!("X-Pchi-Session: {}\r\n", g["token"].as_str().unwrap());
        assert_eq!(http(addr, "POST", "/input", r#"{"event":"DROP"}"#, &ghdr).await.0, 403);
        // signing out ends it at once
        assert_eq!(http(addr, "POST", "/signout", "", &hdr).await.1["signed_out"], true);
        assert_eq!(http(addr, "GET", "/whoami", "", &hdr).await.1["signed_in"], false);
        // an agent does not sign in
        let (agent_key, agent_entry) = governance::new_entry("Nova", "agent", 1000).unwrap();
        show.set_roster(governance::Roster::parse(&json!([agent_entry]).to_string()).unwrap());
        assert_eq!(http(addr, "POST", "/signin", &json!({"key": agent_key}).to_string(), "").await.0, 403);
        // none of the keys is anywhere in the record
        let everything = format!("{} {}", show.governance_export(), serde_json::to_string(&show.events()).unwrap());
        for (_, k) in &keys {
            assert!(!everything.contains(k.as_str()), "a key was written down");
        }
        assert!(everything.contains("auth.signin") && everything.contains("sign-in refused: that key is not known"));
    }

    #[tokio::test]
    async fn a_session_ends_when_its_time_is_up_and_wrong_keys_are_throttled() {
        let (roster, keys) = hashed_roster(1000);
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        show.set_roster(governance::Roster::parse(&roster).unwrap());
        let token = show.sign_in(&keys[0].1).unwrap()["token"].as_str().unwrap().to_string();
        assert!(matches!(show.identify_with(Some(&token), None, Actor::Person(None)), Actor::Member { .. }));
        // its time up: it is gone, and the sweep removes it
        show.sessions.lock().unwrap().get_mut(&session_id(&token)).unwrap().1 = now_ms() - 1;
        assert_eq!(show.identify_with(Some(&token), None, Actor::Person(None)), Actor::Anonymous);
        assert!(show.sessions.lock().unwrap().is_empty());
        // a declared agent stays an agent whatever session it holds
        let live = show.sign_in(&keys[0].1).unwrap()["token"].as_str().unwrap().to_string();
        assert!(show.identify_with(Some(&live), None, Actor::Agent("x".into())).is_agent());
        // ten wrong keys in a minute and sign-in waits, even for the right key
        for _ in 0..10 {
            assert_eq!(show.sign_in("nope").unwrap_err().0, 403);
        }
        let (code, why) = show.sign_in(&keys[0].1).unwrap_err();
        assert_eq!(code, 429, "{why}");
        // a key with a past end date is refused with the reason
        let (k, mut e) = governance::new_entry("Old", "operator", 1000).unwrap();
        e["expires"] = json!("2001-01-01");
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        show.set_roster(governance::Roster::parse(&json!([e]).to_string()).unwrap());
        assert!(show.sign_in(&k).unwrap_err().1.contains("has expired"));
        assert_eq!(show.identify(Some(&k), Actor::Person(None)), Actor::Anonymous, "an expired key does not work as a header either");
    }

    // ---- the MCP server: an agent reads, rehearses and proposes ----

    async fn rpc(c: &mcp::Client, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        serde_json::from_str(&c.handle(&line).await.expect("a request is answered")).unwrap()
    }

    fn tool_text(r: &Value) -> (bool, String) {
        (r["result"]["isError"].as_bool().unwrap_or(false), r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
    }

    #[tokio::test]
    async fn the_mcp_server_speaks_the_protocol_and_offers_only_what_an_agent_may_do() {
        let c = mcp::Client { addr: "127.0.0.1:1".into(), name: "nova".into(), token: None, key: None };
        let init = rpc(&c, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}})).await;
        assert_eq!((init["result"]["protocolVersion"].clone(), init["result"]["serverInfo"]["name"].clone()), (json!("2024-11-05"), json!("pchi")));
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert_eq!(rpc(&c, "initialize", json!({"protocolVersion": "1999-01-01"})).await["result"]["protocolVersion"], "2025-06-18", "an unknown version is answered with one we speak");
        assert!(c.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string()).await.is_none(), "a notification gets no answer");
        assert_eq!(rpc(&c, "ping", json!({})).await["result"], json!({}));
        assert_eq!(rpc(&c, "resources/list", json!({})).await["error"]["code"], -32601);
        assert_eq!(serde_json::from_str::<Value>(&c.handle("not json").await.unwrap()).unwrap()["error"]["code"], -32700);
        let tools = rpc(&c, "tools/list", json!({})).await["result"]["tools"].clone();
        let names: Vec<&str> = tools.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for want in ["pchi_state", "pchi_show", "pchi_doors_check", "pchi_rehearse", "pchi_propose", "pchi_propose_cue_change", "pchi_proposals", "pchi_report", "pchi_stop_lasers"] {
            assert!(names.contains(&want), "{want} is offered");
        }
        for t in tools.as_array().unwrap() {
            assert!(t["description"].as_str().unwrap().len() > 30 && t["inputSchema"]["type"] == "object", "{t}");
            let n = t["name"].as_str().unwrap();
            for never in ["go", "arm", "approve", "reject", "undo", "try", "test", "input", "poll", "reset"] {
                assert!(!n.contains(never) || n == "pchi_proposals" || n == "pchi_report" || n.contains("propose"), "{n} should not exist: an agent cannot {never}");
            }
        }
        // asking for what is not offered is a clear failure, not a surprise
        let (bad, text) = tool_text(&rpc(&c, "tools/call", json!({"name": "pchi_go", "arguments": {}})).await);
        assert!(bad && text.contains("cannot press GO, arm, approve or edit directly"), "{text}");
        // and a runtime that is not there is said plainly
        let (bad, text) = tool_text(&rpc(&c, "tools/call", json!({"name": "pchi_state", "arguments": {}})).await);
        assert!(bad && text.contains("cannot reach the PCHI runtime"), "{text}");
    }

    #[tokio::test]
    async fn an_agent_looks_rehearses_and_proposes_and_a_person_decides() {
        let (show, addr) = governed("mcp").await;
        let c = mcp::Client { addr: addr.to_string(), name: "nova".into(), token: None, key: None };
        let call = |name: &'static str, args: Value| {
            let c = &c;
            async move { tool_text(&rpc(c, "tools/call", json!({"name": name, "arguments": args})).await) }
        };
        let (bad, text) = call("pchi_state", json!({})).await;
        assert!(!bad && text.contains("\"scene\": \"PRESHOW\""), "{text}");
        let (_, def) = call("pchi_show", json!({})).await;
        assert!(serde_json::from_str::<Value>(&def).unwrap()["states"].is_array());
        let (bad, doors) = call("pchi_doors_check", json!({})).await;
        assert!(!bad && doors.contains("\"summary\""), "{doors}");
        show.apply("studio", "START");
        tokio::time::sleep(Duration::from_millis(700)).await;

        // a dry run reports what would change and how it would have played, and proposes nothing
        let change = json!({"scene": "INTRO", "lane": "lighting", "cue": "l1", "at_ms": 60_000, "said": "bring the lights in much later"});
        let mut dry = change.clone();
        dry["dry_run"] = json!(true);
        let (bad, text) = call("pchi_propose_cue_change", dry).await;
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(!bad && v["dry_run"] == true && v["changes"][0].as_str().unwrap().contains("moved"), "{text}");
        assert!(v["rehearsal"]["differences"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("would NOT fire")), "{text}");
        assert_eq!(show.snapshot()["proposals"].as_array().unwrap().len(), 0, "a dry run proposes nothing");

        // without a reason it is refused; with one it is only a proposal
        let mut nowhy = change.clone();
        nowhy["said"] = json!("");
        assert!(call("pchi_propose_cue_change", nowhy).await.0);
        assert!(call("pchi_propose_cue_change", json!({"scene": "INTRO", "lane": "lighting", "cue": "nope", "at_ms": 1, "said": "x"})).await.1.contains("there is no cue 'nope'"));
        assert!(call("pchi_propose_cue_change", json!({"scene": "INTRO", "lane": "lighting", "cue": "l1", "said": "x"})).await.1.contains("change something"));
        let before = show.snapshot()["definition"].clone();
        let (bad, text) = call("pchi_propose_cue_change", change).await;
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(!bad && v["proposed"] == true && v["id"] == 1 && v["next"].as_str().unwrap().contains("changes nothing until then"), "{text}");
        assert_eq!(show.snapshot()["definition"], before, "the agent changed nothing");
        let (_, list) = call("pchi_proposals", json!({})).await;
        assert!(list.contains("agent:nova") && list.contains("bring the lights in much later") && list.contains("pending"), "{list}");

        // the whole-show tools work too, and a bad show is the runtime's refusal in words
        let mut edited: Value = serde_json::from_str(&def).unwrap();
        edited["states"][2]["label"] = json!("Renamed for the test");
        let (bad, text) = call("pchi_rehearse", json!({"show": edited.clone()})).await;
        assert!(!bad && text.contains("verdict"), "{text}");
        assert!(call("pchi_propose", json!({"show": {"nonsense": true}, "said": "x"})).await.0);
        assert!(call("pchi_propose", json!({"show": edited})).await.1.contains("say why"));

        // a person approves it: only then does the show change
        assert_eq!(http(addr, "POST", "/proposals/1/approve", "", KEY).await.0, 200);
        assert_ne!(show.snapshot()["definition"], before);
        let (_, report) = call("pchi_report", json!({})).await;
        assert!(report.contains("agent:nova") && report.contains("proposed edit #1") && report.contains("said"), "{report}");
        // the hard stop is open to it, because it only makes things safer
        let (bad, text) = call("pchi_stop_lasers", json!({})).await;
        assert!(!bad && text.contains("stopped"), "{text}");
        assert!(receipts(&show).iter().any(|(ru, g, m)| ru == "laser.stop" && g == "ALLOW" && m.contains("agent:nova")));
    }

    // ---- failover ----

    async fn serve_app(app: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        (addr, tokio::spawn(async move { axum::serve(l, app).await.unwrap(); }))
    }

    async fn fo_get(addr: std::net::SocketAddr, path: &str, key: Option<&str>) -> (u16, Value) {
        let h: Vec<(&str, &str)> = key.map(|k| vec![("X-Pchi-Failover-Key", k)]).unwrap_or_default();
        failover::http(&addr.to_string(), "GET", path, &h, None).await.unwrap()
    }

    async fn fo_until(secs: u64, mut ok: impl FnMut() -> bool) -> bool {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        while tokio::time::Instant::now() < end {
            if ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        ok()
    }

    fn standby_of(addr: std::net::SocketAddr, auto: bool) -> Arc<failover::Standby> {
        let mut cfg = failover::Config::new(&addr.to_string(), "nova-b", "fk");
        cfg.poll_every = Duration::from_millis(40);
        cfg.takeover_after = Duration::from_millis(400);
        cfg.auto = auto;
        failover::Standby::new(cfg, EventStore::open_in_memory().unwrap(), failover::activation(Some(vec![device("b1", "beyond", 9)]), false, None, Box::new(|_| Ok(()))))
    }

    fn primary_show() -> Arc<Show> {
        let p = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("b1", "beyond", 9)], false))).unwrap();
        p.set_failover_key(Some("fk".into()));
        p
    }

    #[tokio::test]
    async fn a_standby_copies_the_log_and_takes_over_when_the_primary_goes_quiet() {
        let primary = primary_show();
        primary.apply("studio", "START"); // INTRO
        primary.set_context("energy", json!(0.9));
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "laser", 1500);
        primary.replace(edited, None, true).unwrap();
        let (paddr, ptask) = serve_app(router(primary.clone())).await;

        let sb = standby_of(paddr, true);
        tokio::spawn(sb.clone().follow());
        let (saddr, _) = serve_app(failover::standby_router(sb.clone())).await;

        // it copies everything, and says it is a standby; every other request is refused as standby
        assert!(fo_until(3, || sb.copied() as usize >= primary.events().len()).await, "copied {} of {}", sb.copied(), primary.events().len());
        let (code, role) = fo_get(saddr, "/role", None).await;
        assert_eq!((code, role["role"].as_str(), role["ready"].as_bool()), (200, Some("standby"), Some(true)), "{role}");
        let (code, v) = fo_get(saddr, "/state", None).await;
        assert_eq!(code, 503, "{v}");
        assert!(!sb.is_active(), "a primary that answers is not replaced");

        // the primary dies: after the silence the standby runs the show from where it was
        ptask.abort();
        assert!(fo_until(5, || sb.is_active()).await, "the standby did not take over");
        let (code, state) = fo_get(saddr, "/state", None).await;
        assert_eq!(code, 200, "{state}");
        assert_eq!(state["state"], "INTRO");
        assert_eq!(state["context"]["energy"], 0.9, "the context comes with it");
        let lanes = &state["definition"]["states"].as_array().unwrap().iter().find(|s| s["id"] == "INTRO").unwrap()["timeline"]["lanes"];
        assert!(lanes.to_string().contains("1500"), "the live edit came with it: {lanes}");
        let (_, role) = fo_get(saddr, "/role", None).await;
        assert_eq!((role["role"].as_str(), role["epoch"].as_u64()), (Some("active"), Some(1)), "{role}");
        assert_eq!(state["devices"][0]["armed"], false, "lasers start disarmed after a takeover");
        assert_eq!(state["resumed"]["state"], "INTRO");
    }

    #[tokio::test]
    async fn a_standby_that_takes_over_stops_the_primary_even_if_it_comes_back() {
        let primary = primary_show();
        primary.apply("studio", "START");
        primary.set_armed("b1", true).unwrap();
        let (paddr, _p) = serve_app(router(primary.clone())).await;
        let sb = standby_of(paddr, false); // manual
        tokio::spawn(sb.clone().follow());
        let (saddr, _s) = serve_app(failover::standby_router(sb.clone())).await;
        assert!(fo_until(3, || sb.copied() as usize >= primary.events().len()).await);
        // a manual standby does not take over by itself, only on the key's word
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!sb.is_active());
        let (code, _) = failover::http(&saddr.to_string(), "POST", "/takeover", &[], None).await.unwrap();
        assert_eq!(code, 403, "taking over needs the key");
        let (code, v) = failover::http(&saddr.to_string(), "POST", "/takeover", &[("X-Pchi-Failover-Key", "fk")], None).await.unwrap();
        assert_eq!(code, 200, "{v}");
        // the primary was healthy; it is told at once and stops
        assert!(fo_until(3, || primary.is_fenced()).await, "the primary was not told");
        let (_, role) = fo_get(paddr, "/role", None).await;
        assert_eq!((role["role"].as_str(), role["epoch"].as_u64()), (Some("fenced"), Some(1)), "{role}");
        assert_eq!(role["fenced_by"]["by"], "nova-b");
        assert_eq!(primary.apply_checked("studio", "NEXT"), Applied::Ignored, "a fenced runtime ignores inputs");
        assert_eq!(primary.current_state(), "INTRO");
        let snap = primary.snapshot();
        assert_eq!(snap["devices"][0]["armed"], false, "its lasers are disarmed");
        assert_eq!(snap["role"]["role"], "fenced");
        assert!(primary.events().iter().any(|e| e["type"] == "show.fenced"));
        // an older takeover number cannot fence a runtime
        let other = primary_show();
        assert_eq!(other.fence(0, "x")["fenced"], false);
        assert!(!other.is_fenced());
    }

    #[tokio::test]
    async fn the_log_is_only_given_to_a_standby_with_the_key_and_only_for_the_log_it_follows() {
        let primary = primary_show();
        primary.apply("studio", "START");
        let (paddr, _p) = serve_app(router(primary.clone())).await;
        assert_eq!(fo_get(paddr, "/replicate?after=0", None).await.0, 403);
        assert_eq!(fo_get(paddr, "/replicate?after=0", Some("wrong")).await.0, 403);
        let (code, v) = fo_get(paddr, "/replicate?after=0", Some("fk")).await;
        assert_eq!(code, 200);
        assert!(v["records"].as_array().unwrap().len() >= 2);
        assert_eq!(v["epoch"], 0);
        let (code, _) = failover::http(&paddr.to_string(), "POST", "/fence", &[("X-Pchi-Failover-Key", "wrong")], Some("{\"epoch\":5}")).await.unwrap();
        assert_eq!(code, 403);
        assert!(!primary.is_fenced());
        // a copy that does not match this log is refused, not extended
        let (code, v) = fo_get(paddr, "/replicate?after=1&last_ts=12345", Some("fk")).await;
        assert_eq!(code, 409, "{v}");
        let (code, _) = fo_get(paddr, "/replicate?after=999999&last_ts=1", Some("fk")).await;
        assert_eq!(code, 409);
        // failover is off unless asked for
        let plain = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        let (paddr2, _q) = serve_app(router(plain)).await;
        assert_eq!(fo_get(paddr2, "/replicate?after=0", Some("fk")).await.0, 404);
    }

    #[tokio::test]
    async fn a_standby_with_the_wrong_key_does_not_take_over() {
        let primary = primary_show();
        primary.apply("studio", "START");
        let (paddr, ptask) = serve_app(router(primary.clone())).await;
        let mut cfg = failover::Config::new(&paddr.to_string(), "nova-b", "not-the-key");
        cfg.poll_every = Duration::from_millis(40);
        cfg.takeover_after = Duration::from_millis(200);
        let sb = failover::Standby::new(cfg, EventStore::open_in_memory().unwrap(), failover::activation(None, false, None, Box::new(|_| Ok(()))));
        tokio::spawn(sb.clone().follow());
        tokio::time::sleep(Duration::from_millis(300)).await;
        ptask.abort();
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(!sb.is_active(), "a refused standby has nothing to run: that is a mistake to fix, not a death to cover");
        let role = sb.role_json();
        assert_eq!(role["stuck"], true);
        assert!(role["problem"].as_str().unwrap().contains("failover key"), "{role}");
    }

    // ---- recording ----

    async fn a_run_with_everything_in_it() -> Arc<Show> {
        let show = Show::start_with(quick(), EventStore::open_in_memory().unwrap(), Some(Output::new(vec![device("b1", "beyond", 9)], false))).unwrap();
        show.apply("studio", "START"); // INTRO: visual 0, lighting 0.4 s, laser 0.8 s
        show.set_context("energy", json!(0.7));
        tokio::time::sleep(Duration::from_millis(550)).await;
        let mut edited = quick();
        move_cue(&mut edited, "INTRO", "laser", 1500);
        show.replace(edited, None, true).unwrap();
        show.receipt("show.edit", "ALLOW", &Actor::Member { name: "Alex".into(), role: "operator".into() }, "sent an edited show");
        show.apply("studio", "NOT-AN-EVENT");
        show
    }

    #[tokio::test]
    async fn a_run_is_exported_as_a_recording_that_reads_as_a_timeline_and_replays_exactly() {
        let show = a_run_with_everything_in_it().await;
        let (addr, _s) = serve_app(router(show.clone())).await;

        let (code, rec) = fo_get(addr, "/recording", None).await;
        assert_eq!(code, 200);
        assert_eq!(rec["format"], "pchi-run/1");
        assert_eq!(rec["records"][0]["type"], "run.start");
        let n = rec["records"].as_array().unwrap().len();
        assert!(n >= 6, "{n}");

        let (code, v) = fo_get(addr, "/replay", None).await;
        assert_eq!(code, 200, "{v}");
        let kinds: Vec<&str> = v["frames"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
        for k in ["scene", "context", "cue", "edit", "decision", "ignored"] {
            assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
        }
        assert_eq!(v["summary"]["path"], json!(["PRESHOW", "INTRO"]));
        assert_eq!(v["summary"]["edits"], 1);
        assert_eq!(v["summary"]["who"], json!(["Alex (operator)"]));
        assert_eq!(v["scenes"][0]["scene"], "PRESHOW");
        assert!(v["scenes"][1]["to_ms"].is_null(), "the scene still running has no end");
        assert_eq!(v["verify"]["matches"], true, "{}", v["verify"]);
        let frames = v["frames"].as_array().unwrap();
        assert!(frames.windows(2).all(|w| w[0]["t"].as_i64() <= w[1]["t"].as_i64()), "in time order");
        assert!(frames.iter().any(|f| f["kind"] == "edit" && f["text"].as_str().unwrap().contains("moved 0.8s")), "{frames:?}");

        // as a file: it comes back the same, and a stranger's file can be looked at without touching the show
        let text = serde_json::to_string(&rec).unwrap();
        let records = recording::parse(&text).unwrap();
        assert_eq!(records.len(), n);
        let (code, again) = failover::http(&addr.to_string(), "POST", "/replay", &[], Some(&text)).await.unwrap();
        assert_eq!(code, 200);
        assert_eq!(again["frames"], v["frames"]);
        assert_eq!(show.current_state(), "INTRO", "looking at a recording changes nothing");
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_recording_is_refused_and_a_tampered_one_does_not_replay() {
        let show = a_run_with_everything_in_it().await;
        let (addr, _s) = serve_app(router(show.clone())).await;
        for bad in ["{}", "[]", "nonsense", r#"{"format":"pchi-run/1","records":[{"ts":1,"type":"show.input"}]}"#, r#"{"format":"pchi-run/9","records":[]}"#] {
            assert!(recording::parse(bad).is_err(), "{bad}");
            let (code, _) = failover::http(&addr.to_string(), "POST", "/replay", &[], Some(bad)).await.unwrap();
            assert_eq!(code, 400, "{bad}");
        }
        // somebody changes where the show went
        let (_, rec) = fo_get(addr, "/recording", None).await;
        let mut tampered = rec.clone();
        let i = tampered["records"].as_array().unwrap().iter().position(|r| r["type"] == "show.input" && r["payload"]["applied"] == true).unwrap();
        tampered["records"][i]["payload"]["to"] = json!("FINALE");
        let records = recording::parse(&tampered.to_string()).unwrap();
        let v = recording::verify(&records).unwrap();
        assert_eq!(v["matches"], false, "{v}");
        assert!(v["verdict"].as_str().unwrap().contains("does NOT replay"), "{v}");
        // an untouched copy is fine
        assert_eq!(recording::verify(&recording::parse(&rec.to_string()).unwrap()).unwrap()["matches"], true);
    }

    #[tokio::test]
    async fn a_recording_holds_no_secrets() {
        let show = a_run_with_everything_in_it().await;
        show.set_edit_key(Some("the-operator-key-123".into()));
        show.set_audience_token(Some("the-audience-token-456".into()));
        let text = recording::export(&show.records()).unwrap().to_string();
        for secret in ["the-operator-key-123", "the-audience-token-456"] {
            assert!(!text.contains(secret), "{secret} leaked into the recording");
        }
    }

    // ---- sessions that survive a restart ----

    fn fresh_show_with(roster: &str, edit_key: Option<&str>) -> Arc<Show> {
        let show = Show::start(quick(), EventStore::open_in_memory().unwrap()).unwrap();
        show.set_roster(governance::Roster::parse(roster).unwrap());
        show.set_edit_key(edit_key.map(String::from));
        show
    }

    #[tokio::test]
    async fn a_signed_in_session_survives_a_restart_and_the_file_holds_no_token() {
        let path = temp_db("sessions");
        let (roster, keys) = hashed_roster(1000);
        let first = fresh_show_with(&roster, Some("operator-key-1"));
        assert_eq!(first.use_sessions_file(&path).unwrap(), 0, "no file yet");
        let alex = first.sign_in(&keys[0].1).unwrap()["token"].as_str().unwrap().to_string();
        let op = first.sign_in("operator-key-1").unwrap()["token"].as_str().unwrap().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&alex) && !text.contains(&op), "the file must not hold a token that works: {text}");
        assert!(!text.contains("operator-key-1") && !text.contains(&keys[0].1), "nor a key");

        // the runtime restarts: same roster, same file
        let second = fresh_show_with(&roster, Some("operator-key-1"));
        assert_eq!(second.use_sessions_file(&path).unwrap(), 2);
        assert_eq!(second.identify_with(Some(&alex), None, Actor::Person(None)), Actor::Member { name: "Alex".into(), role: "operator".into() });
        assert_eq!(second.identify_with(Some(&op), None, Actor::Anonymous), Actor::Person(None));
        // signing out is remembered too
        assert!(second.sign_out(&alex));
        let third = fresh_show_with(&roster, Some("operator-key-1"));
        assert_eq!(third.use_sessions_file(&path).unwrap(), 1);
        assert_eq!(third.identify_with(Some(&alex), None, Actor::Person(None)), Actor::Anonymous);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_restored_session_is_dropped_if_the_person_is_gone_changed_role_or_ran_out() {
        let path = temp_db("sessions-drop");
        let (roster, keys) = hashed_roster(1000);
        let first = fresh_show_with(&roster, None);
        first.use_sessions_file(&path).unwrap();
        let alex = first.sign_in(&keys[0].1).unwrap()["token"].as_str().unwrap().to_string();
        let gus = first.sign_in(&keys[2].1).unwrap()["token"].as_str().unwrap().to_string();
        let eddie = first.sign_in(&keys[1].1).unwrap()["token"].as_str().unwrap().to_string();
        first.sessions.lock().unwrap().get_mut(&session_id(&eddie)).unwrap().1 = now_ms() - 1;
        first.save_sessions();

        // Gus has left the roster, Alex is now only a guest; Eddie's session had run out
        let (_, a) = governance::new_entry("Alex", "guest", 1000).unwrap();
        let (_, e) = governance::new_entry("Eddie", "editor", 1000).unwrap();
        let smaller = json!({"people": [a, e]}).to_string();
        let second = fresh_show_with(&smaller, None);
        assert_eq!(second.use_sessions_file(&path).unwrap(), 0);
        for t in [&alex, &gus, &eddie] {
            assert_eq!(second.identify_with(Some(t), None, Actor::Person(None)), Actor::Anonymous);
        }
        // the operator key's session does not outlive the key
        let op = first.sign_in(&keys[0].1).unwrap();
        let _ = op;
        let third = fresh_show_with(&roster, None);
        std::fs::write(&path, json!({"sessions": [{"id": "x", "expires": now_ms() + 100000, "kind": "operator"}]}).to_string()).unwrap();
        assert_eq!(third.use_sessions_file(&path).unwrap(), 0, "no operator key now: that session goes");
        // a file that is not a sessions file is a startup error, not silently ignored
        std::fs::write(&path, "not json").unwrap();
        assert!(fresh_show_with(&roster, None).use_sessions_file(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
