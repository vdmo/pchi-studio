use crate::{
    guard::{self, Context},
    state::State,
    transition::{timer_event, Transition},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Showchart {
    /// Schema URL, e.g. "https://pchi.life/schema/showchart-v0.1.json". Informational.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub initial: String,
    /// Starting values for the show context (guards read these).
    #[serde(default, skip_serializing_if = "Context::is_empty")]
    pub context: Context,
    /// Named guard expressions, e.g. {"energy_high": "energy > 0.7"}.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub guards: BTreeMap<String, String>,
    /// Highest cue number ever handed out, so a deleted cue's number is never given to a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue_seq: Option<u64>,
    pub states: Vec<State>,
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    #[error("initial state '{0}' does not exist")]
    UnknownInitial(String),
    #[error("duplicate state id '{0}'")]
    DuplicateState(String),
    #[error("transition on '{event}' has unknown source '{state}'")]
    UnknownSource { event: String, state: String },
    #[error("transition on '{event}' has unknown target '{state}'")]
    UnknownTarget { event: String, state: String },
    #[error("state '{state}' has two unguarded transitions on '{event}' (the second can never fire)")]
    AmbiguousTransition { state: String, event: String },
    #[error("transition {from} -> {to} has neither an event nor after_ms")]
    MissingTrigger { from: String, to: String },
    #[error("transition on '{event}' has an invalid guard '{guard}': {reason}")]
    InvalidGuard { event: String, guard: String, reason: String },
    #[error("state '{state}' has a timer after {after_ms} ms but must run at least {min_ms} ms: the timer would be cut short")]
    TimerBeforeMinimum { state: String, after_ms: u64, min_ms: u64 },
    #[error("state '{0}' is unreachable from the initial state")]
    Unreachable(String),
}

impl Showchart {
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn state(&self, id: &str) -> Option<&State> {
        self.states.iter().find(|s| s.id == id)
    }

    /// A transition's guard may be a name in `guards` or an inline expression.
    pub fn guard_expr<'a>(&'a self, guard: &'a str) -> &'a str {
        self.guards.get(guard).map(String::as_str).unwrap_or(guard)
    }

    /// All structural problems; empty means the chart is runnable.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errs = Vec::new();
        let mut ids = HashSet::new();
        for s in &self.states {
            if !ids.insert(s.id.as_str()) {
                errs.push(ValidationError::DuplicateState(s.id.clone()));
            }
        }
        if !ids.contains(self.initial.as_str()) {
            errs.push(ValidationError::UnknownInitial(self.initial.clone()));
        }
        let mut seen = BTreeSet::new();
        for t in &self.transitions {
            let label = t.trigger().unwrap_or_else(|| "?".into());
            if !ids.contains(t.source.as_str()) {
                errs.push(ValidationError::UnknownSource { event: label.clone(), state: t.source.clone() });
            }
            if !ids.contains(t.target.as_str()) {
                errs.push(ValidationError::UnknownTarget { event: label.clone(), state: t.target.clone() });
            }
            if t.event.is_none() && t.after_ms.is_none() {
                errs.push(ValidationError::MissingTrigger { from: t.source.clone(), to: t.target.clone() });
            }
            match &t.guard {
                Some(g) => {
                    if let Err(e) = guard::check(self.guard_expr(g)) {
                        errs.push(ValidationError::InvalidGuard { event: label.clone(), guard: g.clone(), reason: e.to_string() });
                    }
                }
                None => {
                    // two unguarded transitions on the same trigger: the second is dead
                    let key = (t.source.as_str(), t.event.clone().unwrap_or_else(|| t.after_ms.map(timer_event).unwrap_or_default()));
                    if !seen.insert((key.0, key.1.clone())) {
                        errs.push(ValidationError::AmbiguousTransition { state: t.source.clone(), event: key.1 });
                    }
                }
            }
        }
        // a scene's own timer cannot be shorter than the least it must run
        for t in &self.transitions {
            if let (Some(after_ms), Some(min_ms)) = (t.after_ms, self.state(&t.source).and_then(|s| s.min_ms)) {
                if after_ms < min_ms {
                    errs.push(ValidationError::TimerBeforeMinimum { state: t.source.clone(), after_ms, min_ms });
                }
            }
        }
        // reachability
        let mut reach: HashSet<&str> = HashSet::new();
        let mut stack = vec![self.initial.as_str()];
        while let Some(s) = stack.pop() {
            if !reach.insert(s) {
                continue;
            }
            stack.extend(self.transitions.iter().filter(|t| t.source == s).map(|t| t.target.as_str()));
        }
        if ids.contains(self.initial.as_str()) {
            for s in &self.states {
                if !reach.contains(s.id.as_str()) {
                    errs.push(ValidationError::Unreachable(s.id.clone()));
                }
            }
        }
        errs
    }
}
