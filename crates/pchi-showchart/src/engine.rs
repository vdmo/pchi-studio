use crate::{
    action::Action,
    guard::{self, Context},
    showchart::{Showchart, ValidationError},
};
use serde_json::Value;

/// Result of a pure transition.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub target: String,
    /// exit actions of the source, then enter actions of the target.
    pub actions: Vec<Action>,
}

/// Pure transition function: no mutation, no I/O. `None` = event ignored.
///
/// Order: find the first transition from `current` that matches `event`
/// and whose guard (if any) holds for `ctx`; then EXIT old, change state,
/// ENTER new. A guard that fails to parse counts as not holding.
pub fn step(chart: &Showchart, ctx: &Context, current: &str, event: &str) -> Option<Step> {
    let t = chart.transitions.iter().find(|t| {
        t.source == current
            && t.matches(event)
            && t.guard.as_deref().map_or(true, |g| guard::evaluate(chart.guard_expr(g), ctx).unwrap_or(false))
    })?;
    let mut actions = chart.state(current).map(|s| s.exit_actions.clone()).unwrap_or_default();
    if let Some(next) = chart.state(&t.target) {
        actions.extend(next.enter_actions.iter().cloned());
    }
    Some(Step { target: t.target.clone(), actions })
}

/// What a dispatch did: where we were, where we are, what to emit.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionResult {
    pub previous_state: String,
    pub current_state: String,
    pub emitted_actions: Vec<Action>,
}

#[derive(Clone)]
pub struct ShowEngine {
    pub chart: Showchart,
    pub current_state: String,
    pub context: Context,
}

impl ShowEngine {
    /// Refuses charts that fail validation (unreachable states are allowed
    /// to run; only hard errors block).
    pub fn new(chart: Showchart) -> Result<Self, Vec<ValidationError>> {
        let hard: Vec<_> = chart
            .validate()
            .into_iter()
            .filter(|e| !matches!(e, ValidationError::Unreachable(_)))
            .collect();
        if !hard.is_empty() {
            return Err(hard);
        }
        let current_state = chart.initial.clone();
        let context = chart.context.clone();
        Ok(Self { chart, current_state, context })
    }

    /// Enter actions of the initial state. Call once when the show starts.
    pub fn start(&self) -> Vec<Action> {
        self.chart.state(&self.current_state).map(|s| s.enter_actions.clone()).unwrap_or_default()
    }

    /// Update a context value (from a sensor, an operator, an adapter...).
    pub fn set_context(&mut self, key: &str, value: Value) {
        self.context.insert(key.to_string(), value);
    }

    /// Delays (ms) of timed transitions leaving the current state. The
    /// scheduler measures them from state entry and injects `timer_event(ms)`.
    pub fn pending_timers(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .chart
            .transitions
            .iter()
            .filter(|t| t.source == self.current_state)
            .filter_map(|t| t.after_ms)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Cues of the current state in firing order. The scheduler measures them
    /// from state entry and drops the ones that have not fired when the state is left.
    pub fn pending_cues(&self) -> Vec<crate::ScheduledCue> {
        self.chart.state(&self.current_state).and_then(|s| s.timeline.as_ref()).map(|t| t.scheduled()).unwrap_or_default()
    }

    /// Like `try_dispatch` but reports the states as well as the actions.
    pub fn fire(&mut self, event: &str) -> Option<TransitionResult> {
        let previous_state = self.current_state.clone();
        let emitted_actions = self.try_dispatch(event)?;
        Some(TransitionResult { previous_state, current_state: self.current_state.clone(), emitted_actions })
    }

    /// `Some(actions)` if the event caused a transition, `None` if ignored.
    pub fn try_dispatch(&mut self, event: &str) -> Option<Vec<Action>> {
        let s = step(&self.chart, &self.context, &self.current_state, event)?;
        self.current_state = s.target;
        Some(s.actions)
    }

    /// Actions to emit; empty if the event was ignored (use `try_dispatch`
    /// to tell "ignored" from "transitioned with no actions").
    pub fn dispatch(&mut self, event: &str) -> Vec<Action> {
        self.try_dispatch(event).unwrap_or_default()
    }

    /// Deterministic dry run on a copy: the states visited, starting with the
    /// current one. Ignored events are skipped. Nothing is emitted and this
    /// engine is not changed.
    pub fn simulate(&self, events: &[&str]) -> Vec<String> {
        let mut e = self.clone();
        let mut path = vec![e.current_state.clone()];
        for ev in events {
            if e.try_dispatch(ev).is_some() {
                path.push(e.current_state.clone());
            }
        }
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::{timer_event, Transition};
    use serde_json::json;

    pub fn portal() -> Showchart {
        Showchart::from_json(include_str!("../examples/portal.json")).unwrap()
    }

    fn festival() -> Showchart {
        Showchart::from_json(include_str!("../examples/festival.pchi.json")).unwrap()
    }

    #[test]
    fn walks_the_portal_show() {
        let mut e = ShowEngine::new(portal()).unwrap();
        assert_eq!(e.current_state, "PRESHOW");
        e.dispatch("START");
        assert_eq!(e.current_state, "INTRO");
        e.dispatch("PORTAL_OPEN");
        assert_eq!(e.current_state, "PORTAL");
        e.dispatch("END");
        assert_eq!(e.current_state, "FINALE");
    }

    #[test]
    fn entering_a_state_emits_its_actions_not_renderer_calls() {
        let mut e = ShowEngine::new(portal()).unwrap();
        e.dispatch("START");
        let a = e.dispatch("PORTAL_OPEN");
        let kinds: Vec<_> = a.iter().map(|x| x.kind.as_str()).collect();
        assert_eq!(kinds, ["scene.activate", "renderer.activate"]);
        assert_eq!(a[0].to_event(), json!({"type": "scene.activate", "scene": "portal"}));
    }

    #[test]
    fn exit_actions_run_before_enter_actions() {
        let mut c = portal();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().exit_actions =
            vec![Action::new("scene.fadeout", json!({}))];
        let mut e = ShowEngine::new(c).unwrap();
        e.dispatch("START");
        let kinds: Vec<_> = e.dispatch("PORTAL_OPEN").into_iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ["scene.fadeout", "scene.activate", "renderer.activate"]);
    }

    #[test]
    fn unknown_event_is_ignored_and_state_is_kept() {
        let mut e = ShowEngine::new(portal()).unwrap();
        assert_eq!(e.try_dispatch("END"), None);
        assert_eq!(e.current_state, "PRESHOW");
        assert!(e.dispatch("NOPE").is_empty());
    }

    #[test]
    fn step_is_pure() {
        let c = portal();
        let ctx = Context::new();
        let a = step(&c, &ctx, "PRESHOW", "START");
        assert_eq!(a, step(&c, &ctx, "PRESHOW", "START"));
        assert_eq!(a.unwrap().target, "INTRO");
    }

    #[test]
    fn ghost_temple_v01_example_loads_and_runs() {
        let c = Showchart::from_json(include_str!("../examples/ghost-temple.pchi.json")).unwrap();
        assert_eq!(c.id.as_deref(), Some("ghost-temple"));
        assert_eq!(c.schema.as_deref(), Some("https://pchi.life/schema/showchart-v0.1.json"));
        assert!(c.validate().is_empty());
        let mut e = ShowEngine::new(c).unwrap();
        let r = e.fire("START").unwrap();
        assert_eq!((r.previous_state.as_str(), r.current_state.as_str()), ("PRESHOW", "INTRO"));
        let r = e.fire("PORTAL_OPEN").unwrap();
        assert_eq!(
            r.emitted_actions.iter().map(|a| a.kind.as_str()).collect::<Vec<_>>(),
            ["scene.activate", "output.activate"]
        );
        assert!(e.fire("START").is_none());
    }

    #[test]
    fn json_roundtrip_is_lossless() {
        for src in [
            include_str!("../examples/ghost-temple.pchi.json"),
            include_str!("../examples/festival.pchi.json"),
        ] {
            let c = Showchart::from_json(src).unwrap();
            assert_eq!(c, Showchart::from_json(&serde_json::to_string(&c).unwrap()).unwrap());
        }
    }

    // ── Phase 2 ──

    #[test]
    fn guard_blocks_the_transition_until_context_allows_it() {
        let mut c = festival();
        c.transitions.retain(|t| t.target != "CALM"); // no fallback branch: a blocked guard means "ignored"
        c.context.insert("energy".into(), json!(0.3));
        let mut e = ShowEngine::new(c).unwrap();
        e.dispatch("START");
        assert!(e.try_dispatch("OPEN_PORTAL").is_none(), "energy 0.3 must block (guard energy_high = energy > 0.7)");
        assert_eq!(e.current_state, "INTRO");
        e.set_context("energy", json!(0.9));
        assert!(e.try_dispatch("OPEN_PORTAL").is_some());
        assert_eq!(e.current_state, "PORTAL");
    }

    #[test]
    fn guards_pick_between_branches_in_order() {
        let mut e = ShowEngine::new(festival()).unwrap();
        e.dispatch("START");
        e.set_context("energy", json!(0.1));
        // second, unguarded OPEN_PORTAL branch routes to a calm scene instead
        e.dispatch("OPEN_PORTAL");
        assert_eq!(e.current_state, "CALM");
    }

    #[test]
    fn inline_guard_expression_works_without_a_named_guard() {
        let mut c = portal();
        c.transitions[0].guard = Some("audience_count >= 100".into());
        let mut e = ShowEngine::new(c).unwrap();
        assert!(e.try_dispatch("START").is_none());
        e.set_context("audience_count", json!(120));
        assert!(e.try_dispatch("START").is_some());
    }

    #[test]
    fn broken_guard_is_a_validation_error_and_never_fires() {
        let mut c = portal();
        c.transitions[0].guard = Some("energy >".into());
        assert!(matches!(c.validate()[0], ValidationError::InvalidGuard { .. }));
        assert!(ShowEngine::new(c.clone()).is_err());
        assert!(step(&c, &Context::new(), "PRESHOW", "START").is_none());
    }

    #[test]
    fn timed_transitions_are_reported_and_fire_on_the_timer_event() {
        let mut e = ShowEngine::new(festival()).unwrap();
        e.dispatch("START");
        assert_eq!(e.pending_timers(), [10_000]);
        e.set_context("energy", json!(0.9));
        let r = e.fire(&timer_event(10_000)).unwrap();
        assert_eq!(r.current_state, "PORTAL");
    }

    #[test]
    fn timeline_example_loads_and_exposes_the_current_states_cues() {
        let c = Showchart::from_json(include_str!("../examples/timelines.pchi.json")).unwrap();
        assert!(c.validate().is_empty());
        let mut e = ShowEngine::new(c).unwrap();
        assert!(e.pending_cues().is_empty(), "PRESHOW has no timeline");
        e.dispatch("START");
        let cues: Vec<_> = e.pending_cues().iter().map(|c| (c.at_ms, c.lane.clone(), c.action.kind.clone())).collect();
        assert_eq!(cues, [(0, "visual".to_string(), "scene.activate".to_string()), (2000, "lighting".to_string(), "lighting.look".to_string()), (4000, "laser".to_string(), "laser.cue".to_string())]);
        e.dispatch("DROP");
        assert_eq!(e.pending_cues().len(), 1, "leaving INTRO drops its cues; DROP has its own");
    }

    #[test]
    fn timelines_survive_a_json_roundtrip() {
        let c = Showchart::from_json(include_str!("../examples/timelines.pchi.json")).unwrap();
        assert_eq!(c, Showchart::from_json(&serde_json::to_string(&c).unwrap()).unwrap());
    }

    #[test]
    fn transition_without_a_trigger_is_rejected() {
        let mut c = portal();
        c.transitions[0] = Transition { event: None, ..Transition::on("PRESHOW", "INTRO", "x") };
        assert!(c.validate().iter().any(|e| matches!(e, ValidationError::MissingTrigger { .. })));
    }

    #[test]
    fn simulate_returns_the_path_without_touching_the_engine() {
        let e = ShowEngine::new(portal()).unwrap();
        assert_eq!(e.simulate(&["START", "BOGUS", "PORTAL_OPEN", "END"]), ["PRESHOW", "INTRO", "PORTAL", "FINALE"]);
        assert_eq!(e.current_state, "PRESHOW");
    }

    #[test]
    fn invalid_charts_are_rejected() {
        let mut c = portal();
        c.initial = "NOWHERE".into();
        c.transitions.push(Transition::on("PRESHOW", "GHOST", "X"));
        c.transitions.push(Transition::on("PRESHOW", "INTRO", "START"));
        let errs = ShowEngine::new(c).err().unwrap();
        assert!(errs.contains(&ValidationError::UnknownInitial("NOWHERE".into())));
        assert!(errs.contains(&ValidationError::UnknownTarget { event: "X".into(), state: "GHOST".into() }));
        assert!(errs.contains(&ValidationError::AmbiguousTransition { state: "PRESHOW".into(), event: "START".into() }));
    }

    #[test]
    fn unreachable_states_are_reported_by_validate() {
        let mut c = portal();
        c.states.push(crate::State { id: "ORPHAN".into(), label: String::new(), enter_actions: vec![], exit_actions: vec![], timeline: None, min_ms: None });
        assert_eq!(c.validate(), vec![ValidationError::Unreachable("ORPHAN".into())]);
        assert!(ShowEngine::new(c).is_ok());
    }
}
