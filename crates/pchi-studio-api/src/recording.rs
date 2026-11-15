//! A run as a file you can keep, scrub through and check.
//!
//! The runtime already writes every input, context change, cue, delivery, live edit and decision to its log. A **recording** is that log for
//! the latest run, with the show it started from, as one JSON file (`pchi-run/1`): hand it to the person who was not there, attach it to the
//! post-show notes, open it in Studio's Live tab and drag through it.
//!
//! * `export`: the latest run of a log, as the file.
//! * `parse`: read such a file back into log records (and refuse anything that is not one).
//! * `view`: the run laid out for scrubbing: which scene was running when, and a list of moments (a scene change, a cue and how late it was,
//!   a context value, a delivery that failed, an edit, who decided what). Studio draws the timeline from this.
//! * `verify`: play the recorded inputs back through the show (with its live edits applied where they happened) and check it goes where the
//!   recording says it went. A recording that does not replay is a sign the show file or the runtime changed under it.
//!
//! What a recording holds that you may not want to share: who pressed what (names from the roster), the reasons people and agents gave, and
//! the addresses of the rig. It holds no keys, tokens or passwords. Treat it like the show file.

use pchi_eventstore::{recorder, EventRecord};
use pchi_showchart::{ShowEngine, Showchart};
use serde_json::{json, Value};

pub const FORMAT: &str = "pchi-run/1";

/// The records of the latest run in a log (from its `run.start` on).
fn latest(records: &[EventRecord]) -> Result<&[EventRecord], String> {
    let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no recorded run")?;
    Ok(&records[start..])
}

/// The latest run, as a recording.
pub fn export(records: &[EventRecord]) -> Result<Value, String> {
    let run = latest(records)?;
    let show = run[0].payload["chart"]["id"].as_str().unwrap_or("show");
    Ok(json!({
        "format": FORMAT, "show": show, "started_ts": run[0].timestamp, "ended_ts": run.last().map_or(run[0].timestamp, |r| r.timestamp),
        "records": run.iter().map(|r| json!({"id": r.id, "ts": r.timestamp, "source": r.source, "target": r.target, "type": r.event_type, "payload": r.payload})).collect::<Vec<_>>(),
    }))
}

/// The line-by-line format `--record` writes: a header line, then one record a line, so a run is on disk as it happens.
pub const LOG_FORMAT: &str = "pchi-runlog/1";

/// Called with every record as it is written.
pub type Tee = pchi_eventstore::Tee;

fn record_json(r: &EventRecord) -> Value {
    json!({"id": r.id, "ts": r.timestamp, "source": r.source, "target": r.target, "type": r.event_type, "payload": r.payload})
}

/// A tee that appends each record, as a line, to `path` (with a header line first if the file is new or empty; then `existing` is written too, so a runtime
/// that is resuming a run starts a complete file). Each line is flushed as it is written, and the file is synced to disk at most once a second.
pub fn file_tee(path: &str, existing: &[EventRecord]) -> Result<Tee, String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("opening {path}: {e}"))?;
    let fresh = f.metadata().map(|m| m.len() == 0).unwrap_or(true);
    if fresh {
        writeln!(f, "{}", json!({"format": LOG_FORMAT})).map_err(|e| format!("writing {path}: {e}"))?;
        for r in existing {
            writeln!(f, "{}", record_json(r)).map_err(|e| format!("writing {path}: {e}"))?;
        }
        f.flush().and_then(|_| f.sync_data()).map_err(|e| format!("writing {path}: {e}"))?;
    }
    let state = std::sync::Mutex::new((f, std::time::Instant::now()));
    let path = path.to_string();
    Ok(Box::new(move |r: &EventRecord| {
        let mut g = state.lock().unwrap();
        let line = format!("{}\n", record_json(r));
        if let Err(e) = g.0.write_all(line.as_bytes()).and_then(|_| g.0.flush()) {
            eprintln!("warning: could not write the record to {path}: {e}");
            return;
        }
        if g.1.elapsed() >= std::time::Duration::from_secs(1) {
            let _ = g.0.sync_data();
            g.1 = std::time::Instant::now();
        }
    }))
}

/// Read a `--record` file: the latest run in it. A last line that was cut off (the runtime died mid-write) is dropped, not an error.
fn parse_log(text: &str) -> Result<Vec<EventRecord>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head = lines.next().ok_or("the file is empty")?;
    if serde_json::from_str::<Value>(head).map_err(|e| format!("not a recording: {e}"))?["format"] != json!(LOG_FORMAT) {
        return Err(format!("not a recording: the first line should say \"format\": \"{LOG_FORMAT}\""));
    }
    let rest: Vec<&str> = lines.collect();
    let mut out = Vec::with_capacity(rest.len());
    for (i, l) in rest.iter().enumerate() {
        match serde_json::from_str::<Value>(l) {
            Ok(r) => {
                let (Some(ts), Some(kind)) = (r["ts"].as_i64(), r["type"].as_str()) else { return Err(format!("line {} has no time or type", i + 2)) };
                out.push(EventRecord { id: r["id"].as_i64().unwrap_or(i as i64 + 1), timestamp: ts, source: r["source"].as_str().unwrap_or("").to_string(), target: r["target"].as_str().map(String::from), event_type: kind.to_string(), payload: r["payload"].clone() });
            }
            Err(_) if i + 1 == rest.len() => {} // cut off mid-write
            Err(e) => return Err(format!("line {} is not a record: {e}", i + 2)),
        }
    }
    let start = out.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("not a recording: it holds no run")?;
    Ok(out.split_off(start))
}

/// Read a recording back into records: the one-file format `export` writes, or a `--record` file.
pub fn parse(text: &str) -> Result<Vec<EventRecord>, String> {
    if text.trim_start().starts_with(&format!("{{\"format\":\"{LOG_FORMAT}\"")) {
        return parse_log(text);
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a recording: {e}"))?;
    if v["format"] != json!(FORMAT) {
        return Err(format!("not a recording: it should say \"format\": \"{FORMAT}\""));
    }
    let list = v["records"].as_array().ok_or("not a recording: it has no records")?;
    if list.len() > 2_000_000 {
        return Err("that recording is too large".into());
    }
    let mut out = Vec::with_capacity(list.len());
    for (i, r) in list.iter().enumerate() {
        let (Some(ts), Some(kind)) = (r["ts"].as_i64(), r["type"].as_str()) else { return Err(format!("record {i} has no time or type")) };
        out.push(EventRecord { id: r["id"].as_i64().unwrap_or(i as i64 + 1), timestamp: ts, source: r["source"].as_str().unwrap_or("").to_string(), target: r["target"].as_str().map(String::from), event_type: kind.to_string(), payload: r["payload"].clone() });
    }
    if out.first().map(|r| r.event_type.as_str()) != Some(recorder::RUN_START) {
        return Err("not a recording: it must begin with the run's start".into());
    }
    Ok(out)
}

fn secs(ms: i64) -> String {
    let s = ms as f64 / 1000.0;
    if s >= 60.0 { format!("{}:{:04.1}", (s / 60.0) as i64, s % 60.0) } else { format!("{s:.1}s") }
}

fn label_of(chart: &Showchart, scene: &str, lane: &str, cue: &str) -> String {
    chart.state(scene).and_then(|s| s.timeline.as_ref()).and_then(|t| t.lanes.iter().find(|l| l.id == lane)).and_then(|l| l.cues.iter().find(|c| c.id == cue)).and_then(|c| c.label.clone()).unwrap_or_else(|| cue.to_string())
}

/// The run laid out for scrubbing.
pub fn view(records: &[EventRecord]) -> Result<Value, String> {
    let run = latest(records)?;
    let start = run[0].timestamp;
    let mut chart: Showchart = serde_json::from_value(run[0].payload["chart"].clone()).map_err(|e| format!("the recorded show is unreadable: {e}"))?;
    let initial_context = run[0].payload["context"].clone();
    let end = run.last().map_or(start, |r| r.timestamp);
    let mut scene = chart.initial.clone();
    // the first recorded input says where the show began
    if let Some(r) = run.iter().find(|r| r.event_type == recorder::SHOW_INPUT) {
        scene = r.payload["from"].as_str().unwrap_or(&scene).to_string();
    }
    let mut visits: Vec<Value> = Vec::new();
    let (mut entered, mut via) = (0i64, "start".to_string());
    let mut frames: Vec<Value> = Vec::new();
    let (mut cues, mut late_ms, mut bad, mut edits, mut decisions) = (0, 0i64, 0, 0, 0);
    let mut people: std::collections::BTreeSet<String> = Default::default();
    let mut frame = |t: i64, kind: &str, scene: &str, text: String, extra: Value| {
        let mut f = json!({"t": t, "kind": kind, "scene": scene, "text": text});
        if let Some(o) = extra.as_object() {
            f.as_object_mut().unwrap().extend(o.clone());
        }
        frames.push(f);
    };
    for r in &run[1..] {
        let t = (r.timestamp - start).max(0);
        let p = &r.payload;
        match r.event_type.as_str() {
            recorder::SHOW_INPUT => {
                let (event, to) = (p["event"].as_str().unwrap_or(""), p["to"].as_str().unwrap_or(""));
                if p["applied"] == json!(true) {
                    visits.push(json!({"scene": scene, "from_ms": entered, "to_ms": t, "via": via}));
                    frame(t, "scene", to, format!("{} → {to}  ({event}, {})", p["from"].as_str().unwrap_or(&scene), r.source), json!({"event": event, "source": r.source, "from": p["from"]}));
                    scene = to.to_string();
                    entered = t;
                    via = event.to_string();
                } else {
                    frame(t, "ignored", &scene.clone(), format!("'{event}' did nothing in {scene}  ({})", r.source), json!({"event": event, "source": r.source}));
                }
            }
            recorder::SHOW_CONTEXT => {
                frame(t, "context", &scene.clone(), format!("{} = {}", p["key"].as_str().unwrap_or(""), p["value"]), json!({"key": p["key"], "value": p["value"]}));
            }
            "show.cue" => {
                cues += 1;
                let drift = p["drift_ms"].as_i64().unwrap_or(0);
                late_ms = late_ms.max(drift.abs());
                let label = label_of(&chart, p["scene"].as_str().unwrap_or(&scene), p["lane"].as_str().unwrap_or(""), p["cue"].as_str().unwrap_or(""));
                frame(t, "cue", &scene.clone(), format!("{label}  ({}, {} planned {})", p["kind"].as_str().unwrap_or(""), if drift.abs() < 20 { "on time".to_string() } else { format!("{drift:+} ms") }, secs(p["planned_ms"].as_i64().unwrap_or(0))), json!({"lane": p["lane"], "cue": p["cue"], "drift_ms": drift}));
            }
            "show.delivery" => {
                let status = p["status"].as_str().unwrap_or("");
                let ok = matches!(status, "sent" | "dry-run");
                if !ok {
                    bad += 1;
                }
                frame(t, "delivery", &scene.clone(), format!("{}: {status}{}", p["label"].as_str().or(p["kind"].as_str()).unwrap_or("a device"), p["error"].as_str().map(|e| format!(" — {e}")).unwrap_or_default()), json!({"status": status, "bad": !ok, "device": p["device"]}));
            }
            "show.edit" => {
                edits += 1;
                let changes: Vec<&str> = p["changes"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).collect();
                if let Some(c) = p.get("chart").and_then(|c| serde_json::from_value::<Showchart>(c.clone()).ok()) {
                    chart = c;
                }
                frame(t, "edit", &scene.clone(), if changes.is_empty() { "the show was re-sent unchanged".into() } else { format!("{}{}", if p["undo"] == json!(true) { "undone: " } else { "edited: " }, changes.join("; ")) }, json!({"undo": p["undo"]}));
            }
            "show.decision" => {
                decisions += 1;
                let by = p["by"].as_str().unwrap_or("").to_string();
                if !by.is_empty() {
                    people.insert(by.clone());
                }
                frame(t, "decision", &scene.clone(), format!("{by}: {}", p["text"].as_str().unwrap_or("")), json!({"by": by, "gate": p["gate"], "rule": p["rule"]}));
            }
            "show.held" => frame(t, "held", &scene.clone(), format!("'{}' held: the scene has {} s of its minimum to go", p["event"].as_str().unwrap_or(""), p["remaining_ms"].as_i64().unwrap_or(0) as f64 / 1000.0), json!({})),
            "show.resume" => frame(t, "resume", &scene.clone(), format!("the runtime carried on after {:.0} s down", p["downtime_ms"].as_f64().unwrap_or(0.0) / 1000.0), json!({})),
            "show.fenced" => frame(t, "fenced", &scene.clone(), format!("{} took over the show", p["by"].as_str().unwrap_or("another runtime")), json!({})),
            _ => {}
        }
    }
    visits.push(json!({"scene": scene, "from_ms": entered, "to_ms": null, "via": via}));
    let length = (end - start).max(0);
    let scenes: Vec<String> = visits.iter().filter_map(|v| v["scene"].as_str().map(String::from)).collect();
    Ok(json!({
        "show": chart.id, "started_ts": start, "ended_ts": end, "length_ms": length,
        "initial_context": initial_context, "scenes": visits, "frames": frames,
        "summary": {"scenes_visited": scenes.len(), "path": scenes, "cues": cues, "worst_cue_drift_ms": late_ms, "deliveries_wrong": bad, "edits": edits, "decisions": decisions, "who": people.into_iter().collect::<Vec<_>>(),
                    "line": format!("{} long, {} scene(s), {cues} cue(s){}{}", secs(length), scenes.len(), if bad > 0 { format!(", {bad} delivery problem(s)") } else { String::new() }, if edits > 0 { format!(", {edits} live edit(s)") } else { String::new() })},
    }))
}

/// Play the recorded inputs back through the show and check it goes where the recording says. Live edits are applied where they happened.
pub fn verify(records: &[EventRecord]) -> Result<Value, String> {
    let run = latest(records)?;
    let chart: Showchart = serde_json::from_value(run[0].payload["chart"].clone()).map_err(|e| format!("the recorded show is unreadable: {e}"))?;
    let mut engine = ShowEngine::new(chart).map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))?;
    let (mut checked, mut diverged) = (0, Vec::new());
    for r in &run[1..] {
        match r.event_type.as_str() {
            "show.edit" => {
                if let Some(c) = r.payload.get("chart").and_then(|c| serde_json::from_value::<Showchart>(c.clone()).ok()) {
                    engine.chart = c;
                }
            }
            recorder::SHOW_CONTEXT => {
                if let Some(k) = r.payload["key"].as_str() {
                    engine.set_context(k, r.payload["value"].clone());
                }
            }
            recorder::SHOW_INPUT => {
                let event = r.payload["event"].as_str().unwrap_or("");
                let applied = engine.try_dispatch(event).is_some();
                checked += 1;
                let (rec_applied, rec_to) = (r.payload["applied"].as_bool().unwrap_or(false), r.payload["to"].as_str().unwrap_or(""));
                if applied != rec_applied || engine.current_state != rec_to {
                    diverged.push(format!("at {}: '{event}' went to {} on replay but to {rec_to} in the recording", secs(r.timestamp - run[0].timestamp), engine.current_state));
                    if diverged.len() >= 5 {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(json!({"matches": diverged.is_empty(), "inputs_checked": checked, "differences": diverged,
              "verdict": if diverged.is_empty() { format!("the run replays exactly: {checked} input(s) went where the recording says") } else { format!("the run does NOT replay: {}", diverged[0]) }}))
}
