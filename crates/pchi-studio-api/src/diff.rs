//! What changes when an edited show is sent to the running one.
//! Plain sentences, so the operator can read them before saying yes.

use crate::rig::Device;
use pchi_showchart::{Cue, Showchart};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Diff {
    /// What is different, one line each.
    pub changes: Vec<String>,
    /// Things that make the change unsafe: the update is refused while any remain.
    pub blockers: Vec<String>,
    /// Effects on the scene that is running right now.
    pub notes: Vec<String>,
    /// Cues the edit added, moved or re-pointed (scene, lane, cue id): used to check their devices are ready. Not sent to clients.
    #[serde(skip)]
    pub touched: Vec<(String, String, String)>,
}

type CueKey = (String, String, String); // scene, lane, cue

fn cues(c: &Showchart) -> BTreeMap<CueKey, (&Cue, String)> {
    let mut m = BTreeMap::new();
    for s in &c.states {
        for l in s.timeline.iter().flat_map(|t| &t.lanes) {
            for q in &l.cues {
                m.insert((s.id.clone(), l.id.clone(), q.id.clone()), (q, if l.name.is_empty() { l.id.clone() } else { l.name.clone() }));
            }
        }
    }
    m
}

fn name(q: &Cue) -> String {
    let n = q.num.map(|n| format!("Q{n} ")).unwrap_or_default();
    format!("{n}{}", q.label.clone().unwrap_or_else(|| q.action.kind.clone()))
}
fn secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// `current` is the running scene and `elapsed_ms` how long it has been running.
pub fn diff(old: &Showchart, new: &Showchart, old_dev: &[Device], new_dev: &[Device], current: &str, elapsed_ms: u64) -> Diff {
    let mut d = Diff::default();
    if !new.states.iter().any(|s| s.id == current) {
        d.blockers.push(format!("the running scene {current} is not in the new show: put it back, or restart the show"));
    }
    for s in &new.states {
        if !old.states.iter().any(|o| o.id == s.id) {
            d.changes.push(format!("+ scene {}", s.id));
        }
    }
    for s in &old.states {
        if !new.states.iter().any(|n| n.id == s.id) {
            d.changes.push(format!("- scene {}", s.id));
        }
    }
    let tkey = |t: &pchi_showchart::Transition| format!("{} → {} ({})", t.source, t.target, t.event.clone().or(t.after_ms.map(|a| format!("after {}", secs(a)))).unwrap_or_default());
    for t in &new.transitions {
        if !old.transitions.iter().any(|o| tkey(o) == tkey(t)) {
            d.changes.push(format!("+ transition {}", tkey(t)));
        }
    }
    for t in &old.transitions {
        if !new.transitions.iter().any(|n| tkey(n) == tkey(t)) {
            d.changes.push(format!("- transition {}", tkey(t)));
        }
    }

    let (oc, nc) = (cues(old), cues(new));
    for (k, (q, lane)) in &nc {
        let where_ = format!("{} / {}", k.0, lane);
        match oc.get(k) {
            None => {
                d.touched.push(k.clone());
                d.changes.push(format!("+ {where_}: {} at {}", name(q), secs(q.at_ms)));
                if k.0 == current && q.at_ms <= elapsed_ms {
                    d.notes.push(format!("{} is at {}, already past: it will not fire this time", name(q), secs(q.at_ms)));
                }
            }
            Some((o, _)) => {
                if o.at_ms != q.at_ms || o.action != q.action {
                    d.touched.push(k.clone());
                }
                if o.at_ms != q.at_ms {
                    d.changes.push(format!("~ {where_}: {} moved {} → {}", name(q), secs(o.at_ms), secs(q.at_ms)));
                    if k.0 == current && q.at_ms <= elapsed_ms && o.at_ms > elapsed_ms {
                        d.notes.push(format!("{} moved to {}, which has already passed: it will not fire this time", name(q), secs(q.at_ms)));
                    }
                }
                if o.action != q.action {
                    d.changes.push(format!("~ {where_}: {} action changed", name(q)));
                }
                if o.num != q.num {
                    d.changes.push(format!("~ {where_}: cue number {:?} → {:?}", o.num, q.num));
                }
            }
        }
    }
    for (k, (q, lane)) in &oc {
        if !nc.contains_key(k) {
            d.changes.push(format!("- {} / {}: {} at {}", k.0, lane, name(q), secs(q.at_ms)));
        }
    }
    // lanes pointing at a different device
    for s in &new.states {
        for l in s.timeline.iter().flat_map(|t| &t.lanes) {
            let before = old.states.iter().find(|o| o.id == s.id).and_then(|o| o.timeline.as_ref()).and_then(|t| t.lanes.iter().find(|x| x.id == l.id));
            if let Some(b) = before {
                if b.device != l.device {
                    d.changes.push(format!("~ {} / {}: device {} → {}", s.id, l.id, b.device.as_deref().unwrap_or("auto"), l.device.as_deref().unwrap_or("auto")));
                }
            }
        }
    }
    for n in new_dev {
        match old_dev.iter().find(|o| o.id == n.id) {
            None => d.changes.push(format!("+ device {}", n.label)),
            Some(o) if o.address() != n.address() => d.changes.push(format!("~ device {}: {} → {}", n.label, o.address().unwrap_or_else(|| "no address".into()), n.address().unwrap_or_else(|| "no address".into()))),
            _ => {}
        }
    }
    for o in old_dev {
        if !new_dev.iter().any(|n| n.id == o.id) {
            d.changes.push(format!("- device {}", o.label));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(json: &str) -> Showchart {
        Showchart::from_json(json).unwrap()
    }
    const A: &str = r#"{"initial":"A","states":[{"id":"A","timeline":{"lanes":[{"id":"l","name":"Lights","dept":"lighting","cues":[
        {"id":"c1","at_ms":1000,"num":1,"action":{"kind":"lighting.look","payload":{"look":"blue"}}},
        {"id":"c2","at_ms":5000,"num":2,"action":{"kind":"lighting.look","payload":{"look":"red"}}}]}]}},{"id":"B"}],
        "transitions":[{"source":"A","target":"B","event":"NEXT"}]}"#;

    #[test]
    fn identical_shows_have_no_differences() {
        let d = diff(&show(A), &show(A), &[], &[], "A", 0);
        assert_eq!(d, Diff::default());
    }

    #[test]
    fn reports_added_moved_changed_and_removed_things_in_words() {
        let new = A
            .replace(r#""at_ms":5000"#, r#""at_ms":500"#) // c2 moved earlier than now
            .replace(r#""look":"blue""#, r#""look":"green""#)
            .replace(r#"{"id":"B"}"#, r#"{"id":"B"},{"id":"C"}"#);
        let d = diff(&show(A), &show(&new), &[], &[], "A", 2000);
        assert!(d.changes.contains(&"+ scene C".to_string()), "{:?}", d.changes);
        assert!(d.changes.iter().any(|c| c.contains("Q2") && c.contains("moved 5.0s → 0.5s")), "{:?}", d.changes);
        assert!(d.changes.iter().any(|c| c.contains("Q1") && c.contains("action changed")));
        assert!(d.notes.iter().any(|n| n.contains("already passed")), "{:?}", d.notes);
        assert!(d.blockers.is_empty());
    }

    #[test]
    fn removing_the_running_scene_blocks_the_update() {
        let new = r#"{"initial":"B","states":[{"id":"B"}],"transitions":[]}"#;
        let d = diff(&show(A), &show(new), &[], &[], "A", 0);
        assert_eq!(d.blockers.len(), 1);
        assert!(d.blockers[0].contains("running scene A"));
        assert!(d.changes.contains(&"- scene A".to_string()));
    }
}
