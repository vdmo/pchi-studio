//! `GET /describe`: the runtime describing itself to whoever asks, for an agent that has no manual.
//!
//! One answer says who the caller is and what the runtime will let them do (each action with whether it is allowed *for them*, and what
//! it would turn into), where the show is, which ways out of the current scene would fire right now, the rig and its state, what is
//! waiting for a person, what needs attention, and the rules of the house in plain sentences. The permission table is not a second copy
//! of the rules: it asks the same `allow` the routes ask, for the same caller.
//!
//! It tells the truth about the *caller's* powers and nothing about anyone else's: an agent reads that it may propose and may not press
//! GO; a person with the operator key reads that GO is theirs. Whatever it says, the routes still enforce.

use crate::governance::{Actor, Perm};
use crate::Show;
use serde_json::{json, Value};

pub const SCHEMA: &str = "pchi-describe/1";

/// One thing a caller might try: its route, the permission it needs, and what happens for an agent.
struct Act {
    id: &'static str,
    method: &'static str,
    path: &'static str,
    perm: Option<Perm>,
    what: &'static str,
    /// What the same call becomes when an agent makes it, if that is something else.
    agent: Option<&'static str>,
}

const ACTS: &[Act] = &[
    Act { id: "describe", method: "GET", path: "/describe", perm: None, what: "this description", agent: None },
    Act { id: "read_state", method: "GET", path: "/state", perm: None, what: "the scene, context, readiness, devices, proposals, and the show's definition", agent: None },
    Act { id: "doors_check", method: "GET", path: "/preflight", perm: None, what: "the whole show against the rig: what will not work", agent: None },
    Act { id: "report", method: "GET", path: "/report", perm: None, what: "cue timing, failed deliveries, edits and who decided what", agent: None },
    Act { id: "rehearse", method: "POST", path: "/rehearse", perm: None, what: "play an edited show against the run so far; changes nothing", agent: None },
    Act { id: "list_proposals", method: "GET", path: "/proposals", perm: None, what: "every proposal and where it stands", agent: None },
    Act { id: "ask_memory", method: "GET", path: "/memory?q=", perm: None, what: "what the show remembers (it says when it does not know)", agent: None },
    Act { id: "write_memory", method: "POST", path: "/memory", perm: None, what: "note something worth remembering", agent: Some("stored as a draft under your name until a person verifies it") },
    Act { id: "report_ready", method: "POST", path: "/ready/:dept", perm: None, what: "say a department is standing by or on hold", agent: None },
    Act { id: "stop_lasers", method: "POST", path: "/laser/stop", perm: None, what: "disarm every laser and send each tool's safe message", agent: None },
    Act { id: "propose_edit", method: "POST", path: "/show", perm: Some(Perm::Edit), what: "send an edited show", agent: Some("becomes a proposal that waits for a person; nothing changes until they approve it") },
    Act { id: "press_go", method: "POST", path: "/input", perm: Some(Perm::Go), what: "send an event (GO, NEXT, ...) to the show", agent: None },
    Act { id: "approve_proposal", method: "POST", path: "/proposals/:id/approve", perm: Some(Perm::Approve), what: "apply a waiting proposal", agent: None },
    Act { id: "reject_proposal", method: "POST", path: "/proposals/:id/reject", perm: Some(Perm::Approve), what: "turn a waiting proposal down", agent: None },
    Act { id: "undo_send", method: "POST", path: "/show/undo", perm: Some(Perm::Undo), what: "put the show back as it was before the last send", agent: None },
    Act { id: "arm_laser", method: "POST", path: "/devices/:id/arm?armed=1", perm: Some(Perm::Arm), what: "arm a laser so the show may send to it", agent: None },
    Act { id: "try_cue", method: "POST", path: "/devices/:id/try", perm: Some(Perm::Try), what: "send one cue to one device now", agent: None },
    Act { id: "test_fire", method: "POST", path: "/devices/:id/test", perm: Some(Perm::Test), what: "test-fire a device", agent: None },
];

/// The standing rules, as sentences an agent can follow without reading anything else.
pub const RULES: &[&str] = &[
    "Look before you act: read the state and the show, and run the doors check, before proposing anything.",
    "Rehearse every edit before you propose it. A rehearsal says which cues would no longer fire. Do not propose an edit whose rehearsal shows a cue cut off unless the person asked for exactly that and you say so.",
    "Say why in every proposal, in words. The reason is kept in the signed record next to what you proposed.",
    "You propose; a person approves. A proposal changes nothing until they do. Do not retry a proposal the person rejected without changing it and saying why.",
    "A refusal is final. If the runtime refuses an action, do not try it again, do not look for another route to the same effect, and do not ask for the operator key. Tell the person.",
    "You cannot press GO, arm a laser, approve, undo, try or test-fire. If the task needs one of those, say so and stop: that is a person's call.",
    "STOP LASERS is always yours to use, and only makes things safer. If someone may be in a beam path, use it first and explain after.",
    "When memory says it does not know, or that two notes disagree, say that. Do not fill the gap with a guess and do not write your guess down as fact: anything you write to memory is a draft.",
    "Nothing you send reaches a device unless the runtime is sending (output: live) and this runtime is the active one. Check both before telling anyone that a cue went out.",
    "UDP has no acknowledgement. 'sent' means a packet left the runtime, not that the tool did anything.",
];

fn allowed(show: &Show, actor: &Actor, perm: Perm) -> bool {
    // the same test whoami uses: for a caller limited to departments, any of theirs counts
    let depts = show.depts_of(actor);
    let probe: Vec<String> = if depts.is_empty() { vec!["laser".to_string()] } else { depts };
    probe.iter().any(|d| show.allow(actor, perm, Some(d)))
}

impl Show {
    /// The self-description for `actor` (see the module).
    pub fn describe(&self, actor: &Actor) -> Value {
        let snap = self.snapshot();
        let (chart, ctx, current) = {
            let g = self.inner.lock().unwrap();
            (g.engine.chart.clone(), g.engine.context.clone(), g.engine.current_state.clone())
        };
        let elapsed = snap["elapsed_ms"].as_i64().unwrap_or(0);
        let is_agent = actor.is_agent();
        let cur = chart.state(&current);
        let min_ms = cur.and_then(|s| s.min_ms).unwrap_or(0) as i64;
        let hold_left = (min_ms - elapsed).max(0);

        let scenes: Vec<Value> = chart
            .states
            .iter()
            .map(|s| {
                let lanes = s.timeline.as_ref().map(|t| t.lanes.as_slice()).unwrap_or(&[]);
                let mut depts: Vec<&str> = lanes.iter().filter(|l| !l.cues.is_empty()).filter_map(|l| l.dept.as_deref()).collect();
                depts.sort();
                depts.dedup();
                json!({"id": s.id, "label": s.label, "current": s.id == current, "min_ms": s.min_ms, "cues": lanes.iter().map(|l| l.cues.len()).sum::<usize>(), "departments": depts})
            })
            .collect();

        // the ways out of this scene, and whether each would fire now
        let out: Vec<Value> = chart
            .transitions
            .iter()
            .filter(|t| t.source == current)
            .map(|t| {
                let by_event = t.event.as_deref().map(|e| pchi_showchart::step(&chart, &ctx, &current, e).is_some_and(|s| s.target == t.target));
                json!({
                    "to": t.target, "event": t.event, "after_ms": t.after_ms, "guard": t.guard,
                    "would_fire_now": by_event, // None: a timer, which fires by itself
                    "held_ms": if hold_left > 0 && t.event.is_some() { json!(hold_left) } else { Value::Null },
                })
            })
            .collect();

        let events: Vec<&str> = {
            let mut e: Vec<&str> = chart.transitions.iter().filter_map(|t| t.event.as_deref()).collect();
            e.sort();
            e.dedup();
            e
        };

        let acts: Vec<Value> = ACTS
            .iter()
            .map(|a| {
                let may = match a.perm {
                    None => true,
                    Some(p) => allowed(self, actor, p),
                };
                let outcome = if is_agent && a.agent.is_some() {
                    "allowed_as_proposal"
                } else if may {
                    "allowed"
                } else {
                    "refused"
                };
                let mut v = json!({"id": a.id, "method": a.method, "path": a.path, "does": a.what, "outcome": outcome});
                if let Some(n) = a.agent.filter(|_| is_agent) {
                    v["for_you"] = json!(n);
                }
                if outcome == "refused" {
                    v["note"] = json!(if is_agent { "an agent may not do this: it is a person's call" } else { "your role does not allow this" });
                }
                v
            })
            .collect();

        let role = self.role_json();
        let sending = snap["output"] == json!("live");
        let mut attention: Vec<Value> = Vec::new();
        let mut note = |level: &str, text: String| attention.push(json!({"level": level, "text": text}));
        let rstate = role["role"].as_str().unwrap_or("active");
        if rstate != "active" {
            note("warning", format!("this runtime is {rstate}, not the active one: nothing sent here acts on the show (GET /primary says where the show is)"));
        }
        if snap["output"] == json!("dry-run") {
            note("info", "the runtime is in a dry run: cues are reported, not sent".into());
        }
        if hold_left > 0 {
            note("info", format!("{current} must run {:.1} s more before a person's input can move it on", hold_left as f64 / 1000.0));
        }
        let pending = snap["proposals"].as_array().map_or(0, |a| a.len());
        if pending > 0 {
            note("info", format!("{pending} proposal(s) are waiting for a person"));
        }
        if let Some(r) = snap["readiness"].as_object() {
            for (d, v) in r {
                if v["ready"] == json!(false) {
                    note("warning", format!("{d} is on hold{}", v["note"].as_str().filter(|n| !n.is_empty()).map(|n| format!(": {n}")).unwrap_or_default()));
                }
            }
        }
        if let Some(ds) = snap["devices"].as_array() {
            for d in ds {
                if d["armed"] == json!(true) {
                    note("warning", format!("{} is armed", d["label"].as_str().unwrap_or("a laser")));
                }
                if d["link"]["state"] == json!("lost") {
                    note("warning", format!("the link to {} is lost: it answered before and is silent now", d["label"].as_str().unwrap_or("a device")));
                }
            }
        }
        if snap["edit_locked"] == json!(true) && matches!(actor, Actor::Anonymous) {
            note("warning", "this show is locked and you have not given a key: you can only read".into());
        }

        json!({
            "schema": SCHEMA,
            "you": {"name": actor.name(), "role": actor.role(), "is_agent": is_agent, "signed_in": !matches!(actor, Actor::Anonymous), "departments": self.depts_of(actor)},
            "runtime": {"role": role["role"], "sending": sending, "output": snap["output"], "epoch": role["epoch"], "resumed": snap["resumed"], "governed": snap["governed"]},
            "show": {"name": chart.name, "id": chart.id, "initial": chart.initial, "scenes": scenes, "events": events},
            "now": {"scene": current, "elapsed_ms": elapsed, "context": ctx, "min_ms": cur.and_then(|s| s.min_ms), "held_for_ms": hold_left, "ways_out": out, "timecode": snap["timecode"], "polls": snap["polls"]},
            "rig": snap["devices"],
            "readiness": snap["readiness"],
            "waiting_for_a_person": snap["proposals"],
            "attention": attention,
            "you_may": acts,
            "rules": RULES,
            "how_to": {
                "edit": "GET /state for the definition (the 'definition' field), change a copy, POST it to /rehearse, then POST it to /show?said=<why>. As an agent that creates a proposal.",
                "mcp": "the same tools are offered by pchi-mcp: pchi_describe, pchi_state, pchi_show, pchi_doors_check, pchi_rehearse, pchi_propose, pchi_propose_cue_change, pchi_proposals, pchi_report, pchi_memory_recall, pchi_memory_remember, pchi_stop_lasers",
                "docs": "docs/mcp.md, docs/roles.md, docs/rehearse.md"
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pchi_eventstore::EventStore;
    use pchi_showchart::Showchart;

    fn show() -> std::sync::Arc<Show> {
        let chart = Showchart::from_json(r#"{"initial":"A","states":[{"id":"A","min_ms":60000,"timeline":{"lanes":[{"id":"l","dept":"lighting","cues":[{"id":"c1","at_ms":100,"action":{"kind":"lighting.look","payload":{"look":"x"}}}]}]}},{"id":"B"},{"id":"C"}],
            "guards":{"hype":"energy > 0.5"},
            "transitions":[{"source":"A","target":"B","event":"NEXT"},{"source":"A","target":"C","event":"BIG","guard":"hype"},{"source":"A","target":"B","after_ms":90000}]}"#).unwrap();
        Show::start(chart, EventStore::open_in_memory().unwrap()).unwrap()
    }

    fn outcome(d: &Value, id: &str) -> String {
        d["you_may"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap()["outcome"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn an_agent_is_told_it_may_propose_and_may_not_press_go() {
        let s = show();
        let d = s.describe(&Actor::Agent("nova".into()));
        assert_eq!(d["schema"], SCHEMA);
        assert_eq!(d["you"]["is_agent"], true);
        assert_eq!(outcome(&d, "propose_edit"), "allowed_as_proposal");
        for refused in ["press_go", "approve_proposal", "reject_proposal", "undo_send", "arm_laser", "try_cue", "test_fire"] {
            assert_eq!(outcome(&d, refused), "refused", "{refused}");
        }
        for ok in ["describe", "read_state", "rehearse", "stop_lasers", "report_ready", "ask_memory"] {
            assert_eq!(outcome(&d, ok), "allowed", "{ok}");
        }
        assert_eq!(outcome(&d, "write_memory"), "allowed_as_proposal", "an agent's memory is a draft");
    }

    #[tokio::test]
    async fn a_person_with_the_key_is_told_it_is_theirs() {
        let s = show();
        let d = s.describe(&Actor::Person(Some("Alex".into())));
        assert_eq!(outcome(&d, "press_go"), "allowed");
        assert_eq!(outcome(&d, "propose_edit"), "allowed");
        assert_eq!(outcome(&d, "arm_laser"), "allowed");
        assert_eq!(d["you"]["is_agent"], false);
    }

    #[tokio::test]
    async fn it_says_which_ways_out_would_fire_and_that_the_scene_is_held() {
        let s = show();
        let d = s.describe(&Actor::Agent("nova".into()));
        assert_eq!(d["now"]["scene"], "A");
        let ways = d["now"]["ways_out"].as_array().unwrap();
        let by = |to: &str, ev: Option<&str>| ways.iter().find(|w| w["to"] == to && w["event"].as_str() == ev).unwrap().clone();
        assert_eq!(by("B", Some("NEXT"))["would_fire_now"], true);
        assert_eq!(by("C", Some("BIG"))["would_fire_now"], false, "the guard does not hold");
        assert_eq!(by("B", None)["would_fire_now"], Value::Null, "a timer fires by itself");
        assert!(d["now"]["held_for_ms"].as_i64().unwrap() > 50_000);
        assert!(d["attention"].as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("must run")));
        assert!(d["show"]["events"].as_array().unwrap().contains(&json!("NEXT")));
        assert_eq!(d["show"]["scenes"][0]["cues"], 1);
        assert_eq!(d["show"]["scenes"][0]["departments"], json!(["lighting"]));
    }

    #[tokio::test]
    async fn it_notices_a_department_on_hold() {
        let s = show();
        s.set_ready("laser", false, "beam path check", "test").unwrap();
        let d = s.describe(&Actor::Agent("nova".into()));
        assert!(d["attention"].as_array().unwrap().iter().any(|a| a["text"].as_str().unwrap().contains("laser is on hold: beam path check")));
    }

    #[test]
    fn the_rules_name_the_things_an_agent_must_not_do() {
        let all = RULES.join(" ");
        for must in ["Rehearse", "Say why", "refusal is final", "GO", "STOP LASERS", "does not know", "UDP"] {
            assert!(all.contains(must), "{must}");
        }
    }
}
