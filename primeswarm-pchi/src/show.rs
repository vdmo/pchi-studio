//! Shared show state: where the show is, who is ready, and how confident
//! the conductor can honestly be.
//!
//! This is the model behind Mission Control. Everything in a
//! [`ShowSnapshot`] is derived from what tools actually told the
//! conductor — nothing is invented to look good:
//!
//! - **current / next state** come from `show_state` `enter` messages and
//!   the `plan` (the ordered journey). A sender may override the successor.
//! - **department readiness** comes from `show_state` `readiness` reports.
//!   Readiness is sticky: it stays until the department reports again.
//! - **liveness**: any message from a source counts as seeing it. A
//!   department whose reporting source has been silent for longer than
//!   [`STALE_AFTER`] is treated as `offline`, whatever it last claimed.
//!   (Bridges send a heartbeat every 5 s, so three misses.)
//! - **confidence** is `None` until at least one department has reported —
//!   an unknown is never shown as 100%. Otherwise it is the mean readiness
//!   (ready = 1, degraded = 0.5, not ready / offline = 0) minus a penalty
//!   for rule denials in the last [`DENIAL_WINDOW`], so a show that is
//!   being refused by its own safety rules is visibly less certain.
//!
//! The formula is deliberately simple and fully reported in
//! [`Confidence`], so an operator can see *why* the number is what it is.

use chrono::{DateTime, Duration, Utc};
use pchi_schema::{ReadinessStatus, ShowStateData};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// A source silent for longer than this is considered offline.
pub const STALE_AFTER: Duration = Duration::seconds(15);

/// How far back a rule denial still lowers confidence.
pub const DENIAL_WINDOW: Duration = Duration::seconds(60);

/// Confidence lost per denial inside the window.
const PENALTY_PER_DENIAL: f64 = 0.10;

/// The most confidence a pile of denials can remove.
const MAX_DENIAL_PENALTY: f64 = 0.30;

const HISTORY_CAP: usize = 50;
const DENIAL_CAP: usize = 20;

#[derive(Debug, Clone)]
struct Department {
    status: ReadinessStatus,
    detail: Option<String>,
    source_id: String,
    reported_at: DateTime<Utc>,
}

/// One entered state, for the history trail.
#[derive(Debug, Clone, Serialize)]
pub struct Transition {
    pub state: String,
    pub entered_at: DateTime<Utc>,
    pub source_id: String,
}

/// A rule denial, remembered briefly for confidence and the live feed.
#[derive(Debug, Clone, Serialize)]
pub struct Denial {
    pub at: DateTime<Utc>,
    pub message: String,
}

/// Show-level state held by the conductor.
#[derive(Debug, Clone, Default)]
pub struct ShowState {
    show: Option<String>,
    plan: Vec<String>,
    current: Option<String>,
    explicit_next: Option<String>,
    entered_at: Option<DateTime<Utc>>,
    departments: BTreeMap<String, Department>,
    last_seen: HashMap<String, DateTime<Utc>>,
    history: VecDeque<Transition>,
    denials: VecDeque<Denial>,
    /// The state a transition is being evaluated for. Set only on a
    /// throwaway copy while transition rules run (see `rules.rs`), so
    /// rules can say `show.entering == "FINALE"`. Never set on live state.
    entering: Option<String>,
}

/// What applying a `show_state` message did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Something observable changed; worth announcing to listeners.
    Changed,
    /// The message repeated what was already true (e.g. re-entering the
    /// current state). Not announced, which is what stops two bridges
    /// echoing a transition back and forth.
    Unchanged,
}

impl ShowState {
    /// Record that `source_id` was heard from at `now`.
    pub fn note_seen(&mut self, source_id: &str, now: DateTime<Utc>) {
        self.last_seen.insert(source_id.to_string(), now);
    }

    /// Record a rule denial.
    pub fn record_denial(&mut self, now: DateTime<Utc>, message: String) {
        self.denials.push_back(Denial { at: now, message });
        while self.denials.len() > DENIAL_CAP {
            self.denials.pop_front();
        }
    }

    /// Apply a `show_state` payload from `source_id`.
    pub fn apply(&mut self, data: &ShowStateData, source_id: &str, now: DateTime<Utc>) -> Applied {
        match data {
            ShowStateData::Plan { show, states } => {
                let same = self.plan == *states && self.show == *show;
                self.plan = states.clone();
                self.show = show.clone();
                if same {
                    Applied::Unchanged
                } else {
                    Applied::Changed
                }
            }
            ShowStateData::Enter { state, next_state } => {
                let same_state = self.current.as_deref() == Some(state.as_str());
                if same_state && next_state.is_none() {
                    return Applied::Unchanged;
                }
                self.explicit_next = next_state.clone();
                if same_state {
                    return Applied::Changed;
                }
                self.current = Some(state.clone());
                self.entered_at = Some(now);
                self.history.push_back(Transition {
                    state: state.clone(),
                    entered_at: now,
                    source_id: source_id.to_string(),
                });
                while self.history.len() > HISTORY_CAP {
                    self.history.pop_front();
                }
                Applied::Changed
            }
            // Informational, from the conductor itself. Never changes state.
            ShowStateData::Rejected { .. } => Applied::Unchanged,
            ShowStateData::Readiness {
                department,
                status,
                detail,
            } => {
                let unchanged = self.departments.get(department).is_some_and(|d| {
                    d.status == *status && d.detail == *detail && d.source_id == source_id
                });
                self.departments.insert(
                    department.clone(),
                    Department {
                        status: *status,
                        detail: detail.clone(),
                        source_id: source_id.to_string(),
                        reported_at: now,
                    },
                );
                if unchanged {
                    Applied::Unchanged
                } else {
                    Applied::Changed
                }
            }
        }
    }

    /// The state a transition rule is being evaluated for, if any.
    pub fn entering(&self) -> Option<&str> {
        self.entering.as_deref()
    }

    pub fn set_entering(&mut self, state: Option<String>) {
        self.entering = state;
    }

    /// The state the show is in.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// The state `data` would move the show into, if it would change it.
    pub fn transition_target(&self, data: &ShowStateData) -> Option<String> {
        match data {
            ShowStateData::Enter { state, .. } if self.current() != Some(state.as_str()) => {
                Some(state.clone())
            }
            _ => None,
        }
    }

    /// A department's status as an operator should read it. `None` if it has
    /// never reported (rules read that as `"unknown"`, never as ready).
    pub fn effective_status(
        &self,
        department: &str,
        now: DateTime<Utc>,
    ) -> Option<EffectiveStatus> {
        let d = self.departments.get(department)?;
        Some(if self.is_offline(d, now) {
            EffectiveStatus::Offline
        } else {
            d.status.into()
        })
    }

    /// Every department's effective status, for hashing into a receipt.
    pub fn readiness_map(&self, now: DateTime<Utc>) -> BTreeMap<String, &'static str> {
        self.departments
            .keys()
            .filter_map(|n| Some((n.clone(), self.effective_status(n, now)?.as_rule_str())))
            .collect()
    }

    /// Departments that are not plainly ready (degraded, not ready, offline).
    pub fn blocker_count(&self, now: DateTime<Utc>) -> usize {
        self.departments
            .keys()
            .filter(|name| self.effective_status(name, now) != Some(EffectiveStatus::Ready))
            .count()
    }

    fn next_state(&self) -> Option<String> {
        if let Some(n) = &self.explicit_next {
            return Some(n.clone());
        }
        let current = self.current.as_ref()?;
        let at = self.plan.iter().position(|s| s == current)?;
        self.plan.get(at + 1).cloned()
    }

    fn is_offline(&self, d: &Department, now: DateTime<Utc>) -> bool {
        match self.last_seen.get(&d.source_id) {
            Some(seen) => now - *seen > STALE_AFTER,
            None => now - d.reported_at > STALE_AFTER,
        }
    }

    /// Everything Mission Control shows, as of `now`.
    pub fn snapshot(&self, now: DateTime<Utc>) -> ShowSnapshot {
        let departments: Vec<DepartmentSnapshot> = self
            .departments
            .iter()
            .map(|(name, d)| {
                let offline = self.is_offline(d, now);
                DepartmentSnapshot {
                    name: name.clone(),
                    status: if offline {
                        EffectiveStatus::Offline
                    } else {
                        d.status.into()
                    },
                    reported_status: d.status,
                    detail: d.detail.clone(),
                    source_id: d.source_id.clone(),
                    seconds_since_seen: self
                        .last_seen
                        .get(&d.source_id)
                        .map(|t| (now - *t).num_milliseconds() as f64 / 1000.0),
                }
            })
            .collect();

        let recent_denials: Vec<Denial> = self
            .denials
            .iter()
            .filter(|d| now - d.at <= DENIAL_WINDOW)
            .cloned()
            .collect();

        let blockers = departments
            .iter()
            .filter(|d| d.status != EffectiveStatus::Ready)
            .map(|d| match (&d.status, &d.detail) {
                (EffectiveStatus::Offline, _) => {
                    format!("{}: offline (source {} silent)", d.name, d.source_id)
                }
                (s, Some(detail)) => format!("{}: {} — {}", d.name, s.label(), detail),
                (s, None) => format!("{}: {}", d.name, s.label()),
            })
            .collect();

        let confidence = if departments.is_empty() {
            None
        } else {
            let weights: f64 = departments.iter().map(|d| d.status.weight()).sum();
            let readiness = weights / departments.len() as f64;
            let penalty =
                (PENALTY_PER_DENIAL * recent_denials.len() as f64).min(MAX_DENIAL_PENALTY);
            let score = ((readiness - penalty).max(0.0) * 100.0).round() as u8;
            Some(Confidence {
                score,
                readiness_pct: (readiness * 100.0).round() as u8,
                denial_penalty_pct: (penalty * 100.0).round() as u8,
                recent_denials: recent_denials.len(),
            })
        };

        ShowSnapshot {
            show: self.show.clone(),
            plan: self.plan.clone(),
            current: self.current.clone(),
            next: self.next_state(),
            entered_at: self.entered_at,
            seconds_in_state: self
                .entered_at
                .map(|t| (now - t).num_milliseconds() as f64 / 1000.0),
            departments,
            confidence,
            blockers,
            recent_denials,
            history: self.history.iter().rev().take(10).cloned().collect(),
            generated_at: now,
        }
    }
}

/// Readiness as an operator should read it: a `ready` claim from a silent
/// source is `offline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveStatus {
    Ready,
    Degraded,
    NotReady,
    Offline,
}

impl EffectiveStatus {
    fn weight(self) -> f64 {
        match self {
            Self::Ready => 1.0,
            Self::Degraded => 0.5,
            Self::NotReady | Self::Offline => 0.0,
        }
    }
    /// The word rules compare against: `readiness.lasers != "ready"`.
    pub fn as_rule_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::NotReady => "not_ready",
            Self::Offline => "offline",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::NotReady => "not ready",
            Self::Offline => "offline",
        }
    }
}

impl From<ReadinessStatus> for EffectiveStatus {
    fn from(s: ReadinessStatus) -> Self {
        match s {
            ReadinessStatus::Ready => Self::Ready,
            ReadinessStatus::Degraded => Self::Degraded,
            ReadinessStatus::NotReady => Self::NotReady,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DepartmentSnapshot {
    pub name: String,
    pub status: EffectiveStatus,
    pub reported_status: ReadinessStatus,
    pub detail: Option<String>,
    pub source_id: String,
    pub seconds_since_seen: Option<f64>,
}

/// How the confidence number was reached.
#[derive(Debug, Clone, Serialize)]
pub struct Confidence {
    pub score: u8,
    pub readiness_pct: u8,
    pub denial_penalty_pct: u8,
    pub recent_denials: usize,
}

/// The JSON served at `GET /state`.
#[derive(Debug, Clone, Serialize)]
pub struct ShowSnapshot {
    pub show: Option<String>,
    pub plan: Vec<String>,
    pub current: Option<String>,
    pub next: Option<String>,
    pub entered_at: Option<DateTime<Utc>>,
    pub seconds_in_state: Option<f64>,
    pub departments: Vec<DepartmentSnapshot>,
    /// `None` until at least one department has reported.
    pub confidence: Option<Confidence>,
    pub blockers: Vec<String>,
    pub recent_denials: Vec<Denial>,
    pub history: Vec<Transition>,
    pub generated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-12T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ready(dept: &str, status: ReadinessStatus) -> ShowStateData {
        ShowStateData::Readiness {
            department: dept.into(),
            status,
            detail: None,
        }
    }

    fn plan(states: &[&str]) -> ShowStateData {
        ShowStateData::Plan {
            show: Some("FW27".into()),
            states: states.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn enter(state: &str) -> ShowStateData {
        ShowStateData::Enter {
            state: state.into(),
            next_state: None,
        }
    }

    #[test]
    fn confidence_is_unknown_not_perfect_when_nobody_has_reported() {
        let s = ShowState::default();
        assert!(s.snapshot(t0()).confidence.is_none());
    }

    #[test]
    fn next_state_follows_the_plan_unless_overridden() {
        let mut s = ShowState::default();
        s.apply(&plan(&["PRESHOW", "LOOK_1", "FINALE"]), "x", t0());
        s.apply(&enter("PRESHOW"), "x", t0());
        assert_eq!(s.snapshot(t0()).next.as_deref(), Some("LOOK_1"));
        s.apply(
            &ShowStateData::Enter {
                state: "LOOK_1".into(),
                next_state: Some("ENCORE".into()),
            },
            "x",
            t0(),
        );
        assert_eq!(s.snapshot(t0()).next.as_deref(), Some("ENCORE"));
        s.apply(&enter("FINALE"), "x", t0());
        assert_eq!(s.snapshot(t0()).next, None, "nothing after the last state");
    }

    #[test]
    fn reentering_the_current_state_is_unchanged_so_bridges_cannot_echo() {
        let mut s = ShowState::default();
        assert_eq!(s.apply(&enter("LOOK_1"), "a", t0()), Applied::Changed);
        assert_eq!(s.apply(&enter("LOOK_1"), "b", t0()), Applied::Unchanged);
        assert_eq!(s.snapshot(t0()).history.len(), 1);
    }

    #[test]
    fn confidence_is_mean_readiness() {
        let mut s = ShowState::default();
        s.note_seen("a", t0());
        s.apply(&ready("visuals", ReadinessStatus::Ready), "a", t0());
        s.apply(&ready("lights", ReadinessStatus::Degraded), "a", t0());
        s.apply(&ready("lasers", ReadinessStatus::NotReady), "a", t0());
        s.apply(&ready("audio", ReadinessStatus::Ready), "a", t0());
        // (1 + .5 + 0 + 1) / 4 = 62.5% -> 63
        let c = s.snapshot(t0()).confidence.unwrap();
        assert_eq!(c.readiness_pct, 63);
        assert_eq!(c.score, 63);
        assert_eq!(s.snapshot(t0()).blockers.len(), 2);
    }

    #[test]
    fn a_silent_source_makes_its_department_offline_whatever_it_claimed() {
        let mut s = ShowState::default();
        s.note_seen("strobe", t0());
        s.apply(&ready("visuals", ReadinessStatus::Ready), "strobe", t0());
        let early = s.snapshot(t0() + Duration::seconds(5));
        assert_eq!(early.departments[0].status, EffectiveStatus::Ready);
        let late = s.snapshot(t0() + Duration::seconds(16));
        assert_eq!(late.departments[0].status, EffectiveStatus::Offline);
        assert_eq!(late.confidence.unwrap().score, 0);
        // A heartbeat brings it back.
        s.note_seen("strobe", t0() + Duration::seconds(17));
        let back = s.snapshot(t0() + Duration::seconds(18));
        assert_eq!(back.departments[0].status, EffectiveStatus::Ready);
    }

    #[test]
    fn recent_denials_lower_confidence_then_age_out() {
        let mut s = ShowState::default();
        s.note_seen("a", t0());
        s.apply(&ready("visuals", ReadinessStatus::Ready), "a", t0());
        s.record_denial(t0(), "strobe too bright".into());
        s.record_denial(t0(), "strobe too bright".into());
        let now = t0() + Duration::seconds(5);
        s.note_seen("a", now);
        assert_eq!(s.snapshot(now).confidence.unwrap().score, 80);
        for _ in 0..10 {
            s.record_denial(now, "x".into());
        }
        assert_eq!(
            s.snapshot(now).confidence.unwrap().score,
            70,
            "penalty is capped"
        );
        let later = t0() + Duration::seconds(70);
        s.note_seen("a", later);
        assert_eq!(
            s.snapshot(later).confidence.unwrap().score,
            100,
            "denials age out"
        );
    }

    #[test]
    fn an_unchanged_readiness_report_is_not_news() {
        let mut s = ShowState::default();
        assert_eq!(
            s.apply(&ready("audio", ReadinessStatus::Ready), "a", t0()),
            Applied::Changed
        );
        assert_eq!(
            s.apply(&ready("audio", ReadinessStatus::Ready), "a", t0()),
            Applied::Unchanged
        );
        assert_eq!(
            s.apply(&ready("audio", ReadinessStatus::Degraded), "a", t0()),
            Applied::Changed
        );
    }
}
