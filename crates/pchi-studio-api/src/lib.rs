//! Live state feed for Studio.
//!
//! The server owns the engine; Studio observes it and may send inputs.
//!
//! WebSocket `/ws`, server -> client (JSON text frames):
//!   {"type":"hello", "chart":{id,name,version}, "definition":<showchart>, "state", "context", "timers":[ms]}
//!   {"type":"transition", "ts", "source", "event", "from", "to", "applied"}
//!   {"type":"action", "ts", "event":{"type":<kind>, ...payload}}
//!   {"type":"context", "ts", "key", "value"}
//!   {"type":"error", "message"}
//! client -> server:
//!   {"type":"input","event":"START"}  {"type":"context","key":"energy","value":0.3}  {"type":"reset"}
//!
//! HTTP: `GET /state` (the hello snapshot), `GET /events` (the event log).
//! Every input is recorded to the event store, so a live session can be replayed.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use pchi_eventstore::{recorder, EventStore};
use pchi_showchart::{timer_event, ShowEngine, Showchart};
use serde_json::{json, Value};
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
}

impl Show {
    /// Validates the chart, begins a recorded run and arms the initial state's timers.
    pub fn start(chart: Showchart, store: EventStore) -> Result<Arc<Self>, String> {
        let engine = ShowEngine::new(chart)
            .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
        let ts = now_ms();
        recorder::begin_run(&store, &engine, ts).map_err(|e| e.to_string())?;
        recorder::record_start(&store, &engine, ts).map_err(|e| e.to_string())?;
        let timers = engine.pending_timers();
        let show = Arc::new(Self {
            inner: Mutex::new(Inner { engine, store, generation: 0, entered_at: ts }),
            tx: broadcast::channel(1024).0,
        });
        show.arm(0, timers);
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
            // elapsed, not a timestamp: the client counts from its own clock
            "elapsed_ms": (now_ms() - g.entered_at).max(0),
        })
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
                        armed = Some((g.generation, g.engine.pending_timers()));
                    }
                }
                Err(e) => {
                    applied = false;
                    out.push(json!({"type": "error", "message": e.to_string()}));
                }
            }
        }
        out.into_iter().for_each(|m| self.publish(m));
        if let Some((generation, timers)) = armed {
            self.arm(generation, timers);
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
        let (generation, timers) = {
            let mut g = self.inner.lock().unwrap();
            let chart = g.engine.chart.clone();
            let ts = now_ms();
            let Ok(engine) = ShowEngine::new(chart) else { return };
            let _ = recorder::begin_run(&g.store, &engine, ts);
            let _ = recorder::record_start(&g.store, &engine, ts);
            g.engine = engine;
            g.generation += 1;
            g.entered_at = ts;
            (g.generation, g.engine.pending_timers())
        };
        self.publish(self.snapshot());
        self.arm(generation, timers);
    }

    /// One task per timed transition: after the delay, fire the timer event
    /// unless the show has left the state it was armed for.
    fn arm(self: &Arc<Self>, generation: u64, timers: Vec<u64>) {
        for ms in timers {
            let show = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let current = show.inner.lock().unwrap().generation;
                if current == generation {
                    show.apply("timer", &timer_event(ms));
                }
            });
        }
    }
}

pub fn router(show: Arc<Show>) -> Router {
    Router::new()
        .route("/state", get(|State(s): State<Arc<Show>>| async move { Json(s.snapshot()) }))
        .route("/events", get(|State(s): State<Arc<Show>>| async move { Json(s.events()) }))
        .route("/ws", get(ws_upgrade))
        .with_state(show)
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(show): State<Arc<Show>>) -> impl IntoResponse {
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

    #[tokio::test]
    async fn reset_starts_a_fresh_run() {
        let show = Show::start(festival(), EventStore::open_in_memory().unwrap()).unwrap();
        show.apply("studio", "START");
        show.reset();
        assert_eq!(show.snapshot()["state"], "PRESHOW");
    }
}
