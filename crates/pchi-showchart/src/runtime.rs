use crate::{action::Action, engine::ShowEngine};
use std::cell::RefCell;

/// Where actions go. Core implements this by publishing PCHI events.
pub trait Runtime {
    fn emit(&self, action: &Action);
}

/// Collects actions in memory (tests, `simulate`, replay).
#[derive(Default)]
pub struct RecordingRuntime {
    pub emitted: RefCell<Vec<Action>>,
}

impl Runtime for RecordingRuntime {
    fn emit(&self, action: &Action) {
        self.emitted.borrow_mut().push(action.clone());
    }
}

/// Start the show and feed events through the engine into a runtime.
/// Returns, per event, whether it caused a transition.
pub fn replay(engine: &mut ShowEngine, events: &[&str], rt: &dyn Runtime) -> Vec<bool> {
    engine.start().iter().for_each(|a| rt.emit(a));
    events
        .iter()
        .map(|e| match engine.try_dispatch(e) {
            Some(actions) => {
                actions.iter().for_each(|a| rt.emit(a));
                true
            }
            None => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Showchart;

    #[test]
    fn replay_records_actions_and_reports_ignored_events() {
        let chart = Showchart::from_json(include_str!("../examples/portal.json")).unwrap();
        let mut e = ShowEngine::new(chart).unwrap();
        let rt = RecordingRuntime::default();
        let r = replay(&mut e, &["START", "BOGUS", "PORTAL_OPEN"], &rt);
        assert_eq!(r, [true, false, true]);
        assert_eq!(rt.emitted.borrow().len(), 2);
    }
}
