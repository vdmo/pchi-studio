//! Memory derived from a run: what the log says went wrong or was decided, as drafts that carry the hash of the records they came from.
//!
//! Nothing here is invented. Each memory is a sentence built from the recorded events (a delivery that failed, a cue that fired a quarter of a second late, a
//! restart, a takeover, a person's decision and the reason they gave), with a deterministic id, so deriving the same run twice gives the same memories and
//! changes nothing. They are **drafts**: the log says *that* it happened, a person decides whether it is worth remembering (`verify`).
//!
//! `check_evidence` goes the other way: given the records, does a memory's evidence still match? A memory whose records have changed or gone is flagged, not trusted.

use crate::{Kind, Memory, New, Record, Who};
use pchi_eventstore::{recorder, EventRecord};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const LATE_MS: i64 = 250;

fn ymd(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let (y, doy) = (yoe + era * 400, doe - (365 * yoe + yoe / 4 - yoe / 100));
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// `run:<start>:<ids>:<hash>`: the hash is of the records' text, in order.
pub fn evidence(start_ts: i64, recs: &[&EventRecord]) -> String {
    let mut h = Sha256::new();
    for r in recs {
        h.update(format!("{}|{}|{}|{}\n", r.id, r.timestamp, r.event_type, r.payload));
    }
    format!("run:{start_ts}:{}:{}", recs.iter().map(|r| r.id.to_string()).collect::<Vec<_>>().join(","), &hex::encode(h.finalize())[..32])
}

/// Does the evidence of a memory still match these records? `None` if it has no run evidence to check; `Some(false)` if a record it cites is gone or changed.
pub fn check_evidence(rec: &Record, records: &[EventRecord]) -> Option<bool> {
    let mut any = false;
    for e in rec.evidence.iter().filter(|e| e.starts_with("run:")) {
        any = true;
        let parts: Vec<&str> = e.splitn(4, ':').collect();
        let (Some(start), Some(ids)) = (parts.get(1).and_then(|s| s.parse::<i64>().ok()), parts.get(2)) else { return Some(false) };
        let ids: Vec<i64> = ids.split(',').filter_map(|i| i.parse().ok()).collect();
        let found: Vec<&EventRecord> = ids.iter().filter_map(|i| records.iter().find(|r| r.id == *i)).collect();
        if found.len() != ids.len() || evidence(start, &found) != *e {
            return Some(false);
        }
    }
    any.then_some(true)
}

/// The memories a run gives rise to, as drafts.
pub fn from_run(records: &[EventRecord]) -> Result<Vec<New>, String> {
    let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no recorded run")?;
    let run = &records[start..];
    let (t0, show) = (run[0].timestamp, run[0].payload["chart"]["id"].as_str().unwrap_or("show").to_string());
    let date = ymd(t0);
    let source_ref = format!("run:{t0}");
    let mk = |kind: Kind, text: String, key: Option<String>, dept: &str, ev: Vec<&EventRecord>| New { kind, text, key, show: show.clone(), dept: dept.to_string(), confidence: 0.9, source_type: "run".into(), source_ref: source_ref.clone(), evidence: vec![evidence(t0, &ev)], verify: false };
    let mut out = Vec::new();

    // sends that did not go through, one memory for each device and reason
    let mut failed: BTreeMap<(String, String), Vec<&EventRecord>> = BTreeMap::new();
    for r in run.iter().filter(|r| r.event_type == "show.delivery" && matches!(r.payload["status"].as_str(), Some("failed") | Some("no-device"))) {
        let who = r.payload["label"].as_str().or(r.payload["device"].as_str()).or(r.payload["dept"].as_str()).unwrap_or("a device").to_string();
        let why = r.payload["error"].as_str().unwrap_or(r.payload["status"].as_str().unwrap_or("failed")).to_string();
        failed.entry((who, why)).or_default().push(r);
    }
    for ((who, why), recs) in failed {
        let dept = recs[0].payload["dept"].as_str().unwrap_or("").to_string();
        out.push(mk(Kind::Incident, format!("In the {show} run of {date}, {} cue(s) to {who} did not go through: {why}.", recs.len()), Some(format!("delivery:{who}")), &dept, recs));
    }
    // cues that ran late: the three worst
    let mut late: Vec<&EventRecord> = run.iter().filter(|r| r.event_type == "show.cue" && r.payload["drift_ms"].as_i64().unwrap_or(0).abs() > LATE_MS).collect();
    late.sort_by_key(|r| std::cmp::Reverse(r.payload["drift_ms"].as_i64().unwrap_or(0).abs()));
    for r in late.into_iter().take(3) {
        let p = &r.payload;
        out.push(mk(
            Kind::Incident,
            format!("In the {show} run of {date}, cue {}/{} in scene {} fired {} ms {} its planned time ({} ms into the scene).", p["lane"].as_str().unwrap_or(""), p["cue"].as_str().unwrap_or(""), p["scene"].as_str().unwrap_or(""), p["drift_ms"].as_i64().unwrap_or(0).abs(), if p["drift_ms"].as_i64().unwrap_or(0) >= 0 { "after" } else { "before" }, p["planned_ms"].as_i64().unwrap_or(0)),
            Some(format!("late:{}/{}", p["lane"].as_str().unwrap_or(""), p["cue"].as_str().unwrap_or(""))),
            "",
            vec![r],
        ));
    }
    // what happened to the runtime itself
    for r in run.iter().filter(|r| r.event_type == "show.resume") {
        out.push(mk(Kind::Incident, format!("In the {show} run of {date}, the runtime restarted and carried on after {:.0} s down.", r.payload["downtime_ms"].as_f64().unwrap_or(0.0) / 1000.0), Some("runtime-restart".into()), "", vec![r]));
    }
    for r in run.iter().filter(|r| r.event_type == "show.fenced") {
        out.push(mk(Kind::Incident, format!("In the {show} run of {date}, {} took over the show and this runtime stopped.", r.payload["by"].as_str().unwrap_or("another runtime")), Some("takeover".into()), "", vec![r]));
    }
    for r in run.iter().filter(|r| r.event_type == "show.decision" && r.payload["rule"].as_str().is_some_and(|x| x.starts_with("failover."))) {
        out.push(mk(Kind::Incident, format!("In the {show} run of {date}: {}", r.payload["text"].as_str().unwrap_or("").trim_end_matches('.').to_string() + "."), Some(format!("failover:{}", r.payload["rule"].as_str().unwrap_or(""))), "", vec![r]));
    }
    // decisions: a rejected proposal or an edit that was allowed or refused, with who and why
    for r in run.iter().filter(|r| r.event_type == "show.decision" && matches!(r.payload["rule"].as_str(), Some("show.reject") | Some("show.edit") | Some("show.propose"))).take(20) {
        let p = &r.payload;
        let by = p["by"].as_str().filter(|b| !b.is_empty()).unwrap_or("someone");
        out.push(mk(Kind::Decision, format!("On {date}, in {show}, {by}: {} ({}).", p["text"].as_str().unwrap_or("").trim_end_matches('.'), p["gate"].as_str().unwrap_or("")), Some(format!("decision:{}", p["rule"].as_str().unwrap_or(""))), "", vec![r]));
    }
    Ok(out)
}

/// What deriving a run did.
#[derive(Debug)]
pub struct Derived {
    pub new: usize,
    pub already: usize,
    pub conflicts: usize,
}

/// Derive a run into memory as drafts. Running it again on the same run changes nothing.
pub fn derive_into(mem: &Memory, records: &[EventRecord], who: &Who, now: i64) -> Result<Derived, String> {
    let (mut new, mut already, mut conflicts) = (0, 0, 0);
    for n in from_run(records)? {
        match mem.remember(who, n, now) {
            Ok((r, true)) => {
                new += 1;
                conflicts += r.conflicts_with.len();
            }
            Ok((_, false)) => already += 1,
            Err(e) => return Err(e.0),
        }
    }
    Ok(Derived { new, already, conflicts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecallOpts, Status};
    use serde_json::{json, Value};

    fn rec(id: i64, ts: i64, kind: &str, payload: Value) -> EventRecord {
        EventRecord { id, timestamp: ts, source: "t".into(), target: None, event_type: kind.into(), payload }
    }

    fn run() -> Vec<EventRecord> {
        let t = 1_800_000_000_000;
        vec![
            rec(1, t, "run.start", json!({"chart": {"id": "gala"}, "context": {}})),
            rec(2, t + 1000, "show.cue", json!({"scene": "OPENING", "lane": "haze", "cue": "c4", "kind": "lighting.look", "planned_ms": 4000, "actual_ms": 4400, "drift_ms": 400})),
            rec(3, t + 1000, "show.delivery", json!({"status": "failed", "label": "House desk", "dept": "lighting", "error": "no route to host"})),
            rec(4, t + 2000, "show.delivery", json!({"status": "failed", "label": "House desk", "dept": "lighting", "error": "no route to host"})),
            rec(5, t + 3000, "show.decision", json!({"rule": "show.reject", "gate": "DENY", "by": "Alex", "text": "rejected the agent's proposal to move the finale: too close to the laser cue"})),
            rec(6, t + 4000, "show.resume", json!({"downtime_ms": 12000})),
        ]
    }

    #[test]
    fn a_run_gives_drafts_with_evidence_and_deriving_twice_changes_nothing() {
        let m = Memory::in_memory().unwrap();
        let d = derive_into(&m, &run(), &Who::agent("memory"), 1_800_000_100_000).unwrap();
        assert_eq!((d.new, d.already), (4, 0), "{d:?}");
        let all = m.list(&Default::default()).unwrap();
        assert!(all.iter().all(|r| r.status == Status::Draft && r.source_type == "run" && r.evidence.len() == 1));
        let failed = all.iter().find(|r| r.text.contains("House desk")).unwrap();
        assert!(failed.text.contains("2 cue(s)") && failed.text.contains("2027-01-15") && failed.text.contains("no route to host"), "{}", failed.text);
        assert!(all.iter().any(|r| r.kind == Kind::Decision && r.text.contains("too close to the laser cue")));
        assert!(all.iter().any(|r| r.text.contains("restarted") && r.text.contains("12 s")));
        let again = derive_into(&m, &run(), &Who::agent("memory"), 1_800_000_200_000).unwrap();
        assert_eq!((again.new, again.already), (0, 4));
        // and it can be asked about
        let r = m.recall("why did the house desk fail", &RecallOpts::default(), 1_800_000_300_000).unwrap();
        assert!(r.abstained.is_none() && r.hits[0].record.text.contains("House desk"), "{:?}", r.to_json());
    }

    #[test]
    fn a_memory_whose_records_have_changed_is_flagged() {
        let m = Memory::in_memory().unwrap();
        derive_into(&m, &run(), &Who::agent("memory"), 1).unwrap();
        let all = m.list(&Default::default()).unwrap();
        let r = all.iter().find(|r| r.text.contains("House desk")).unwrap();
        assert_eq!(check_evidence(r, &run()), Some(true));
        let mut tampered = run();
        tampered[2].payload["error"] = json!("it was fine");
        assert_eq!(check_evidence(r, &tampered), Some(false));
        assert_eq!(check_evidence(r, &run()[..2]), Some(false), "a cited record is gone");
        let (p, _) = m.remember(&Who::person("Alex"), New::new(Kind::Preference, "Likes it quiet."), 1).unwrap();
        assert_eq!(check_evidence(&p, &run()), None);
    }
}

/// Read a recording file (`pchi-run/1` one-file export, or a `pchi-runlog/1` file written by `--record`) into records.
pub fn parse_recording(text: &str) -> Result<Vec<EventRecord>, String> {
    use serde_json::Value;
    let rec = |r: &Value, i: usize| -> Option<EventRecord> {
        Some(EventRecord { id: r["id"].as_i64().unwrap_or(i as i64 + 1), timestamp: r["ts"].as_i64()?, source: r["source"].as_str().unwrap_or("").to_string(), target: r["target"].as_str().map(String::from), event_type: r["type"].as_str()?.to_string(), payload: r["payload"].clone() })
    };
    let first: Value = serde_json::from_str(text.lines().find(|l| !l.trim().is_empty()).unwrap_or("")).or_else(|_| serde_json::from_str(text)).map_err(|e| format!("not a recording: {e}"))?;
    if first["format"] == "pchi-runlog/1" {
        let mut out = Vec::new();
        for (i, l) in text.lines().filter(|l| !l.trim().is_empty()).skip(1).enumerate() {
            match serde_json::from_str::<Value>(l).ok().and_then(|v| rec(&v, i)) {
                Some(r) => out.push(r),
                None if l.trim_end().len() == l.len() && i > 0 => {}
                None => {}
            }
        }
        return Ok(out);
    }
    let whole: Value = serde_json::from_str(text).map_err(|e| format!("not a recording: {e}"))?;
    if whole["format"] != "pchi-run/1" {
        return Err("not a recording: expected format pchi-run/1 or pchi-runlog/1".into());
    }
    whole["records"].as_array().ok_or("not a recording: no records")?.iter().enumerate().map(|(i, r)| rec(r, i).ok_or_else(|| format!("record {} is not readable", i + 1))).collect()
}
