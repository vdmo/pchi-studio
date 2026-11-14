//! The show-day safety drill: before the doors, check that PCHI behaves safely **with this show**.
//!
//! Each drill builds a fresh copy of the show in memory, sends nothing, provokes one kind of trouble and checks what PCHI does:
//! a laser that must stay held until armed, an agent that must not act, a press that comes too early, a device that cannot be
//! reached, a restart in the middle of a show, a send that must be undoable, timecode that stops. The result is a report with
//! what held and what did not, tied to the exact show by a hash, and optionally a signed receipt.
//!
//! What the drill is, and is not: it tests PCHI's own behaviour on the show's devices and scenes. It does NOT test the venue's
//! network, the tools, the fixtures or the people. Those are the human questions it asks at the end, and `pchi-gearcheck`.

use crate::gearcheck::Console;
use crate::governance::{Actor, Perm, Roster};
use crate::output::Output;
use crate::rig::Device;
use crate::timecode::Fps;
use crate::{Applied, Show};
use pchi_eventstore::EventStore;
use pchi_showchart::Showchart;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Drill {
    pub name: String,
    /// What it proves, in a sentence.
    pub proves: String,
    /// "held", "failed" or "skipped".
    pub outcome: String,
    /// What was seen, or why it was skipped.
    pub detail: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Human {
    pub item: String,
    /// "yes", "no" or "not asked".
    pub answer: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Report {
    pub show: String,
    pub show_hash: String,
    pub date: String,
    pub drills: Vec<Drill>,
    pub human: Vec<Human>,
}

impl Report {
    pub fn held(&self) -> usize {
        self.drills.iter().filter(|d| d.outcome == "held").count()
    }
    pub fn failed(&self) -> usize {
        self.drills.iter().filter(|d| d.outcome == "failed").count()
    }
    pub fn skipped(&self) -> usize {
        self.drills.iter().filter(|d| d.outcome == "skipped").count()
    }
    /// Safe to go: nothing failed, and no human item was answered no.
    pub fn go(&self) -> bool {
        self.failed() == 0 && !self.human.iter().any(|h| h.answer == "no")
    }
    pub fn verdict(&self) -> String {
        let asked: Vec<&Human> = self.human.iter().filter(|h| h.answer != "not asked").collect();
        let confirmed = asked.iter().filter(|h| h.answer == "yes").count();
        format!(
            "{}: {} held, {} failed, {} skipped; people: {}",
            if self.go() { "GO" } else { "NO GO" },
            self.held(),
            self.failed(),
            self.skipped(),
            if asked.is_empty() { "not asked".to_string() } else { format!("{confirmed} of {} confirmed", asked.len()) }
        )
    }
}

fn drill(name: &str, proves: &str, result: Result<String, String>) -> Drill {
    match result {
        Ok(d) => Drill { name: name.into(), proves: proves.into(), outcome: "held".into(), detail: d },
        Err(d) => Drill { name: name.into(), proves: proves.into(), outcome: "failed".into(), detail: d },
    }
}

fn skipped(name: &str, proves: &str, why: &str) -> Drill {
    Drill { name: name.into(), proves: proves.into(), outcome: "skipped".into(), detail: why.into() }
}

/// Which way each drill went: Ok with what was seen, Err with what went wrong, Skip with why it does not apply.
enum R {
    Done(Result<String, String>),
    Skip(String),
}

fn fresh(chart: &Showchart, devices: &[Device], send: bool, store: Option<EventStore>) -> Result<Arc<Show>, String> {
    let out = (!devices.is_empty()).then(|| Output::new(devices.to_vec(), send));
    let store = match store {
        Some(s) => s,
        None => EventStore::open_in_memory().map_err(|e| e.to_string())?,
    };
    Show::start_with(chart.clone(), store, out)
}

/// The events (and the scene each leads to) that move the show from its first scene to `target`, by presses only.
fn path_to(chart: &Showchart, target: &str) -> Option<Vec<String>> {
    let mut seen = vec![chart.initial.clone()];
    let mut queue = vec![(chart.initial.clone(), Vec::<String>::new())];
    while !queue.is_empty() {
        let (scene, path) = queue.remove(0);
        if scene == target {
            return Some(path);
        }
        for t in chart.transitions.iter().filter(|t| t.source == scene && t.guard.is_none()) {
            if let Some(ev) = t.event.as_deref().filter(|e| !e.starts_with("tc:")) {
                if !seen.contains(&t.target) {
                    seen.push(t.target.clone());
                    let mut p = path.clone();
                    p.push(ev.to_string());
                    queue.push((t.target.clone(), p));
                }
            }
        }
    }
    None
}

/// Walk the show to `target` as the show's own clock would (so no minimum holds the way), without sending anything.
fn go_to(show: &Arc<Show>, chart: &Showchart, target: &str) -> Result<(), String> {
    for ev in path_to(chart, target).ok_or_else(|| format!("no way to {target} by presses alone"))? {
        if show.apply_checked("timer", &ev) != Applied::Moved {
            return Err(format!("'{ev}' did not move the show on the way to {target}"));
        }
    }
    Ok(())
}

/// An edit that changes something harmless: the first cue, one second later.
fn nudged(chart: &Showchart) -> Option<Showchart> {
    let mut c = chart.clone();
    let cue = c.states.iter_mut().filter_map(|s| s.timeline.as_mut()).flat_map(|t| t.lanes.iter_mut()).flat_map(|l| l.cues.iter_mut()).next()?;
    cue.at_ms += 1_000;
    Some(c)
}

fn laser_of(devices: &[Device]) -> Option<&Device> {
    devices.iter().find(|d| d.dept == "laser")
}

async fn lasers_held(chart: &Showchart, devices: &[Device]) -> R {
    let Some(l) = laser_of(devices) else { return R::Skip("this show has no laser device".into()) };
    let r = async {
        let show = fresh(chart, devices, false, None)?;
        let status = |v: Result<Value, String>| v.map(|m| m["status"].as_str().unwrap_or("?").to_string());
        let s1 = status(show.test_fire(&l.id).await)?;
        if s1 != "held" {
            return Err(format!("a disarmed laser was not held: its test came back '{s1}'"));
        }
        show.set_armed(&l.id, true)?;
        let s2 = status(show.test_fire(&l.id).await)?;
        if s2 == "held" {
            return Err("an armed laser was still held".to_string());
        }
        let stopped = show.laser_stop();
        if !stopped["stopped"].as_array().is_some_and(|a| a.iter().any(|x| x == &json!(l.id))) {
            return Err(format!("STOP LASERS did not stop {}: {stopped}", l.label));
        }
        let s3 = status(show.test_fire(&l.id).await)?;
        if s3 != "held" {
            return Err(format!("after STOP LASERS the laser was not held again: '{s3}'"));
        }
        Ok(format!("{} was held until armed, passed cues while armed, and was held again after STOP LASERS", l.label))
    }
    .await;
    R::Done(r)
}

fn who_may(_: &Show) {}

fn people_and_agents(chart: &Showchart, devices: &[Device]) -> R {
    let r = (|| {
        let show = fresh(chart, devices, false, None)?;
        who_may(&show);
        show.set_edit_key(Some("drill-key".into()));
        let anon = show.identify(None, Actor::Person(None));
        let holder = show.identify(Some("drill-key"), Actor::Person(None));
        let agent = show.identify(Some("drill-key"), Actor::Agent("drill".into()));
        let mut bad: Vec<String> = Vec::new();
        for (who, label, perm, want) in [
            (&anon, "someone with no key", Perm::Edit, false),
            (&anon, "someone with no key", Perm::Arm, false),
            (&holder, "the key holder", Perm::Edit, true),
            (&holder, "the key holder", Perm::Arm, true),
            (&agent, "an agent, even holding the key", Perm::Edit, false),
            (&agent, "an agent, even holding the key", Perm::Arm, false),
            (&agent, "an agent, even holding the key", Perm::Go, false),
        ] {
            if show.allow(who, perm, Some("laser")) != want {
                bad.push(format!("{label} {} {perm:?}", if want { "could not" } else { "could" }));
            }
        }
        // roles: a guest looks, the laser safety officer arms lasers and nothing else
        show.set_roster(Roster::parse(r#"[{"name":"G","role":"guest","key":"guest-key-1"},{"name":"S","role":"lso","key":"lso-key-123"}]"#)?);
        let guest = show.identify(Some("guest-key-1"), Actor::Person(None));
        let lso = show.identify(Some("lso-key-123"), Actor::Person(None));
        for (who, label, perm, dept, want) in [
            (&guest, "a guest", Perm::Go, None, false),
            (&guest, "a guest", Perm::Arm, None, false),
            (&lso, "the laser safety officer", Perm::Arm, None, true),
            (&lso, "the laser safety officer", Perm::Edit, None, false),
            (&lso, "the laser safety officer", Perm::Try, Some("lighting"), false),
        ] {
            if show.allow(who, perm, dept) != want {
                bad.push(format!("{label} {} {perm:?}", if want { "could not" } else { "could" }));
            }
        }
        if bad.is_empty() { Ok("no key, an agent and a guest were refused what only an operator may do; the key holder and the laser safety officer were allowed theirs".to_string()) } else { Err(bad.join("; ")) }
    })();
    R::Done(r)
}

fn agent_proposals(chart: &Showchart, devices: &[Device]) -> R {
    let Some(edited) = nudged(chart) else { return R::Skip("this show has no cues to edit".into()) };
    let r = (|| {
        let show = fresh(chart, devices, false, None)?;
        let before = show.snapshot()["definition"].clone();
        let p = show.propose(&Actor::Agent("drill".into()), edited.clone(), None, "safety drill").map_err(|e| format!("a proposal was refused: {}", e.1))?;
        if p["proposed"] != json!(true) || show.snapshot()["definition"] != before {
            return Err("an agent's edit changed the running show instead of waiting for a person".into());
        }
        let a = show.approve(1, &Actor::Person(None)).map_err(|e| format!("approving failed: {}", e.1))?;
        if a["applied"] != json!(true) || show.snapshot()["definition"] != json!(edited) {
            return Err("approving the proposal did not apply it".into());
        }
        Ok("an agent's edit waited as a proposal and changed the show only when a person approved it".to_string())
    })();
    R::Done(r)
}

fn minimum_holds(chart: &Showchart, devices: &[Device]) -> R {
    // a scene with a minimum that can be left by a plain press
    let found = chart.states.iter().find_map(|s| {
        let min = s.min_ms?;
        let ev = chart.transitions.iter().find(|t| t.source == s.id && t.guard.is_none() && t.event.as_deref().is_some_and(|e| !e.starts_with("tc:")))?.event.clone()?;
        path_to(chart, &s.id).map(|_| (s.id.clone(), min, ev))
    });
    let Some((scene, min, ev)) = found else { return R::Skip("no scene with a minimum run time that a press can leave".into()) };
    let r = (|| {
        let show = fresh(chart, devices, false, None)?;
        go_to(&show, chart, &scene)?;
        match show.apply_checked("drill", &ev) {
            Applied::Held { .. } if show.snapshot()["state"] == json!(scene) => Ok(format!("a press of '{ev}' straight after entering {scene} was held (it must run {:.1} s)", min as f64 / 1000.0)),
            other => Err(format!("a press of '{ev}' in {scene} straight after entering it was not held: {other:?}")),
        }
    })();
    R::Done(r)
}

async fn device_down(chart: &Showchart, devices: &[Device]) -> R {
    let Some(i) = devices.iter().position(|d| d.dept != "laser" && !(d.kind == "sacn" && d.host.as_deref().map_or(true, str::is_empty))) else { return R::Skip("no device with an address to take away".into()) };
    let mut broken = devices.to_vec();
    broken[i].host = Some("no-such-host.invalid".into());
    let label = broken[i].label.clone();
    let id = broken[i].id.clone();
    let r = async {
        let show = fresh(chart, &broken, true, None)?;
        let m = show.test_fire(&id).await?;
        if m["status"] != json!("failed") {
            return Err(format!("{label} could not be reached but its delivery said '{}'", m["status"]));
        }
        let first = chart.transitions.iter().find(|t| t.source == chart.initial && t.event.is_some() && t.guard.is_none()).and_then(|t| t.event.clone());
        match first {
            Some(ev) if !ev.starts_with("tc:") => {
                if show.apply("drill", &ev) {
                    Ok(format!("{label} was unreachable: its delivery was reported failed and the show still moved on at '{ev}'"))
                } else {
                    Err(format!("with {label} unreachable the show would not move on at '{ev}'"))
                }
            }
            _ => Ok(format!("{label} was unreachable: its delivery was reported failed and the show kept answering")),
        }
    }
    .await;
    R::Done(r)
}

fn restart_resumes(chart: &Showchart, devices: &[Device]) -> R {
    let path = std::env::temp_dir().join(format!("pchi-drill-{}-{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let r = (|| {
        let store = EventStore::open(path.to_str().ok_or("temp path")?).map_err(|e| e.to_string())?;
        let show = fresh(chart, devices, false, Some(store))?;
        let ev = chart.transitions.iter().find(|t| t.source == chart.initial && t.event.is_some() && t.guard.is_none() && !t.event.as_deref().unwrap_or("").starts_with("tc:")).and_then(|t| t.event.clone());
        if let Some(ev) = &ev {
            show.apply_checked("timer", ev);
        }
        let laser = laser_of(devices);
        if let Some(l) = laser {
            show.set_armed(&l.id, true)?;
        }
        let state = show.snapshot()["state"].clone();
        let store2 = EventStore::open(path.to_str().ok_or("temp path")?).map_err(|e| e.to_string())?;
        let out = (!devices.is_empty()).then(|| Output::new(devices.to_vec(), false));
        let (again, info) = Show::resume(chart.clone(), store2, out)?;
        if again.snapshot()["state"] != state {
            return Err(format!("the show was in {state} and resumed in {}", again.snapshot()["state"]));
        }
        if let Some(l) = laser {
            if again.snapshot()["devices"].as_array().is_some_and(|d| d.iter().any(|x| x["id"] == json!(l.id) && x["armed"] == json!(true))) {
                return Err(format!("{} was armed again after a restart", l.label));
            }
        }
        if !again.was_resumed() {
            return Err("the restarted runtime did not say it had resumed".into());
        }
        Ok(format!("after a restart the show came back in {state} ({} cue(s) that came due while it was down skipped){}", info["skipped_cues"].as_array().map_or(0, |a| a.len()), if laser.is_some() { ", and the laser was disarmed" } else { "" }))
    })();
    let _ = std::fs::remove_file(&path);
    R::Done(r)
}

fn undo_restores(chart: &Showchart, devices: &[Device]) -> R {
    let Some(edited) = nudged(chart) else { return R::Skip("this show has no cues to edit".into()) };
    let r = (|| {
        let show = fresh(chart, devices, false, None)?;
        let before = show.snapshot()["definition"].clone();
        show.replace(edited.clone(), None, true).map_err(|e| format!("the edit was refused: {}", e.1))?;
        if show.snapshot()["definition"] == before {
            return Err("sending an edit changed nothing".into());
        }
        show.undo(true).map_err(|e| format!("undo was refused: {}", e.1))?;
        if show.snapshot()["definition"] != before {
            return Err("undo did not put the show back as it was".into());
        }
        Ok("an edit sent to the running show was undone and the show was exactly as before".to_string())
    })();
    R::Done(r)
}

async fn timecode_stops(chart: &Showchart, devices: &[Device]) -> R {
    let found = chart.states.iter().find_map(|s| s.timeline.as_ref()?.tc_start.clone().map(|t| (s.id.clone(), t)));
    let Some((scene, tc)) = found else { return R::Skip("this show does not lock anything to timecode".into()) };
    let r = async {
        let show = fresh(chart, devices, false, None)?;
        go_to(&show, chart, &scene)?;
        let start = crate::timecode::written_to_ms(&tc, Fps::F25)?;
        let mut rx = show.subscribe();
        for step in 0..4 {
            show.timecode_update(start - 80.0 + 40.0 * step as f64, Fps::F25, "drill");
        }
        if show.timecode_json()["state"] != json!("running") {
            return Err("timecode that was playing did not show as running".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if show.timecode_json()["state"] != json!("stopped") {
            return Err("timecode that stopped did not show as stopped".into());
        }
        let warned = show.preflight()["checks"].as_array().is_some_and(|c| c.iter().any(|x| x["area"] == "timecode" && x["level"] == "warn"));
        if !warned {
            return Err("the Doors check did not warn that the timecode had stopped".into());
        }
        while rx.try_recv().is_ok() {}
        show.timecode_update(start + 60_000.0, Fps::F25, "drill"); // a locate far ahead
        let fired = std::iter::from_fn(|| rx.try_recv().ok()).any(|m| m["type"] == "action");
        if fired {
            return Err("a jump in the timecode fired cues that were jumped over".into());
        }
        Ok(format!("{scene} is locked to {tc}: timecode that stopped showed as stopped and the Doors check warned; a jump of a minute fired nothing"))
    }
    .await;
    R::Done(r)
}

fn doors(chart: &Showchart, devices: &[Device]) -> R {
    if devices.is_empty() {
        return R::Skip("this show has no rig, so there is nothing to check it against".into());
    }
    let r = (|| {
        let show = fresh(chart, devices, false, None)?;
        let pf = show.preflight();
        let fails: Vec<String> = pf["checks"].as_array().into_iter().flatten().filter(|c| c["level"] == "fail").map(|c| c["text"].as_str().unwrap_or("").to_string()).collect();
        if fails.is_empty() {
            Ok(format!("the Doors check found nothing that will not work ({} to look at)", pf["summary"]["warn"]))
        } else {
            Err(format!("{} thing(s) will not work: {}", fails.len(), fails.iter().take(4).cloned().collect::<Vec<_>>().join(" | ")))
        }
    })();
    R::Done(r)
}

/// What only people can answer, from what the show contains.
fn human_items(chart: &Showchart, devices: &[Device]) -> Vec<String> {
    let mut v = vec!["Someone other than the operator knows how to take over: where STOP LASERS is, and how to run the show from the tools' own surfaces if PCHI stops.".to_string()];
    if !devices.is_empty() {
        v.push("Every device's cable and switch is checked, and pchi-gearcheck has been run on each tool and fixture the show depends on (an unticked line in foh-proof.md is an unproven claim).".into());
    }
    if devices.iter().any(|d| d.dept == "laser") {
        v.push("A laser safety officer is present. The beam path is clear, and STOP LASERS has been pressed with the laser live and the laser software's own output went dark.".into());
    }
    if devices.iter().any(|d| d.dept == "audio") {
        v.push("The PA is muted for tests, and a scene or snapshot recall has been tried on the real desk.".into());
    }
    if chart.states.iter().any(|s| s.timeline.as_ref().is_some_and(|t| t.tc_start.is_some())) || chart.transitions.iter().any(|t| t.event.as_deref().is_some_and(|e| e.starts_with("tc:"))) {
        v.push("The playback machine's timecode has been seen running on the Live tab, at the frame rate the show expects.".into());
    }
    if devices.iter().any(|d| d.is_dmx()) {
        v.push("The DMX lines have been seen to light the right fixtures, and nothing strobes or moves unexpectedly.".into());
    }
    v
}

/// Run every drill that applies to this show, then (if `console` is given) ask the human questions.
pub async fn run(chart: &Showchart, devices: &[Device], date: &str, console: Option<&mut dyn Console>) -> Report {
    let mut drills = Vec::new();
    let mut push = |name: &str, proves: &str, r: R| match r {
        R::Done(res) => drills.push(drill(name, proves, res)),
        R::Skip(why) => drills.push(skipped(name, proves, &why)),
    };
    push("Doors check", "nothing in the show will fail against the rig as it stands", doors(chart, devices));
    push("Lasers stay held", "a laser gets nothing until armed, and STOP LASERS makes it held again", lasers_held(chart, devices).await);
    push("Keys, roles and agents", "no key, an agent or a guest cannot do what only an operator may", people_and_agents(chart, devices));
    push("Agents propose", "an agent's edit waits for a person and changes the show only when approved", agent_proposals(chart, devices));
    push("A press that comes too early", "a scene's minimum run time holds a press that would leave it", minimum_holds(chart, devices));
    push("A device goes away", "an unreachable device is reported failed and the show carries on", device_down(chart, devices).await);
    push("A restart in the middle", "the show resumes in the same scene and lasers come back disarmed", restart_resumes(chart, devices));
    push("Undo a send", "an edit sent to the running show can be taken back exactly", undo_restores(chart, devices));
    push("Timecode stops and jumps", "stopped timecode is noticed, and a locate fires nothing", timecode_stops(chart, devices).await);
    let items = human_items(chart, devices);
    let human = match console {
        Some(c) => {
            c.say("\nNow the questions only people can answer. y = yes, n = no, anything else = not asked.");
            items.into_iter().map(|item| {
                let a = c.ask(&format!("\n{item}\nDone? (y/n):")).trim().to_ascii_lowercase();
                Human { item, answer: match a.as_str() { "y" | "yes" => "yes", "n" | "no" => "no", _ => "not asked" }.into() }
            }).collect()
        }
        None => items.into_iter().map(|item| Human { item, answer: "not asked".into() }).collect(),
    };
    let show = chart.name.clone().or_else(|| chart.id.clone()).unwrap_or_else(|| "show".into());
    Report { show, show_hash: primeswarm_pchi::governance::hash_state(&json!({"show": chart, "rig": devices})), date: date.into(), drills, human }
}

pub fn markdown(r: &Report) -> String {
    let mut s = format!("# Safety drill: {} ({})\n\n**{}**\n\nShow and rig hash: `{}`\n\n", r.show, r.date, r.verdict(), r.show_hash);
    s += "This tests PCHI's own behaviour on this show, with nothing sent. It does not test the venue's network, the tools, the fixtures or the people.\n\n## What PCHI did\n\n";
    for d in &r.drills {
        let mark = match d.outcome.as_str() { "held" => "HELD", "failed" => "**FAILED**", _ => "skipped" };
        s += &format!("- **{}**: {mark}. {} ({})\n", d.name, d.detail, d.proves);
    }
    s += "\n## What only people can say\n\n";
    for h in &r.human {
        s += &format!("- [{}] {}\n", match h.answer.as_str() { "yes" => "x", "no" => "NO", _ => " " }, h.item);
    }
    s
}

/// Write the result as a signed receipt (same chain as the runtime's), tied to the show by its hash. Returns the receipt id.
pub fn sign(r: &Report, log: &primeswarm_pchi::GovernanceLog) -> String {
    let gate = if r.go() { "ALLOW" } else { "DENY" };
    log.record("drill.run", gate, &format!("safety drill of {} on {}: {}", r.show, r.date, r.verdict()), &r.show_hash).receipt_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gearcheck::Console;
    use std::collections::VecDeque;

    struct Script(VecDeque<String>, Vec<String>);
    impl Console for Script {
        fn say(&mut self, t: &str) {
            self.1.push(t.into());
        }
        fn ask(&mut self, p: &str) -> String {
            self.1.push(p.into());
            self.0.pop_front().unwrap_or_default()
        }
    }

    fn dev(id: &str, kind: &str, host: Option<&str>) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: crate::rig::dept_of_type(kind).unwrap().into(), host: host.map(String::from), port: Some(9), listen_port: None, profile: None, universe: None, patch: None, password: None }
    }

    fn rig() -> Vec<Device> {
        vec![dev("laser", "beyond", Some("127.0.0.1")), dev("desk", "eos", Some("127.0.0.1")), dev("media", "resolume", Some("127.0.0.1"))]
    }

    /// The example show with values the rig's tools actually understand (the example's own are placeholders like "intro").
    fn show_chart() -> Showchart {
        let mut c = Showchart::from_json(include_str!("../../pchi-showchart/examples/timelines.pchi.json")).unwrap();
        let fix = |a: &mut pchi_showchart::Action| match a.kind.as_str() {
            "scene.activate" => a.payload = json!({"scene": "1"}),
            "lighting.look" => a.payload = json!({"look": "3"}),
            _ => {}
        };
        for s in c.states.iter_mut() {
            s.enter_actions.iter_mut().chain(s.exit_actions.iter_mut()).for_each(fix);
            for l in s.timeline.iter_mut().flat_map(|t| t.lanes.iter_mut()) {
                l.cues.iter_mut().for_each(|q| fix(&mut q.action));
            }
        }
        c
    }

    /// The example with a minimum on INTRO and INTRO locked to timecode.
    fn full_chart() -> Showchart {
        let mut c = show_chart();
        let intro = c.states.iter_mut().find(|s| s.id == "INTRO").unwrap();
        intro.min_ms = Some(5_000);
        intro.timeline.as_mut().unwrap().tc_start = Some("00:01:00:00".into());
        c
    }

    fn by<'a>(r: &'a Report, name: &str) -> &'a Drill {
        r.drills.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name}"))
    }

    #[tokio::test]
    async fn every_drill_that_applies_holds_on_a_plain_show() {
        let r = run(&show_chart(), &rig(), "2026-11-14", None).await;
        for d in &r.drills {
            assert_ne!(d.outcome, "failed", "{}: {}", d.name, d.detail);
        }
        for name in ["Lasers stay held", "Keys, roles and agents", "Agents propose", "A device goes away", "A restart in the middle", "Undo a send"] {
            assert_eq!(by(&r, name).outcome, "held", "{name}: {}", by(&r, name).detail);
        }
        // this show has no minimum and no timecode, so those do not apply and say why
        assert_eq!(by(&r, "A press that comes too early").outcome, "skipped");
        assert_eq!(by(&r, "Timecode stops and jumps").outcome, "skipped");
        assert!(by(&r, "A restart in the middle").detail.contains("the laser was disarmed"));
    }

    #[tokio::test]
    async fn a_show_with_a_minimum_and_timecode_gets_those_drills_too() {
        let r = run(&full_chart(), &rig(), "2026-11-14", None).await;
        let min = by(&r, "A press that comes too early");
        assert_eq!(min.outcome, "held", "{}", min.detail);
        assert!(min.detail.contains("INTRO") && min.detail.contains("5.0 s"), "{}", min.detail);
        let tc = by(&r, "Timecode stops and jumps");
        assert_eq!(tc.outcome, "held", "{}", tc.detail);
        assert!(tc.detail.contains("00:01:00:00"));
    }

    #[tokio::test]
    async fn a_show_that_will_not_work_is_a_no_go_and_the_doors_check_says_why() {
        // an Eos cue value no console understands, and no device for the visual lane
        let mut c = show_chart();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "lighting").unwrap().cues[0].action.payload = json!({"look": "warm"});
        let r = run(&c, &[dev("desk", "eos", Some("127.0.0.1"))], "2026-11-14", None).await;
        let d = by(&r, "Doors check");
        assert_eq!(d.outcome, "failed");
        assert!(d.detail.contains("will not work"), "{}", d.detail);
        assert!(!r.go());
        assert!(r.verdict().starts_with("NO GO"), "{}", r.verdict());
        // no rig: most things do not apply, and the report says so rather than passing silently
        let r = run(&show_chart(), &[], "2026-11-14", None).await;
        assert_eq!(by(&r, "Doors check").outcome, "skipped");
        assert_eq!(by(&r, "Lasers stay held").outcome, "skipped");
        assert!(r.verdict().starts_with("GO"), "nothing failed: {}", r.verdict());
    }

    #[tokio::test]
    async fn the_people_are_asked_what_the_show_contains_and_a_no_is_a_no_go() {
        let chart = show_chart();
        let items = human_items(&chart, &rig());
        assert!(items.iter().any(|i| i.contains("laser safety officer")) && items.iter().any(|i| i.contains("gearcheck")), "{items:?}");
        assert!(!items.iter().any(|i| i.contains("PA is muted")), "no audio device, so no PA question");
        let mut all_yes = Script(VecDeque::from(vec!["y".to_string(); 10]), vec![]);
        let r = run(&chart, &rig(), "2026-11-14", Some(&mut all_yes)).await;
        assert!(r.human.iter().all(|h| h.answer == "yes") && r.go(), "{}", r.verdict());
        assert!(r.verdict().contains(&format!("{} of {} confirmed", r.human.len(), r.human.len())));
        let mut one_no = Script(VecDeque::from(vec!["y".to_string(), "n".to_string()]), vec![]);
        let r = run(&chart, &rig(), "2026-11-14", Some(&mut one_no)).await;
        assert!(!r.go(), "a person said no: {}", r.verdict());
        assert_eq!(r.human[1].answer, "no");
    }

    #[tokio::test]
    async fn the_report_is_tied_to_the_exact_show_and_can_be_signed() {
        let a = run(&show_chart(), &rig(), "2026-11-14", None).await;
        let b = run(&show_chart(), &rig(), "2026-11-14", None).await;
        assert_eq!(a.show_hash, b.show_hash, "the same show and rig give the same hash");
        let c = run(&show_chart(), &rig()[..2], "2026-11-14", None).await;
        assert_ne!(a.show_hash, c.show_hash, "a different rig is a different hash");
        let md = markdown(&a);
        assert!(md.contains("# Safety drill") && md.contains("What PCHI did") && md.contains("does not test the venue's network"), "{md}");
        let dir = std::env::temp_dir().join(format!("pchi-drill-sign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = primeswarm_pchi::GovernanceLog::open(dir.join("k.hex").to_str().unwrap(), dir.join("l.jsonl").to_str().unwrap()).unwrap();
        let id = sign(&a, &log);
        let ex = log.export();
        let rec: primeswarm_pchi::governance::GovernanceReceipt = serde_json::from_value(ex["receipts"][0].clone()).unwrap();
        assert_eq!(rec.receipt_id, id);
        assert_eq!((rec.rule_name.as_str(), rec.gate_state.as_str(), rec.state_snapshot_hash.as_str()), ("drill.run", "ALLOW", a.show_hash.as_str()));
        assert!(primeswarm_pchi::governance::verify_receipt(&rec, ex["verifying_key"].as_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
