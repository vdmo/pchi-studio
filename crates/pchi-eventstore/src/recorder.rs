//! Recording a run into the event store.
//!
//! Record types (all in the one append-only `events` table):
//! - `run.start`    payload {chart, context}: the whole chart, so a replay needs only the db file
//! - `show.context` payload {key, value}: a context value changed
//! - `show.input`   payload {event, from, to, applied, actions: [kind...]}: an event and its outcome
//! - anything else  an emitted action as a PCHI event (type = action kind)

use crate::store::{EventStore, Result};
use pchi_showchart::{Action, ShowEngine};
use serde_json::{json, Value};

pub const RUN_START: &str = "run.start";
pub const SHOW_CONTEXT: &str = "show.context";
pub const SHOW_INPUT: &str = "show.input";

fn show_id(engine: &ShowEngine) -> String {
    engine.chart.id.clone().unwrap_or_else(|| "show".into())
}

fn record_actions(store: &EventStore, engine: &ShowEngine, ts: i64, actions: &[Action]) -> Result<()> {
    for a in actions {
        store.append(ts, &show_id(engine), None, &a.kind, &a.to_event())?;
    }
    Ok(())
}

/// Marks the beginning of a run. Call before anything else is recorded.
pub fn begin_run(store: &EventStore, engine: &ShowEngine, ts: i64) -> Result<i64> {
    store.append(ts, &show_id(engine), None, RUN_START, &json!({ "chart": engine.chart, "context": engine.context }))
}

/// Records and returns the initial state's enter actions.
pub fn record_start(store: &EventStore, engine: &ShowEngine, ts: i64) -> Result<Vec<Action>> {
    let actions = engine.start();
    record_actions(store, engine, ts, &actions)?;
    Ok(actions)
}

pub fn record_context(store: &EventStore, engine: &mut ShowEngine, key: &str, value: Value, ts: i64) -> Result<()> {
    store.append(ts, "operator", None, SHOW_CONTEXT, &json!({ "key": key, "value": value }))?;
    engine.set_context(key, value);
    Ok(())
}

/// Dispatch `event` and record the input, its outcome, and every action.
/// `Some(actions)` if it caused a transition, `None` if it was ignored.
pub fn record_input(store: &EventStore, engine: &mut ShowEngine, source: &str, event: &str, ts: i64) -> Result<Option<Vec<Action>>> {
    let from = engine.current_state.clone();
    let result = engine.try_dispatch(event);
    let (applied, kinds): (bool, Vec<&str>) = match &result {
        Some(a) => (true, a.iter().map(|x| x.kind.as_str()).collect()),
        None => (false, vec![]),
    };
    store.append(
        ts,
        source,
        None,
        SHOW_INPUT,
        &json!({ "event": event, "from": from, "to": engine.current_state, "applied": applied, "actions": kinds }),
    )?;
    if let Some(a) = &result {
        record_actions(store, engine, ts, a)?;
    }
    Ok(result)
}
