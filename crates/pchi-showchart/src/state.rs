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
}
