use serde::{Deserialize, Serialize};

/// What fires a timed transition (the scheduler injects this as an event).
pub fn timer_event(ms: u64) -> String {
    format!("after:{ms}")
}

/// A transition fires on a named `event`, after a delay (`after_ms`), or both
/// spellings may exist on different transitions. `guard` is a guard name from
/// the chart's `guards` map, or an inline expression.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub source: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_ms: Option<u64>,
}

impl Transition {
    /// Plain event-triggered, unguarded transition.
    pub fn on(source: &str, target: &str, event: &str) -> Self {
        Self { source: source.into(), target: target.into(), event: Some(event.into()), guard: None, after_ms: None }
    }

    /// The event name that triggers this transition, if any.
    pub fn trigger(&self) -> Option<String> {
        self.event.clone().or_else(|| self.after_ms.map(timer_event))
    }

    pub fn matches(&self, event: &str) -> bool {
        self.event.as_deref() == Some(event) || self.after_ms.map(timer_event).as_deref() == Some(event)
    }

    /// Human label: `START`, `after 10000ms`, with `[guard]` appended.
    pub fn label(&self) -> String {
        let base = match (&self.event, self.after_ms) {
            (Some(e), _) => e.clone(),
            (None, Some(ms)) => format!("after {ms}ms"),
            (None, None) => "(no trigger)".into(),
        };
        match &self.guard {
            Some(g) => format!("{base} [{g}]"),
            None => base,
        }
    }
}
