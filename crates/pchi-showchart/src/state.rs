use crate::action::Action;
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
}
