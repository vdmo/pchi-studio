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
pub mod diff;
pub mod governance;
pub mod links;
pub mod output;
pub mod preflight;
pub mod profiles;
pub mod rehearse;
pub mod rig;

use futures_util::{SinkExt, StreamExt};
use governance::{Actor, Proposal};
use output::{Output, Plan};
use pchi_eventstore::{recorder, EventStore};
use pchi_showchart::{timer_event, Action, ScheduledCue, ShowEngine, Showchart};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

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
                        s.publish(m);
                    }
                }));
                let now = now_ms();
                for dev in out.devices() {
                    if output::Output::expects_feedback(dev) {
                        match out.links().get(dev).await {
                            Err(e) => {
                                if warned.insert(dev.id.clone()) {
                                    show.publish(json!({"type": "error", "message": e}));
                                }
                            }
                            Ok(_) => {
                                if let Some(p) = profiles::probe(&dev.kind) {
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
            "edit_locked": self.edit_key.lock().unwrap().is_some(),
            "resumed": self.resumed.lock().unwrap().clone(),
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
            (g.engine.pending_timers(), g.engine.pending_cues())
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
            let _ = g.store.append(ts, "studio", Some(&show_id), "show.edit", &json!({"changes": d.changes, "notes": d.notes, "undo": is_undo}));
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
            let msgs = profiles::stop_messages(&dev.kind);
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
        let checks = preflight::run(&chart, out.as_deref(), &ready, &state);
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
        let packets = match output::packets_for(&dev, &action) {
            Ok(p) => p,
            Err(e) => return Err(e),
        };
        let sent_as: Vec<Value> = packets.iter().flat_map(|p| links::flatten(p)).map(|(a, args)| json!({"address": a, "args": args})).collect();
        let mut m = json!({"type": "delivery", "ts": now_ms(), "kind": kind, "lane": null, "cue": null, "device": dev.id, "label": dev.label, "dept": dev.dept, "target": dev.address(), "sent_as": sent_as, "manual": true});
        if dev.dept == "laser" && !out.is_armed(&dev.id) {
            m["status"] = json!("held");
            m["error"] = json!(format!("{} is not armed: arm it first", dev.label));
        } else if !out.sending() {
            m["status"] = json!("dry-run");
            m["note"] = json!("sending is off: this is exactly what WOULD be sent");
        } else {
            match out.send_packets(&dev, packets).await {
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
    pub fn propose(self: &Arc<Self>, actor: &Actor, chart: Showchart, devices: Option<Vec<rig::Device>>) -> Result<Value, (u16, Value)> {
        let preview = self.replace(chart.clone(), devices.clone(), false)?;
        let strings = |k: &str| preview["diff"][k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
        let (changes, notes) = (strings("changes"), strings("notes"));
        if changes.is_empty() {
            return Err((409, json!({"error": "nothing to propose: that is the show already running"})));
        }
        let base = self.base_hash();
        let ts = now_ms();
        let (id, view) = {
            let mut ps = self.gov.proposals.lock().unwrap();
            let id = ps.iter().map(|p| p.id).max().unwrap_or(0) + 1;
            let receipt = self.receipt("show.propose", "ESCALATE", actor, &format!("proposed edit #{id} ({} change(s)); waiting for a person", changes.len()));
            ps.push(Proposal { id, actor: actor.name(), ts, status: "pending".into(), changes, notes, receipt, base, chart, devices });
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
        let (chart, devices, by, changes) = {
            let mut ps = self.gov.proposals.lock().unwrap();
            let p = ps.iter_mut().find(|p| p.id == id).ok_or((404u16, json!({"error": format!("no proposal #{id}")})))?;
            if p.status != "pending" {
                return Err((409, json!({"error": format!("proposal #{id} is {}, not pending", p.status), "status": p.status})));
            }
            p.status = "approving".into();
            (p.chart.clone(), p.devices.clone(), p.actor.clone(), p.changes.len())
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
                v["receipt"] = json!(self.receipt("show.edit", "ALLOW", actor, &format!("approved proposal #{id} from {by} and applied it ({changes} change(s))")));
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
        let ts = now_ms();
        let event = cue.action.to_event();
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, &show_id, None, &cue.action.kind, &event);
            // what the show planned against what happened, for the run report
            let actual = (ts - g.entered_at).max(0);
            let rec = json!({"scene": g.engine.current_state, "lane": cue.lane, "cue": cue.cue, "num": cue.num, "kind": cue.action.kind,
                             "planned_ms": cue.at_ms, "actual_ms": actual, "drift_ms": actual - cue.at_ms as i64});
            let _ = g.store.append(ts, "runtime", Some(&show_id), "show.cue", &rec);
        }
        self.publish(json!({"type": "action", "ts": ts, "event": event, "lane": cue.lane, "cue": cue.cue}));
        self.route(ts, &cue.action, Some(&cue.lane), Some(&cue.cue), cue.dept.as_deref(), cue.device.as_deref());
    }

    /// Send an action to the device that handles it and report what happened as a
    /// `delivery` message: status "sent", "dry-run", "no-device" or "failed". Never blocks the show.
    fn route(self: &Arc<Self>, ts: i64, action: &Action, lane: Option<&str>, cue: Option<&str>, dept: Option<&str>, device: Option<&str>) {
        let Some(out) = self.output.lock().unwrap().clone() else { return };
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
        .route("/ws", get(ws_upgrade))
        .with_state(App { show, token })
}

fn authorized(token: &Option<String>, q: &HashMap<String, String>, h: &HeaderMap) -> bool {
    let Some(t) = token else { return true };
    let bearer = h.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    q.get("token").map(String::as_str) == Some(t.as_str()) || bearer == Some(t.as_str())
}

type Reply = (StatusCode, Json<Value>);

/// Who is asking: `X-Pchi-Actor: agent:<name>` (or `?actor=`), otherwise a person.
fn actor_of(q: &HashMap<String, String>, h: &HeaderMap) -> Actor {
    Actor::from_headers(h, q.get("actor").map(String::as_str))
}

/// An agent asked for something only a person may do: refuse, and write the refusal down.
fn refuse_agent(show: &Show, actor: &Actor, rule: &str, what: &str) -> Reply {
    let receipt = show.receipt(rule, "DENY", actor, &format!("refused: agents may not {what}"));
    (StatusCode::FORBIDDEN, Json(json!({"error": format!("agents may not {what}: they can propose a show edit for a person to approve"), "denied": true, "agent": true, "receipt": receipt})))
}

/// The operator's key is missing or wrong: refuse, and write the refusal down.
fn refuse_key(show: &Show, actor: &Actor, rule: &str, what: &str) -> Reply {
    show.receipt(rule, "DENY", actor, &format!("refused: {what} needs the operator key"));
    locked()
}

/// For routes only a person with the key may use (agents are refused first).
fn person_with_key(a: &App, actor: &Actor, q: &HashMap<String, String>, h: &HeaderMap, rule: &str, what: &str) -> Result<(), Reply> {
    if actor.is_agent() {
        return Err(refuse_agent(&a.show, actor, rule, what));
    }
    if !a.show.edit_ok(edit_key_of(q, h)) {
        return Err(refuse_key(&a.show, actor, rule, what));
    }
    Ok(())
}

fn unauthorized() -> Reply {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "token required"})))
}

/// `POST /show` (a show file, as Studio saves it) replaces the running show's definition;
/// `?dry=1` only reports what would change. See `Show::replace`.
/// From an agent (`X-Pchi-Actor: agent:...`) a real send is stored as a proposal for a person to approve.
async fn post_show(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, body: String) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    let dry = q.get("dry").is_some_and(|v| v == "1" || v == "true");
    if !actor.is_agent() && !a.show.edit_ok(edit_key_of(&q, &h)) {
        return refuse_key(&a.show, &actor, "show.edit", "sending a show");
    }
    let (chart, devices) = match rig::load(&body) {
        Ok(x) => x,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    };
    let result = if dry {
        a.show.replace(chart, devices, false)
    } else if actor.is_agent() {
        a.show.propose(&actor, chart, devices)
    } else {
        a.show.replace(chart, devices, true).inspect(|v| {
            let n = v["diff"]["changes"].as_array().map_or(0, |c| c.len());
            a.show.receipt("show.edit", "ALLOW", &actor, &format!("sent an edited show ({n} change(s))"));
        })
    };
    match result {
        Ok(v) if v["proposed"] == json!(true) => (StatusCode::ACCEPTED, Json(v)),
        Ok(v) => (StatusCode::OK, Json(v)),
        Err((code, v)) => (StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), Json(v)),
    }
}

/// The operator's key, from `X-Pchi-Edit-Key` or `?edit_key=`.
fn edit_key_of<'a>(q: &'a HashMap<String, String>, h: &'a HeaderMap) -> Option<&'a str> {
    h.get("x-pchi-edit-key").and_then(|v| v.to_str().ok()).or_else(|| q.get("edit_key").map(String::as_str))
}

fn locked() -> Reply {
    (StatusCode::FORBIDDEN, Json(json!({"error": "the operator key is required for this", "locked": true})))
}

/// `POST /show/undo` (`?dry=1` to preview): put the show back as it was before the last send.
async fn post_undo(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    let dry = q.get("dry").is_some_and(|v| v == "1" || v == "true");
    if !dry {
        if let Err(r) = person_with_key(&a, &actor, &q, &h, "show.undo", "undo a send") {
            return r;
        }
    } else if !actor.is_agent() && !a.show.edit_ok(edit_key_of(&q, &h)) {
        return locked();
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

/// `POST /devices/<id>/try` {"value": "3"}: one cue to one device through its profile. Needs the operator key.
async fn post_try(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    if let Err(r) = person_with_key(&a, &actor, &q, &h, "device.try", "send a cue to a device") {
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

/// `POST /laser/stop`: the hard stop. Needs the feed token (if any) but NOT the operator key: anyone on the crew can stop, agents included.
async fn post_laser_stop(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    let v = a.show.laser_stop();
    a.show.receipt("laser.stop", "ALLOW", &actor, &format!("hard stop: disarmed {} laser(s)", v["stopped"].as_array().map_or(0, |s| s.len())));
    (StatusCode::OK, Json(v))
}

/// `POST /devices/<id>/arm?armed=1|0`: arming needs the operator key and a person; disarming never does (agents may disarm).
async fn post_arm(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    let armed = q.get("armed").is_some_and(|v| v == "1" || v == "true");
    if armed {
        if let Err(r) = person_with_key(&a, &actor, &q, &h, "laser.arm", "arm a laser") {
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
    let actor = actor_of(&q, &h);
    if let Err(r) = person_with_key(&a, &actor, &q, &h, "device.test", "test-fire a device") {
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

/// `GET /proposals`: every proposal (pending first in the Studio's list; settled ones stay for the record).
async fn get_proposals(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    (StatusCode::OK, Json(a.show.proposals()))
}

/// `POST /proposals/<id>/approve|reject`: a person with the operator key decides.
async fn post_decide(State(a): State<App>, Path((id, verb)): Path<(u32, String)>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let actor = actor_of(&q, &h);
    let (rule, what) = match verb.as_str() {
        "approve" => ("show.edit", "approve a proposal"),
        "reject" => ("show.reject", "reject a proposal"),
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error": "use approve or reject"}))),
    };
    if let Err(r) = person_with_key(&a, &actor, &q, &h, rule, what) {
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
    let actor = actor_of(&q, &h);
    if actor.is_agent() {
        return refuse_agent(&a.show, &actor, "show.go", "run the show (send an input)");
    }
    let applied = a.show.apply("http", event);
    if applied {
        a.show.receipt("show.go", "ALLOW", &actor, &format!("input '{event}' moved the show to {}", a.show.current_state()));
    }
    (StatusCode::OK, Json(json!({"applied": applied})))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let show = a.show;
    let actor = actor_of(&q, &h);
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
    // an agent may report readiness, but it may not run the show
    if actor.is_agent() && matches!(kind, Some("input") | Some("context") | Some("reset")) {
        show.receipt("show.go", "DENY", actor, &format!("refused: agents may not run the show ({})", kind.unwrap_or("")));
        show.publish(json!({"type": "error", "message": "agents may not run the show: they can propose a show edit for a person to approve"}));
        return;
    }
    match kind {
        Some("input") => {
            if let Some(e) = v["event"].as_str() {
                if show.apply("studio", e) {
                    show.receipt("show.go", "ALLOW", actor, &format!("input '{e}' moved the show to {}", show.current_state()));
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
        rig::Device { id: id.into(), label: id.into(), kind: kind.into(), dept: rig::dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None }
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
}
