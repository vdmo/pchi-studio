use crate::{action::Action, timeline::Timeline};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enter_actions: Vec<Action>,
    #[serde(default)]
    pub exit_actions: Vec<Action>,
    /// Lanes of cues that play while this state is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<Timeline>,
    /// The scene must run at least this long before a person or a device can move it on: an input that would
    /// leave it earlier is held (ignored, and reported) until the time is up. The show's own timers are not
    /// inputs and must not be shorter (validation says so). The longest a scene lasts is its timer transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<u64>,
}
