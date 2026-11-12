//! Drives a ShowEngine in real time: external events in, timer events
//! injected, every resulting action published on the bus.
//!
//! The engine itself stays pure and deterministic; all wall-clock behaviour
//! lives here. Timers are measured from the moment a state is entered and are
//! cancelled by leaving it. A timed transition whose guard fails when it
//! fires is simply skipped (it does not retry until the state is re-entered).

use crate::EventBus;
use pchi_showchart::{timer_event, ShowEngine};
use std::collections::HashSet;
use tokio::{
    sync::mpsc,
    time::{sleep_until, Duration, Instant},
};

/// Publishes the initial state's enter actions, then runs until `events` is
/// closed and no timer is left. Returns the engine in its final state.
pub async fn run_show(mut engine: ShowEngine, bus: EventBus, mut events: mpsc::Receiver<String>) -> ShowEngine {
    engine.start().iter().for_each(|a| {
        bus.publish_action(a);
    });
    let mut entered = Instant::now();
    let mut fired: HashSet<u64> = HashSet::new();
    let mut open = true;

    loop {
        let next = engine.pending_timers().into_iter().find(|ms| !fired.contains(ms));
        if !open && next.is_none() {
            break;
        }
        let deadline = next.map(|ms| entered + Duration::from_millis(ms));

        let event: Option<String> = tokio::select! {
            ev = events.recv(), if open => {
                if ev.is_none() {
                    open = false;
                }
                ev
            }
            _ = async {
                match deadline {
                    Some(d) => sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let ms = next.expect("deadline implies a pending timer");
                fired.insert(ms);
                Some(timer_event(ms))
            }
        };

        let Some(ev) = event else { continue };
        if let Some(actions) = engine.try_dispatch(&ev) {
            actions.iter().for_each(|a| {
                bus.publish_action(a);
            });
            entered = Instant::now();
            fired.clear();
        }
    }
    engine
}

#[cfg(test)]
mod tests {
    use super::*;
    use pchi_showchart::Showchart;
    use serde_json::json;

    fn festival(energy: f64) -> ShowEngine {
        let mut c = Showchart::from_json(include_str!("../../pchi-showchart/examples/festival.pchi.json")).unwrap();
        c.context.insert("energy".into(), json!(energy));
        ShowEngine::new(c).unwrap()
    }

    fn kinds_scenes(v: serde_json::Value) -> (String, String) {
        (v["type"].as_str().unwrap().to_string(), v["scene"].as_str().unwrap_or("").to_string())
    }

    #[tokio::test(start_paused = true)]
    async fn timed_transition_fires_after_its_delay_and_publishes_actions() {
        let bus = EventBus::new(64);
        let mut sub = bus.subscribe();
        let (tx, rx) = mpsc::channel(8);
        let t0 = Instant::now();
        let handle = tokio::spawn(run_show(festival(0.9), bus, rx));

        tx.send("START".into()).await.unwrap();
        assert_eq!(kinds_scenes(sub.recv().await.unwrap()), ("scene.activate".into(), "intro".into()));
        // nothing else is sent: the 10s timer must carry the show into PORTAL
        assert_eq!(kinds_scenes(sub.recv().await.unwrap()), ("scene.deactivate".into(), "intro".into()));
        assert_eq!(kinds_scenes(sub.recv().await.unwrap()), ("scene.activate".into(), "portal".into()));
        assert!(t0.elapsed() >= Duration::from_millis(10_000));

        drop(tx);
        assert_eq!(handle.await.unwrap().current_state, "PORTAL");
    }

    #[tokio::test(start_paused = true)]
    async fn guard_failing_at_fire_time_skips_the_timed_transition() {
        let bus = EventBus::new(64);
        let (tx, rx) = mpsc::channel(8);
        let handle = tokio::spawn(run_show(festival(0.1), bus, rx));
        tx.send("START".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30_000)).await;
        drop(tx);
        assert_eq!(handle.await.unwrap().current_state, "INTRO");
    }

    #[tokio::test(start_paused = true)]
    async fn leaving_a_state_cancels_its_timer() {
        let bus = EventBus::new(64);
        let mut sub = bus.subscribe();
        let (tx, rx) = mpsc::channel(8);
        let handle = tokio::spawn(run_show(festival(0.9), bus, rx));
        tx.send("START".into()).await.unwrap();
        tx.send("OPEN_PORTAL".into()).await.unwrap(); // leaves INTRO before 10s
        tokio::time::sleep(Duration::from_millis(60_000)).await;
        drop(tx);
        assert_eq!(handle.await.unwrap().current_state, "PORTAL");
        let mut n = 0;
        // the runner (and its bus) is gone, so just drain what was buffered
        while sub.try_recv().is_ok() {
            n += 1;
        }
        // enter INTRO (1) + exit INTRO and enter PORTAL (2); the old timer added nothing
        assert_eq!(n, 3);
    }
}
