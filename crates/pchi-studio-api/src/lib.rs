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
//!   {"type":"error", "message"}
//! client -> server:
//!   {"type":"input","event":"START"}  {"type":"context","key":"energy","value":0.3}  {"type":"reset"}
//!   {"type":"ready","dept":"lighting","ready":true,"note":"cue 12 loaded"}
//!
//! HTTP: `GET /state` (the hello snapshot), `GET /events` (the event log),
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
use futures_util::{SinkExt, StreamExt};
use pchi_eventstore::{recorder, EventStore};
use pchi_showchart::{timer_event, ScheduledCue, ShowEngine, Showchart};
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
}

impl Show {
    /// Validates the chart, begins a recorded run and arms the initial state's timers.
    pub fn start(chart: Showchart, store: EventStore) -> Result<Arc<Self>, String> {
        let engine = ShowEngine::new(chart)
            .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
        let ts = now_ms();
        recorder::begin_run(&store, &engine, ts).map_err(|e| e.to_string())?;
        recorder::record_start(&store, &engine, ts).map_err(|e| e.to_string())?;
        let show = Arc::new(Self {
            inner: Mutex::new(Inner { engine, store, generation: 0, entered_at: ts }),
            tx: broadcast::channel(1024).0,
            readiness: Mutex::new(BTreeMap::new()),
        });
        show.arm(0);
        Ok(show)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }

    /// Full picture for a client that just connected.
    pub fn snapshot(&self) -> Value {
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
            // elapsed, not a timestamp: the client counts from its own clock
            "elapsed_ms": (now_ms() - g.entered_at).max(0),
        })
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
        {
            let mut r = self.readiness.lock().unwrap();
            // a stray client cannot grow the list forever
            if !r.contains_key(&dept) && r.len() >= 64 {
                return Err("too many departments".into());
            }
            r.insert(dept.clone(), json!({"ready": ready, "note": note, "ts": ts, "source": source}));
        }
        let msg = json!({"type": "readiness", "ts": ts, "dept": dept, "ready": ready, "note": note, "source": source});
        {
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
        let (timers, cues) = {
            let g = self.inner.lock().unwrap();
            (g.engine.pending_timers(), g.engine.pending_cues())
        };
        for ms in timers {
            let show = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                if show.inner.lock().unwrap().generation == generation {
                    show.apply("timer", &timer_event(ms));
                }
            });
        }
        for cue in cues {
            let show = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(cue.at_ms)).await;
                if show.inner.lock().unwrap().generation == generation {
                    show.fire_cue(&cue);
                }
            });
        }
    }

    /// Publish (and record) the action of a timeline cue that has come due.
    fn fire_cue(&self, cue: &ScheduledCue) {
        let ts = now_ms();
        let event = cue.action.to_event();
        {
            let g = self.inner.lock().unwrap();
            let show_id = g.engine.chart.id.clone().unwrap_or_else(|| "show".into());
            let _ = g.store.append(ts, &show_id, None, &cue.action.kind, &event);
        }
        self.publish(json!({"type": "action", "ts": ts, "event": event, "lane": cue.lane, "cue": cue.cue}));
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
    let applied = a.show.apply("http", event);
    (StatusCode::OK, Json(json!({"applied": applied})))
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let show = a.show;
    ws.on_upgrade(move |socket| client(socket, show))
}

async fn client(socket: WebSocket, show: Arc<Show>) {
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
                    handle_client(&show, &t);
                }
            }
        }
    }
}

fn handle_client(show: &Arc<Show>, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    match v["type"].as_str() {
        Some("input") => {
            if let Some(e) = v["event"].as_str() {
                show.apply("studio", e);
            }
        }
        Some("context") => {
            if let Some(k) = v["key"].as_str() {
                show.set_context(k, v["value"].clone());
            }
        }
        Some("reset") => show.reset(),
        Some("ready") => {
            if let Some(d) = v["dept"].as_str() {
                if let Err(e) = show.set_ready(d, v["ready"].as_bool().unwrap_or(true), v["note"].as_str().unwrap_or(""), "ws") {
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
}
