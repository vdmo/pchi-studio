//! Replay: re-run the recorded inputs of the latest run through a fresh
//! engine built from the chart stored in the log, and compare every outcome
//! with what was recorded. Replay uses the recording, not a simulation.

use crate::recorder::{RUN_START, SHOW_CONTEXT, SHOW_INPUT};
use crate::store::{EventStore, StoreError};
use pchi_showchart::{Action, ShowEngine, Showchart};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("the log contains no run (no 'run.start' record)")]
    NoRun,
    #[error("the recorded chart cannot be read: {0}")]
    BadChart(String),
    #[error("the recorded chart is invalid: {0}")]
    InvalidChart(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayStep {
    pub timestamp: i64,
    pub event: String,
    pub from: String,
    pub to: String,
    pub applied: bool,
    pub actions: Vec<Action>,
}

#[derive(Debug)]
pub struct ReplayReport {
    pub chart_id: String,
    pub initial: String,
    pub steps: Vec<ReplayStep>,
    /// Places where replaying the inputs did not reproduce the recording.
    pub divergences: Vec<String>,
    pub final_state: String,
}

impl ReplayReport {
    pub fn matches_recording(&self) -> bool {
        self.divergences.is_empty()
    }
}

pub fn replay_latest(store: &EventStore) -> Result<ReplayReport, ReplayError> {
    let records = store.all()?;
    let start = records.iter().rposition(|r| r.event_type == RUN_START).ok_or(ReplayError::NoRun)?;

    let chart: Showchart = serde_json::from_value(records[start].payload["chart"].clone())
        .map_err(|e| ReplayError::BadChart(e.to_string()))?;
    let mut engine = ShowEngine::new(chart.clone()).map_err(|errs| {
        ReplayError::InvalidChart(errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
    })?;

    let mut report = ReplayReport {
        chart_id: chart.id.clone().unwrap_or_else(|| "show".into()),
        initial: engine.current_state.clone(),
        steps: vec![],
        divergences: vec![],
        final_state: String::new(),
    };

    for r in &records[start + 1..] {
        match r.event_type.as_str() {
            SHOW_CONTEXT => {
                if let Some(key) = r.payload["key"].as_str() {
                    engine.set_context(key, r.payload["value"].clone());
                }
            }
            SHOW_INPUT => {
                let event = r.payload["event"].as_str().unwrap_or("").to_string();
                let from = engine.current_state.clone();
                let result = engine.try_dispatch(&event);
                let applied = result.is_some();
                let actions = result.unwrap_or_default();
                let to = engine.current_state.clone();

                let rec_applied = r.payload["applied"].as_bool().unwrap_or(false);
                let rec_to = r.payload["to"].as_str().unwrap_or("");
                let rec_kinds: Vec<&str> = r.payload["actions"].as_array().map_or(vec![], |a| a.iter().filter_map(|k| k.as_str()).collect());
                let kinds: Vec<&str> = actions.iter().map(|a| a.kind.as_str()).collect();
                if applied != rec_applied || to != rec_to || kinds != rec_kinds {
                    report.divergences.push(format!(
                        "record #{}: '{}' recorded {}→{} ({:?}) but replay gave {}→{} ({:?})",
                        r.id, event, r.payload["from"].as_str().unwrap_or("?"), rec_to, rec_kinds, from, to, kinds
                    ));
                }
                report.steps.push(ReplayStep { timestamp: r.timestamp, event, from, to, applied, actions });
            }
            _ => {} // emitted actions: re-derived above, not replayed from the log
        }
    }
    report.final_state = engine.current_state;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::*;
    use serde_json::json;

    fn chart() -> Showchart {
        Showchart::from_json(include_str!("../../pchi-showchart/examples/festival.pchi.json")).unwrap()
    }

    fn record_session(store: &EventStore) {
        let mut e = ShowEngine::new(chart()).unwrap();
        begin_run(store, &e, 1000).unwrap();
        record_start(store, &e, 1000).unwrap();
        record_input(store, &mut e, "cli", "START", 1100).unwrap();
        record_context(store, &mut e, "energy", json!(0.2), 1150).unwrap();
        record_input(store, &mut e, "cli", "OPEN_PORTAL", 1200).unwrap(); // guard blocks -> CALM
        record_input(store, &mut e, "cli", "BOGUS", 1300).unwrap(); // ignored
    }

    #[test]
    fn a_recorded_run_replays_identically_from_the_db_alone() {
        let store = EventStore::open_in_memory().unwrap();
        record_session(&store);
        let r = replay_latest(&store).unwrap();
        assert!(r.matches_recording(), "{:?}", r.divergences);
        assert_eq!(r.chart_id, "festival-demo");
        assert_eq!(r.initial, "PRESHOW");
        let path: Vec<_> = r.steps.iter().map(|s| (s.event.as_str(), s.to.as_str(), s.applied)).collect();
        assert_eq!(path, [("START", "INTRO", true), ("OPEN_PORTAL", "CALM", true), ("BOGUS", "CALM", false)]);
        assert_eq!(r.final_state, "CALM");
    }

    #[test]
    fn emitted_actions_are_logged_as_their_own_events() {
        let store = EventStore::open_in_memory().unwrap();
        record_session(&store);
        let kinds: Vec<_> = store.all().unwrap().into_iter().map(|r| r.event_type).filter(|t| t.starts_with("scene.")).collect();
        // START: enter INTRO; OPEN_PORTAL: exit INTRO, enter CALM
        assert_eq!(kinds, ["scene.activate", "scene.deactivate", "scene.activate"]);
    }

    #[test]
    fn tampering_with_the_log_is_reported_as_a_divergence() {
        let store = EventStore::open_in_memory().unwrap();
        let mut e = ShowEngine::new(chart()).unwrap();
        begin_run(&store, &e, 1).unwrap();
        record_input(&store, &mut e, "cli", "START", 2).unwrap();
        // a forged record claiming START led somewhere else
        store.append(3, "cli", None, SHOW_INPUT, &json!({"event": "START", "from": "INTRO", "to": "PORTAL", "applied": true, "actions": []})).unwrap();
        let r = replay_latest(&store).unwrap();
        assert!(!r.matches_recording());
    }

    #[test]
    fn only_the_latest_run_is_replayed() {
        let store = EventStore::open_in_memory().unwrap();
        record_session(&store);
        let mut e = ShowEngine::new(chart()).unwrap();
        begin_run(&store, &e, 9000).unwrap();
        record_input(&store, &mut e, "cli", "START", 9100).unwrap();
        let r = replay_latest(&store).unwrap();
        assert_eq!(r.steps.len(), 1);
        assert_eq!(r.final_state, "INTRO");
    }

    #[test]
    fn empty_log_has_no_run() {
        assert!(matches!(replay_latest(&EventStore::open_in_memory().unwrap()), Err(ReplayError::NoRun)));
    }
}
