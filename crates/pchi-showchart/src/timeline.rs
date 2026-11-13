//! Per-state timelines: lanes of cues that play while a state is active.
//!
//! A state can carry a timeline: several lanes (visual, lighting, laser,
//! audio ...), each holding cues placed at an offset from the moment the
//! state was entered. Leaving the state cancels the cues that have not fired.
//! Like transitions, cues only ever produce [`Action`]s; the runtime decides
//! when to publish them.

use crate::action::Action;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Intended length of the state, for the editor's ruler. Not enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub lanes: Vec<Lane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Department the lane belongs to ("visual", "lighting", "laser", "audio", ...). Informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dept: Option<String>,
    /// Id of the rig device (Studio's Rig tab) that handles this lane. Informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default)]
    pub cues: Vec<Cue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub id: String,
    /// Offset from entering the state, in milliseconds.
    pub at_ms: u64,
    /// How long the cue lasts (display only: the action fires once, at `at_ms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The cue number a stage manager calls (Q12, or 12.5 for one inserted later). Assigned once and kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num: Option<f64>,
    pub action: Action,
}

/// A cue with the lane it sits in, ready to be scheduled.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledCue {
    pub at_ms: u64,
    pub lane: String,
    pub cue: String,
    pub action: Action,
}

impl Timeline {
    /// Every cue in firing order. Cues at the same time keep lane order, then cue order.
    pub fn scheduled(&self) -> Vec<ScheduledCue> {
        let mut v: Vec<ScheduledCue> = self
            .lanes
            .iter()
            .flat_map(|l| {
                l.cues.iter().map(move |c| ScheduledCue {
                    at_ms: c.at_ms,
                    lane: l.id.clone(),
                    cue: c.id.clone(),
                    action: c.action.clone(),
                })
            })
            .collect();
        v.sort_by_key(|c| c.at_ms); // stable: ties keep lane/cue order
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cue(id: &str, at: u64, kind: &str) -> Cue {
        Cue { id: id.into(), at_ms: at, length_ms: None, label: None, num: None, action: Action::new(kind, json!({})) }
    }

    #[test]
    fn scheduled_sorts_by_time_and_keeps_lane_order_on_ties() {
        let t = Timeline {
            duration_ms: None,
            lanes: vec![
                Lane { id: "lights".into(), name: "Lights".into(), dept: Some("lighting".into()), device: None, cues: vec![cue("l1", 2000, "lighting.look"), cue("l0", 0, "lighting.look")] },
                Lane { id: "laser".into(), name: "Laser".into(), dept: None, device: None, cues: vec![cue("z1", 2000, "laser.cue")] },
            ],
        };
        let scheduled = t.scheduled();
        let order: Vec<_> = scheduled.iter().map(|c| (c.at_ms, c.cue.as_str())).collect();
        assert_eq!(order, [(0, "l0"), (2000, "l1"), (2000, "z1")]);
    }

    #[test]
    fn roundtrips_through_json_and_tolerates_missing_optionals() {
        let src = r#"{"lanes":[{"id":"a","cues":[{"id":"c","at_ms":500,"action":{"kind":"scene.activate","payload":{"scene":"x"}}}]}]}"#;
        let t: Timeline = serde_json::from_str(src).unwrap();
        assert_eq!(t.lanes[0].name, "");
        assert_eq!(t.lanes[0].cues[0].length_ms, None);
        assert_eq!(t, serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap());
    }
}
