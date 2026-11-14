//! Rehearse an edited show against a run that already happened: no gear, no waiting.
//!
//! The recorded run gives the real timeline of what people and sensors did: when GO was pressed, which context
//! values arrived. The candidate show is played against exactly that, on a virtual clock, with its own timers and
//! cues. The answer is in plain words: does it take the same path, which cues would fire, which would be cut off
//! because the scene ends first, what would the tools receive, and where it differs from what really ran.
//!
//! What it cannot know: how the people would have reacted to a different show (their presses are replayed as
//! they were), and anything about the tools themselves. A cue due at the very moment its scene ends counts as cut off.

use crate::{output, rig};
use pchi_eventstore::{recorder, EventRecord};
use pchi_showchart::{timer_event, Action, ShowEngine, Showchart};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MAX_STEPS: usize = 5_000;

struct Fired {
    cue: String,
    lane: String,
    label: String,
    kind: String,
    at_ms: u64,
    sent_as: Option<Vec<Value>>,
}

struct Visit {
    scene: String,
    entered: u64,
    left: Option<u64>,
    via: String,
    fired: Vec<Fired>,
    cut_off: Vec<(String, u64)>,
}

fn secs(ms: u64) -> String {
    let s = ms as f64 / 1000.0;
    if s >= 60.0 { format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0) } else { format!("{s:.1}s") }
}

fn label_of(chart: &Showchart, scene: &str, lane: &str, cue: &str) -> String {
    chart
        .state(scene)
        .and_then(|s| s.timeline.as_ref())
        .and_then(|t| t.lanes.iter().find(|l| l.id == lane))
        .and_then(|l| l.cues.iter().find(|c| c.id == cue))
        .and_then(|c| c.label.clone())
        .unwrap_or_else(|| cue.to_string())
}

/// What a cue would put on the wire, or why it cannot be sent: only when the candidate has a rig.
fn wire(devices: &[rig::Device], dept: Option<&str>, device: Option<&str>, action: &Action) -> Result<Option<Vec<Value>>, String> {
    if devices.is_empty() {
        return Ok(None);
    }
    let dept = dept.map(String::from).unwrap_or_else(|| rig::dept_of_kind(&action.kind).to_string());
    if dept == "other" {
        return Ok(None);
    }
    let dev = rig::resolve(devices, &dept, device)?;
    Ok(Some(output::wire_lines(dev, action)?.into_iter().map(|(a, args)| json!({"device": dev.label, "address": a, "args": args})).collect()))
}

/// Play `chart` against the recorded run. `until` is where the run ends (now, for a live run).
pub fn run(records: &[EventRecord], chart: &Showchart, devices: &[rig::Device], until: i64) -> Result<Value, String> {
    let start_i = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no recorded run to rehearse against")?;
    let start = records[start_i].timestamp;
    let run = &records[start_i..];
    let end = until.max(run.last().map_or(start, |r| r.timestamp));
    let length = (end - start).max(0) as u64;

    // ---- what really happened ----
    let mut real: Vec<Visit> = Vec::new();
    let initial_real = run.iter().find(|r| r.event_type == recorder::SHOW_INPUT).map(|r| r.payload["from"].as_str().unwrap_or("").to_string());
    // the first recorded input says where the show began; without any input there is a single scene we cannot name
    let mut scene = initial_real.unwrap_or_else(|| chart.initial.clone());
    let mut entered = 0u64;
    let mut via = "start".to_string();
    let mut real_cues: Vec<(u64, &Value)> = Vec::new();
    let mut edits = 0;
    for r in run {
        let t = (r.timestamp - start).max(0) as u64;
        match r.event_type.as_str() {
            recorder::SHOW_INPUT if r.payload["applied"] == json!(true) => {
                real.push(Visit { scene: scene.clone(), entered, left: Some(t), via: via.clone(), fired: vec![], cut_off: vec![] });
                scene = r.payload["to"].as_str().unwrap_or("").to_string();
                entered = t;
                via = r.payload["event"].as_str().unwrap_or("").to_string();
            }
            "show.cue" => real_cues.push((t, &r.payload)),
            "show.edit" => edits += 1,
            _ => {}
        }
    }
    real.push(Visit { scene: scene.clone(), entered, left: None, via, fired: vec![], cut_off: vec![] });
    // the inputs people and sensors made (timers are the show's own, the candidate has its own)
    let mut externals: Vec<(u64, Option<(String, Value)>, Option<String>)> = Vec::new();
    for r in run {
        let t = (r.timestamp - start).max(0) as u64;
        match r.event_type.as_str() {
            recorder::SHOW_INPUT if r.source != "timer" => externals.push((t, None, Some(r.payload["event"].as_str().unwrap_or("").to_string()))),
            recorder::SHOW_CONTEXT => externals.push((t, Some((r.payload["key"].as_str().unwrap_or("").to_string(), r.payload["value"].clone())), None)),
            _ => {}
        }
    }

    // ---- the candidate, on a virtual clock ----
    let mut engine = ShowEngine::new(chart.clone()).map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
    let mut sim: Vec<Visit> = Vec::new();
    let mut divergences: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut cur = Visit { scene: engine.current_state.clone(), entered: 0, left: None, via: "start".into(), fired: vec![], cut_off: vec![] };
    let mut timers = engine.pending_timers();
    let mut ti = 0usize;
    let mut ei = 0usize;
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > MAX_STEPS {
            divergences.push("stopped after too many steps: a timer loop with no delay?".into());
            break;
        }
        let next_ext = externals.get(ei).map(|e| e.0);
        let next_timer = timers.get(ti).map(|ms| cur.entered + ms);
        let take_timer = match (next_ext, next_timer) {
            (None, None) => break,
            (Some(_), None) => false,
            (None, Some(tt)) => {
                if tt > length {
                    break;
                }
                true
            }
            (Some(e), Some(tt)) => tt < e,
        };
        let (t, applied_to_visit): (u64, Option<(String, String)>) = if take_timer {
            let ms = timers[ti];
            ti += 1;
            let t = cur.entered + ms;
            if t > length {
                break;
            }
            let _ = engine.try_dispatch(&timer_event(ms)).map(|_| ());
            (t, Some((timer_event(ms), "timer".into())))
        } else {
            let (t, ctx, ev) = externals[ei].clone();
            ei += 1;
            if let Some((k, v)) = ctx {
                engine.set_context(&k, v);
                continue;
            }
            let ev = ev.unwrap_or_default();
            let was = engine.current_state.clone();
            if let Some(min) = chart.state(&was).and_then(|s| s.min_ms) {
                if t - cur.entered < min && engine.clone().try_dispatch(&ev).is_some() {
                    divergences.push(format!("{}: the recorded input '{ev}' would be held: {was} must run {} and had run {}", secs(t), secs(min), secs(t - cur.entered)));
                    continue;
                }
            }
            let moved = engine.try_dispatch(&ev).is_some();
            if !moved {
                divergences.push(format!("{}: the recorded input '{ev}' would do nothing: this show is in {was}", secs(t)));
                continue;
            }
            (t, Some((ev, "input".into())))
        };
        let Some((event, _kind)) = applied_to_visit else { continue };
        if engine.current_state == cur.scene {
            continue; // a timer the guard blocked, or a no-op
        }
        // leave the visit: cues due before the exit fire, the rest are cut off
        finish(&mut cur, t, chart, devices, &mut problems);
        let next = Visit { scene: engine.current_state.clone(), entered: t, left: None, via: event, fired: vec![], cut_off: vec![] };
        sim.push(std::mem::replace(&mut cur, next));
        timers = engine.pending_timers();
        ti = 0;
    }
    finish(&mut cur, length, chart, devices, &mut problems);
    sim.push(cur);

    // ---- compare ----
    for v in real.iter_mut() {
        let end_v = v.left.unwrap_or(length);
        for (t, c) in &real_cues {
            if *t >= v.entered && (*t < end_v || (v.left.is_none() && *t <= end_v)) && c["scene"].as_str() == Some(v.scene.as_str()) {
                v.fired.push(Fired {
                    cue: c["cue"].as_str().unwrap_or("").into(),
                    lane: c["lane"].as_str().unwrap_or("").into(),
                    label: c["kind"].as_str().unwrap_or("").into(),
                    kind: c["kind"].as_str().unwrap_or("").into(),
                    at_ms: c["planned_ms"].as_u64().unwrap_or(0),
                    sent_as: None,
                });
            }
        }
    }
    let real_path: Vec<&str> = real.iter().map(|v| v.scene.as_str()).collect();
    let sim_path: Vec<&str> = sim.iter().map(|v| v.scene.as_str()).collect();
    let same_path = real_path == sim_path;
    let mut lines: Vec<String> = Vec::new();
    let mut visits_json: Vec<Value> = Vec::new();
    let mut diverged_at: Option<usize> = None;
    for (i, v) in sim.iter().enumerate() {
        let mut notes: Vec<String> = Vec::new();
        let open = i + 1 == sim.len(); // the scene the run ended in: cues not yet due are not "cut off"
        let r = real.get(i).filter(|r| r.scene == v.scene && diverged_at.is_none());
        if diverged_at.is_none() && r.is_none() {
            diverged_at = Some(i);
            let there = real.get(i).map(|r| r.scene.as_str()).unwrap_or("the show had ended");
            lines.push(format!("From {}: this show goes to {} where the real run went to {}", secs(v.entered), v.scene, there));
        }
        if let Some(r) = r {
            let (real_len, sim_len) = (r.left.unwrap_or(length) - r.entered, v.left.unwrap_or(length) - v.entered);
            if real_len.abs_diff(sim_len) >= 100 {
                notes.push(format!("{} lasts {} instead of {}", v.scene, secs(sim_len), secs(real_len)));
            }
            let ids = |x: &Visit| x.fired.iter().map(|f| (f.lane.clone(), f.cue.clone())).collect::<Vec<_>>();
            let had: BTreeMap<(String, String), u64> = r.fired.iter().map(|f| ((f.lane.clone(), f.cue.clone()), f.at_ms)).collect();
            for f in &v.fired {
                match had.get(&(f.lane.clone(), f.cue.clone())) {
                    None => notes.push(format!("+ \"{}\" would now fire at {} (it did not before)", f.label, secs(f.at_ms))),
                    Some(&was) if was != f.at_ms => notes.push(format!("~ \"{}\" moves from {} to {}", f.label, secs(was), secs(f.at_ms))),
                    _ => {}
                }
            }
            for ((lane, cue), at) in &had {
                if !ids(v).contains(&(lane.clone(), cue.clone())) {
                    let why = if let Some((_, due)) = v.cut_off.iter().find(|(c, _)| c == cue) {
                        if open {
                            format!("it is due at {}, later than the {} the run lasted", secs(*due), secs(sim_len))
                        } else {
                            format!("{} is left at {} before it is due at {}", v.scene, secs(sim_len), secs(*due))
                        }
                    } else {
                        "it is no longer in this scene".to_string()
                    };
                    notes.push(format!("- \"{}\" (was at {}) would NOT fire: {why}", label_of(chart, &v.scene, lane, cue), secs(*at)));
                }
            }
        }
        for (cue, due) in &v.cut_off {
            if !open && r.is_none() {
                notes.push(format!("cue {cue} due at {} never fires: {} ends at {}", secs(*due), v.scene, secs(v.left.unwrap_or(length) - v.entered)));
            }
        }
        for n in &notes {
            lines.push(format!("{}: {n}", v.scene));
        }
        visits_json.push(json!({
            "scene": v.scene, "entered_ms": v.entered, "duration_ms": v.left.unwrap_or(length) - v.entered, "via": v.via,
            "recorded_duration_ms": r.map(|r| r.left.unwrap_or(length) - r.entered),
            "fires": v.fired.iter().map(|f| json!({"cue": f.cue, "lane": f.lane, "label": f.label, "kind": f.kind, "at_ms": f.at_ms, "sent_as": f.sent_as})).collect::<Vec<_>>(),
            "cut_off": v.cut_off.iter().map(|(c, d)| json!({"cue": c, "due_ms": d})).collect::<Vec<_>>(),
            "changes": notes,
        }));
    }
    if sim.len() < real.len() && diverged_at.is_none() {
        lines.push(format!("This show ends in {} but the real run went on to {}", sim.last().map_or("?", |v| v.scene.as_str()), real_path[sim.len()..].join(" → ")));
    }
    lines.extend(divergences.iter().cloned());
    lines.extend(problems.iter().map(|p| format!("cannot be sent: {p}")));
    let different = !same_path || !lines.is_empty();
    Ok(json!({
        "run": {"started": start, "length_ms": length, "inputs": externals.len(), "edited_live": edits},
        "same_path": same_path,
        "recorded_path": real_path,
        "rehearsed_path": sim_path,
        "visits": visits_json,
        "problems": problems,
        "divergences": divergences,
        "summary": {"different": different, "lines": lines,
                    "verdict": if different { format!("{} difference(s) from the real run", lines.len()) } else { "no difference: same path, same cues, same timing".to_string() }},
    }))
}

/// Close a visit at `t`: cues due before then fire (and are checked against their device), later ones are cut off.
fn finish(v: &mut Visit, t: u64, chart: &Showchart, devices: &[rig::Device], problems: &mut Vec<String>) {
    let Ok(mut engine) = ShowEngine::new(chart.clone()) else { return };
    engine.current_state = v.scene.clone();
    let span = t - v.entered;
    v.left = Some(t);
    for c in engine.pending_cues() {
        if c.at_ms < span {
            let sent_as = match wire(devices, c.dept.as_deref(), c.device.as_deref(), &c.action) {
                Ok(s) => s,
                Err(e) => {
                    problems.push(format!("{} / {}: {e}", v.scene, label_of(chart, &v.scene, &c.lane, &c.cue)));
                    None
                }
            };
            v.fired.push(Fired { label: label_of(chart, &v.scene, &c.lane, &c.cue), cue: c.cue, lane: c.lane, kind: c.action.kind.clone(), at_ms: c.at_ms, sent_as });
        } else {
            v.cut_off.push((c.cue, c.at_ms));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_000_000;

    fn chart() -> Showchart {
        Showchart::from_json(include_str!("../../pchi-showchart/examples/timelines.pchi.json")).unwrap()
    }

    fn rec(id: i64, at: i64, source: &str, kind: &str, payload: Value) -> EventRecord {
        EventRecord { id, timestamp: T0 + at, source: source.into(), target: None, event_type: kind.into(), payload }
    }

    /// START at 1 s, INTRO runs 3 s (its 4 s laser cue never fires), DROP pressed at 4 s, run ends at 6 s.
    fn real_run() -> Vec<EventRecord> {
        let cue = |id: i64, at: i64, scene: &str, lane: &str, cue: &str, planned: u64| rec(id, at, "runtime", "show.cue", json!({"scene": scene, "lane": lane, "cue": cue, "kind": "x", "planned_ms": planned, "actual_ms": planned, "drift_ms": 0}));
        vec![
            rec(1, 0, "studio", recorder::RUN_START, json!({})),
            rec(2, 1000, "studio", recorder::SHOW_INPUT, json!({"event": "START", "from": "PRESHOW", "to": "INTRO", "applied": true, "actions": []})),
            cue(3, 1000, "INTRO", "visual", "v1", 0),
            cue(4, 3000, "INTRO", "lighting", "l1", 2000),
            rec(5, 4000, "studio", recorder::SHOW_INPUT, json!({"event": "DROP", "from": "INTRO", "to": "DROP", "applied": true, "actions": []})),
            cue(6, 4000, "DROP", "visual", "v2", 0),
            rec(7, 6000, "runtime", "show.delivery", json!({})),
        ]
    }

    fn lines(r: &Value) -> Vec<String> {
        r["summary"]["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn the_same_show_changes_nothing() {
        let r = run(&real_run(), &chart(), &[], T0 + 6000).unwrap();
        assert_eq!(r["recorded_path"], json!(["PRESHOW", "INTRO", "DROP"]));
        assert_eq!(r["rehearsed_path"], r["recorded_path"]);
        assert_eq!(r["summary"]["different"], false, "{:?}", lines(&r));
        assert!(r["summary"]["verdict"].as_str().unwrap().starts_with("no difference"));
        // INTRO's laser cue (4 s) never fired in the real run either, and is not reported as news
        assert_eq!(r["visits"][1]["fires"].as_array().unwrap().len(), 2);
        assert_eq!(r["visits"][1]["cut_off"][0]["cue"], "z1");
    }

    #[test]
    fn a_cue_moved_past_the_end_of_its_scene_is_reported_as_cut_off() {
        let mut c = chart();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "lighting").unwrap().cues[0].at_ms = 3_500;
        let r = run(&real_run(), &c, &[], T0 + 6000).unwrap();
        assert_eq!(r["summary"]["different"], true);
        let l = lines(&r).join("\n");
        assert!(l.contains("would NOT fire") && l.contains("left at 3.0s before it is due at 3.5s"), "{l}");
        assert!(r["same_path"] == true, "the path is the same, only a cue is lost");
    }

    #[test]
    fn a_cue_moved_earlier_and_one_added_are_both_reported() {
        let mut c = chart();
        let lanes = &mut c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes;
        lanes.iter_mut().find(|l| l.id == "lighting").unwrap().cues[0].at_ms = 1_500;
        lanes.iter_mut().find(|l| l.id == "laser").unwrap().cues[0].at_ms = 2_900;
        let l = lines(&run(&real_run(), &c, &[], T0 + 6000).unwrap()).join("\n");
        assert!(l.contains("moves from 2.0s to 1.5s"), "{l}");
        assert!(l.contains("would now fire at 2.9s (it did not before)"), "{l}");
    }

    #[test]
    fn a_timer_that_ends_a_scene_early_changes_how_long_it_lasts() {
        let mut c = chart();
        let mut t = c.transitions.iter().find(|t| t.source == "INTRO").unwrap().clone();
        t.event = None;
        t.after_ms = Some(2_500);
        c.transitions.push(t);
        let r = run(&real_run(), &c, &[], T0 + 6000).unwrap();
        let l = lines(&r).join("\n");
        assert!(l.contains("INTRO: INTRO lasts 2.5s instead of 3.0s"), "{l}");
        assert!(l.contains("the recorded input 'DROP' would do nothing: this show is in DROP"), "{l}");
        assert_eq!(r["rehearsed_path"], json!(["PRESHOW", "INTRO", "DROP"]));
    }

    #[test]
    fn a_show_that_no_longer_reaches_a_scene_says_so() {
        let mut c = chart();
        c.transitions.iter_mut().find(|t| t.source == "INTRO").unwrap().event = Some("NEXT".into());
        let r = run(&real_run(), &c, &[], T0 + 6000).unwrap();
        assert_eq!(r["same_path"], false);
        let l = lines(&r).join("\n");
        assert!(l.contains("the recorded input 'DROP' would do nothing: this show is in INTRO"), "{l}");
        assert!(l.contains("This show ends in INTRO but the real run went on to DROP"), "{l}");
    }

    #[test]
    fn cues_are_checked_against_the_rig_and_show_what_would_be_sent() {
        let dev = |id: &str, kind: &str| rig::Device { id: id.into(), label: id.into(), kind: kind.into(), dept: rig::dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(9), listen_port: None, profile: None, universe: None, patch: None };
        // a Resolume for visuals only: the lighting cue has nowhere to go
        let r = run(&real_run(), &chart(), &[dev("r1", "resolume")], T0 + 6000).unwrap();
        assert!(r["summary"]["different"] == true);
        let problems: Vec<&str> = r["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert!(problems.iter().any(|p| p.contains("no lighting device in the rig")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("use a column number")), "the demo's scene name is not a Resolume column: {problems:?}");
        // with a column number the cue is fine, and the wire message is shown
        let mut c = chart();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes[0].cues[0].action.payload = json!({"scene": "3"});
        let r = run(&real_run(), &c, &[dev("r1", "resolume")], T0 + 6000).unwrap();
        assert_eq!(r["visits"][1]["fires"][0]["sent_as"][0]["address"], "/composition/columns/3/connect");
    }

    #[test]
    fn a_scene_minimum_would_have_held_a_press_that_came_too_early() {
        let mut c = chart();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().min_ms = Some(5_000);
        let r = run(&real_run(), &c, &[], T0 + 6000).unwrap();
        let l = lines(&r).join("\n");
        assert!(l.contains("the recorded input 'DROP' would be held: INTRO must run 5.0s and had run 3.0s"), "{l}");
        assert_eq!(r["rehearsed_path"], json!(["PRESHOW", "INTRO"]), "the show never reaches DROP");
        // a minimum the real run already met changes nothing
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().min_ms = Some(2_000);
        assert_eq!(run(&real_run(), &c, &[], T0 + 6000).unwrap()["summary"]["different"], false);
    }

    #[test]
    fn a_dmx_cue_is_read_as_channels_and_a_console_cue_sent_to_it_is_a_problem() {
        let dev = |kind: &str| rig::Device { id: "d".into(), label: "Rack".into(), kind: kind.into(), dept: "lighting".into(), host: Some("127.0.0.1".into()), port: Some(6454), listen_port: None, profile: None, universe: Some(2), patch: None };
        let mut c = chart();
        c.states.iter_mut().find(|s| s.id == "INTRO").unwrap().timeline.as_mut().unwrap().lanes.iter_mut().find(|l| l.id == "lighting").unwrap().cues[0].action.payload = json!({"look": "1-4=50% over 2"});
        let r = run(&real_run(), &c, &[dev("artnet")], T0 + 6000).unwrap();
        let fire = r["visits"][1]["fires"].as_array().unwrap().iter().find(|f| f["cue"] == "l1").unwrap();
        assert_eq!(fire["sent_as"][0]["address"], "Art-Net universe 2");
        assert!(fire["sent_as"][0]["args"][1].as_str().unwrap().starts_with("4 channel(s) over 2"), "{fire}");
        // the demo's own value for that cue is a console look ("blue"): not channels
        let r = run(&real_run(), &chart(), &[dev("artnet")], T0 + 6000).unwrap();
        assert!(r["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("use channels like")), "{}", r["problems"]);
    }

    #[test]
    fn there_must_be_a_run_to_rehearse_against() {
        assert!(run(&[], &chart(), &[], 0).unwrap_err().contains("no recorded run"));
    }
}
