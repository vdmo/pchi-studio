//! The post-show report: one page, built from a recording, for the person who was not there.
//!
//! A recording (`recording.rs`) holds everything the runtime wrote during a run. This turns it into something to read and hand over: how long the show ran and
//! where it went, how late the cues were, whether anything failed to reach a device, every live edit, every takeover or pause, who decided what, and whether
//! the signed receipts hold together. It is one self-contained HTML file (no scripts, no network), so it can be mailed, printed or kept with the invoice.
//!
//! It says what the log says. It cannot say whether the lights *looked* right: a delivery "sent" means the message left the runtime, not that the tool did it
//! (that is what the gear check is for), and the report says so at the bottom.

use crate::recording;
use pchi_eventstore::{recorder, EventRecord};
use primeswarm_pchi::governance::{verify_receipt, GovernanceReceipt};
use serde_json::Value;

/// How the signed receipts check out.
pub struct Chain {
    pub count: usize,
    pub ok: bool,
    pub text: String,
}

/// Check an export of the receipts (`GET /governance/export`): each links to the one before, and every signature verifies against the key in the export.
/// This is the same check as `scripts/verify_governance_chain.py`, except that it trusts the key the export carries: to be sure of WHO signed, compare that
/// key with the one you wrote down when the show was set up.
pub fn check_chain(export: &Value) -> Chain {
    let receipts = export["receipts"].as_array().cloned().unwrap_or_default();
    let Some(vk) = export["verifying_key"].as_str() else {
        return Chain { count: receipts.len(), ok: receipts.is_empty(), text: export["note"].as_str().unwrap_or("no signed receipts were supplied").to_string() };
    };
    let mut parent: Option<String> = None;
    for (i, r) in receipts.iter().enumerate() {
        let Ok(rec) = serde_json::from_value::<GovernanceReceipt>(r.clone()) else { return Chain { count: receipts.len(), ok: false, text: format!("receipt {} could not be read", i + 1) } };
        if rec.parent_decision_hash != parent {
            return Chain { count: receipts.len(), ok: false, text: format!("receipt {} does not follow the one before it: the chain is broken or something was removed", i + 1) };
        }
        if !verify_receipt(&rec, vk) {
            return Chain { count: receipts.len(), ok: false, text: format!("the signature of receipt {} does not verify", i + 1) };
        }
        parent = Some(rec.decision_hash.clone());
    }
    Chain { count: receipts.len(), ok: true, text: format!("{} receipt(s), each linked to the one before and signed by key {}…", receipts.len(), vk.chars().take(12).collect::<String>()) }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn clock(ms: i64) -> String {
    if ms.abs() < 60_000 {
        return format!("{:.1} s", ms.max(0) as f64 / 1000.0);
    }
    let s = (ms.max(0) / 1000) as i64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn when(ts: i64) -> String {
    let secs = ts.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    format!("{} {:02}:{:02}:{:02} UTC", crate::gearcheck::date_of_ms(days * 86_400_000), rem / 3600, (rem / 60) % 60, rem % 60)
}

fn percentile(sorted: &[i64], p: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

const CSS: &str = "
:root{--bg:#fff;--fg:#1b1d22;--mut:#5d6470;--line:#dfe3ea;--card:#f6f7f9;--ok:#1a7f4b;--bad:#b3261e;--warn:#9a6700;--c0:#4c78c8;--c1:#c8794c;--c2:#4cae8a;--c3:#a64cc8;--c4:#c8b44c;--c5:#c84c7a}
@media (prefers-color-scheme:dark){:root{--bg:#14161a;--fg:#e8eaee;--mut:#9aa3b0;--line:#2c313a;--card:#1c1f25;--ok:#5fd39a;--bad:#ff8a80;--warn:#e3b341}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}
main{max-width:920px;margin:0 auto;padding:28px 16px 60px}h1{font-size:26px;margin:0 0 4px}h2{font-size:17px;margin:34px 0 8px;border-bottom:1px solid var(--line);padding-bottom:4px}
.sub{color:var(--mut);margin:0 0 18px}.verdict{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px 14px;margin:14px 0}
.ok{color:var(--ok)}.bad{color:var(--bad)}.warn{color:var(--warn)}.mut{color:var(--mut)}
table{border-collapse:collapse;width:100%;font-size:14px}th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--line);vertical-align:top}th{color:var(--mut);font-weight:600}
.num{text-align:right;font-variant-numeric:tabular-nums}.bar{display:flex;height:30px;border-radius:6px;overflow:hidden;margin:8px 0;border:1px solid var(--line)}
.bar div{min-width:3px;display:flex;align-items:center;padding:0 6px;color:#fff;font-size:12px;overflow:hidden;white-space:nowrap}
.stats{display:flex;flex-wrap:wrap;gap:10px;margin:10px 0}.stat{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:8px 12px;min-width:120px}.stat b{display:block;font-size:20px}.stat span{color:var(--mut);font-size:12px}
ul{margin:6px 0;padding-left:20px}@media print{body{font-size:12px}main{padding:0}}
";

/// The report as an HTML page. `chain` is the receipts' check, if the receipts were supplied.
/// What the show's memory holds about this run, for the report.
pub struct MemoryView {
    /// Drafts from this run that wait for a person: (id, kind, text, in conflict).
    pub waiting: Vec<(String, String, String, bool)>,
    pub verified: usize,
    /// Notes this run would give rise to that have not been derived yet.
    pub not_yet_derived: usize,
    /// Memories anywhere in the file that disagree and wait for a person to settle.
    pub conflicts: usize,
}

/// Look at the memory without changing it.
pub fn memory_view(mem: &pchi_memory::Memory, records: &[EventRecord]) -> Result<MemoryView, String> {
    let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no recorded run")?;
    let source_ref = format!("run:{}", records[start].timestamp);
    let all = mem.list(&pchi_memory::Filter::default()).map_err(|e| e.0)?;
    let mine: Vec<_> = all.iter().filter(|r| r.source_ref == source_ref && r.status != pchi_memory::Status::Archived).collect();
    let not_yet = pchi_memory::derive::from_run(records)?.iter().filter(|n| !all.iter().any(|r| r.id == pchi_memory::make_id(n.kind, &n.key, &n.text, &n.show))).count();
    Ok(MemoryView {
        waiting: mine.iter().filter(|r| r.status == pchi_memory::Status::Draft).map(|r| (r.id.clone(), r.kind.name().to_string(), r.text.clone(), !r.conflicts_with.is_empty())).collect(),
        verified: mine.iter().filter(|r| r.status == pchi_memory::Status::Verified).count(),
        not_yet_derived: not_yet,
        conflicts: all.iter().filter(|r| r.status != pchi_memory::Status::Archived && !r.conflicts_with.is_empty()).count(),
    })
}

pub fn html(records: &[EventRecord], chain: Option<&Chain>, memory: Option<&MemoryView>) -> Result<String, String> {
    let view = recording::view(records)?;
    let replay = recording::verify(records)?;
    let start = records.iter().rposition(|r| r.event_type == recorder::RUN_START).ok_or("there is no recorded run")?;
    let run = &records[start..];
    let t0 = run[0].timestamp;
    let rel = |ts: i64| clock(ts - t0);
    let show = view["show"].as_str().unwrap_or("show");
    let length = view["length_ms"].as_i64().unwrap_or(0);

    // cues
    let mut drifts: Vec<(i64, &EventRecord)> = Vec::new();
    for r in run.iter().filter(|r| r.event_type == "show.cue") {
        drifts.push((r.payload["drift_ms"].as_i64().unwrap_or(0), r));
    }
    let mut abs: Vec<i64> = drifts.iter().map(|(d, _)| d.abs()).collect();
    abs.sort_unstable();
    let (med, p95, worst) = (percentile(&abs, 0.5), percentile(&abs, 0.95), abs.last().copied().unwrap_or(0));
    let late = abs.iter().filter(|d| **d > 50).count();

    // deliveries
    let mut by_status: std::collections::BTreeMap<String, usize> = Default::default();
    let mut problems: Vec<&EventRecord> = Vec::new();
    for r in run.iter().filter(|r| r.event_type == "show.delivery") {
        let s = r.payload["status"].as_str().unwrap_or("?").to_string();
        if matches!(s.as_str(), "failed" | "no-device" | "refused") {
            problems.push(r);
        }
        *by_status.entry(s).or_default() += 1;
    }

    // decisions, and what happened to the runtime itself
    let decisions: Vec<&EventRecord> = run.iter().filter(|r| r.event_type == "show.decision").collect();
    let failover: Vec<&&EventRecord> = decisions.iter().filter(|r| r.payload["rule"].as_str().is_some_and(|x| x.starts_with("failover."))).collect();
    let others: Vec<&&EventRecord> = decisions.iter().filter(|r| !r.payload["rule"].as_str().is_some_and(|x| x.starts_with("failover."))).collect();
    let resumed = run.iter().filter(|r| r.event_type == "show.resume").count();
    let fenced = run.iter().filter(|r| r.event_type == "show.fenced").count();
    let ignored = view["frames"].as_array().map_or(0, |f| f.iter().filter(|x| x["kind"] == "ignored").count());
    let held = run.iter().filter(|r| r.event_type == "show.held").count();
    let edits: Vec<&EventRecord> = run.iter().filter(|r| r.event_type == "show.edit").collect();

    let mut flags: Vec<(&str, String)> = Vec::new();
    if replay["matches"] != true {
        flags.push(("bad", format!("The recording does not replay: {}", replay["verdict"].as_str().unwrap_or(""))));
    }
    if !problems.is_empty() {
        flags.push(("bad", format!("{} cue(s) did not reach a device (failed or had no device).", problems.len())));
    }
    if worst > 250 {
        flags.push(("warn", format!("The worst cue was {worst} ms off its planned time.")));
    }
    if !failover.is_empty() || fenced > 0 {
        flags.push(("warn", "The runtime paused, was fenced or was taken over during the run: see 'Interruptions'.".into()));
    }
    if resumed > 0 {
        flags.push(("warn", format!("The runtime restarted and carried on {resumed} time(s).")));
    }
    if let Some(c) = chain.filter(|c| !c.ok) {
        flags.push(("bad", format!("The signed record does not verify: {}", c.text)));
    }
    if let Some(m) = memory.filter(|m| m.conflicts > 0) {
        flags.push(("warn", format!("{} memory(ies) disagree and wait for a person to settle them (Studio: Live, Memory).", m.conflicts)));
    }
    let verdict = if flags.is_empty() { "<p class=\"ok\"><b>Nothing to report.</b> The run replays exactly, every cue reached a device, and none was more than a quarter of a second late.</p>".to_string() } else { format!("<ul>{}</ul>", flags.iter().map(|(c, t)| format!("<li class=\"{c}\">{}</li>", esc(t))).collect::<String>()) };

    let mut h = String::new();
    h += &format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Run report: {}</title><style>{CSS}</style></head><body><main>", esc(show));
    h += &format!("<h1>{}</h1><p class=\"sub\">Run report. Started {}, ran {}.</p>", esc(show), when(t0), clock(length));
    h += &format!("<div class=\"verdict\">{verdict}</div>");
    h += &format!(
        "<div class=\"stats\"><div class=\"stat\"><b>{}</b><span>scenes visited</span></div><div class=\"stat\"><b>{}</b><span>cues fired</span></div><div class=\"stat\"><b>{} ms</b><span>median cue lateness</span></div><div class=\"stat\"><b>{} ms</b><span>worst cue lateness</span></div><div class=\"stat\"><b>{}</b><span>delivery problems</span></div><div class=\"stat\"><b>{}</b><span>live edits</span></div></div>",
        view["summary"]["scenes_visited"], drifts.len(), med, worst, problems.len(), edits.len()
    );

    // the path
    h += "<h2>Where the show went</h2>";
    let visits = view["scenes"].as_array().cloned().unwrap_or_default();
    let total = length.max(1) as f64;
    let mut names: Vec<String> = Vec::new();
    h += "<div class=\"bar\">";
    for v in &visits {
        let name = v["scene"].as_str().unwrap_or("?").to_string();
        if !names.contains(&name) {
            names.push(name.clone());
        }
        let (a, b) = (v["from_ms"].as_i64().unwrap_or(0), v["to_ms"].as_i64().unwrap_or(length));
        h += &format!("<div style=\"width:{:.2}%;background:var(--c{})\" title=\"{} for {}\">{}</div>", ((b - a).max(0) as f64 / total * 100.0).max(0.3), names.iter().position(|n| *n == name).unwrap_or(0) % 6, esc(&name), clock(b - a), esc(&name));
    }
    h += "</div><table><tr><th>Scene</th><th class=\"num\">From</th><th class=\"num\">For</th><th>Left by</th></tr>";
    for (i, v) in visits.iter().enumerate() {
        let (a, b) = (v["from_ms"].as_i64().unwrap_or(0), v["to_ms"].as_i64().unwrap_or(length));
        let left_by = visits.get(i + 1).and_then(|n| n["via"].as_str()).unwrap_or("(run ended)");
        h += &format!("<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td></tr>", esc(v["scene"].as_str().unwrap_or("?")), clock(a), clock(b - a), esc(left_by));
    }
    h += "</table>";
    if ignored > 0 || held > 0 {
        h += &format!("<p class=\"mut\">{ignored} input(s) did nothing in the scene they arrived in; {held} were held because the scene had not run its minimum.</p>");
    }

    // timing
    h += "<h2>Cue timing</h2>";
    if drifts.is_empty() {
        h += "<p class=\"mut\">No timeline cues fired in this run.</p>";
    } else {
        h += &format!("<p>{} cue(s) fired. Median {} ms from the planned time, 95% within {} ms, worst {} ms. {} were more than 50 ms off.</p>", drifts.len(), med, p95, worst, late);
        let mut worst_list = drifts.clone();
        worst_list.sort_by_key(|(d, _)| std::cmp::Reverse(d.abs()));
        h += "<table><tr><th>At</th><th>Scene</th><th>Cue</th><th>Kind</th><th class=\"num\">Planned</th><th class=\"num\">Off by</th></tr>";
        for (d, r) in worst_list.iter().take(8) {
            h += &format!("<tr><td>{}</td><td>{}</td><td>{}/{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{:+} ms</td></tr>", rel(r.timestamp), esc(r.payload["scene"].as_str().unwrap_or("")), esc(r.payload["lane"].as_str().unwrap_or("")), esc(r.payload["cue"].as_str().unwrap_or("")), esc(r.payload["kind"].as_str().unwrap_or("")), clock(r.payload["planned_ms"].as_i64().unwrap_or(0)), d);
        }
        h += "</table><p class=\"mut\">The eight furthest from plan. Lateness is how long after its planned time the runtime fired the cue, not how long the tool took.</p>";
    }

    // deliveries
    h += "<h2>Did the cues reach the devices?</h2>";
    if by_status.is_empty() {
        h += "<p class=\"mut\">No cue was routed to a device (the show has no rig, or no cue needed one).</p>";
    } else {
        h += &format!("<p>{}.</p>", by_status.iter().map(|(k, v)| format!("{v} {}", esc(k))).collect::<Vec<_>>().join(", "));
        if by_status.contains_key("dry-run") {
            h += "<p class=\"warn\">Some were <b>dry-run</b>: the runtime was not started with --send, so those cues were worked out but not sent.</p>";
        }
        if !problems.is_empty() {
            h += "<table><tr><th>At</th><th>Device</th><th>What</th><th>Why</th></tr>";
            for r in problems.iter().take(30) {
                h += &format!("<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"bad\">{}</td></tr>", rel(r.timestamp), esc(r.payload["label"].as_str().or(r.payload["dept"].as_str()).unwrap_or("?")), esc(r.payload["kind"].as_str().unwrap_or("")), esc(r.payload["error"].as_str().unwrap_or(r.payload["status"].as_str().unwrap_or(""))));
            }
            h += "</table>";
            if problems.len() > 30 {
                h += &format!("<p class=\"mut\">…and {} more.</p>", problems.len() - 30);
            }
        }
    }

    // interruptions
    h += "<h2>Interruptions</h2>";
    if failover.is_empty() && fenced == 0 && resumed == 0 {
        h += "<p class=\"ok\">None: this runtime ran the whole show without pausing, restarting or being replaced.</p>";
    } else {
        h += "<table><tr><th>At</th><th>What happened</th></tr>";
        let mut rows: Vec<(i64, String)> = Vec::new();
        for r in run.iter().filter(|r| r.event_type == "show.resume") {
            rows.push((r.timestamp, format!("The runtime restarted and carried on after {:.0} s down.", r.payload["downtime_ms"].as_f64().unwrap_or(0.0) / 1000.0)));
        }
        for r in run.iter().filter(|r| r.event_type == "show.fenced") {
            rows.push((r.timestamp, format!("{} took over the show; this runtime stopped.", r.payload["by"].as_str().unwrap_or("another runtime"))));
        }
        for r in &failover {
            rows.push((r.timestamp, r.payload["text"].as_str().unwrap_or("").to_string()));
        }
        rows.sort_by_key(|(t, _)| *t);
        for (t, text) in rows {
            h += &format!("<tr><td>{}</td><td>{}</td></tr>", rel(t), esc(&text));
        }
        h += "</table>";
    }

    // edits
    h += "<h2>Live edits</h2>";
    if edits.is_empty() {
        h += "<p class=\"mut\">The show was not edited while it ran.</p>";
    } else {
        h += "<table><tr><th>At</th><th>Change</th></tr>";
        for r in &edits {
            let changes: Vec<&str> = r.payload["changes"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).collect();
            h += &format!("<tr><td>{}</td><td>{}{}</td></tr>", rel(r.timestamp), if r.payload["undo"] == true { "undone: " } else { "" }, if changes.is_empty() { "re-sent unchanged".to_string() } else { esc(&changes.join("; ")) });
        }
        h += "</table>";
    }

    // people
    h += "<h2>Who decided what</h2>";
    if others.is_empty() {
        h += "<p class=\"mut\">No decisions were recorded (no roster, or nobody signed in).</p>";
    } else {
        h += "<table><tr><th>At</th><th>Who</th><th>Result</th><th>What</th></tr>";
        for r in others.iter().take(60) {
            let denied = r.payload["gate"].as_str().is_some_and(|g| g != "ALLOW");
            h += &format!("<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td><td>{}</td></tr>", rel(r.timestamp), esc(r.payload["by"].as_str().unwrap_or("")), if denied { "bad" } else { "ok" }, esc(r.payload["gate"].as_str().unwrap_or("")), esc(r.payload["text"].as_str().unwrap_or("")));
        }
        h += "</table>";
        if others.len() > 60 {
            h += &format!("<p class=\"mut\">The first 60 of {}; the recording holds the rest.</p>", others.len());
        }
    }

    // signed record
    h += "<h2>The signed record</h2>";
    match chain {
        Some(c) if c.ok && c.count > 0 => h += &format!("<p class=\"ok\"><b>Holds together.</b> {}</p><p class=\"mut\">This checks that the receipts link and verify against the key in the export. To be sure of who signed, compare that key with the one recorded when the show was set up.</p>", esc(&c.text)),
        Some(c) if c.ok => h += &format!("<p class=\"mut\">{}</p>", esc(&c.text)),
        Some(c) => h += &format!("<p class=\"bad\"><b>Does not verify.</b> {}</p>", esc(&c.text)),
        None => h += "<p class=\"mut\">No signed receipts were supplied with this report (pass the governance export).</p>",
    }

    // memory
    h += "<h2>Memory from this run</h2>";
    match memory {
        None => h += "<p class=\"mut\">Memory is off, or was not given to this report (start the runtime with --memory FILE, or give pchi-run-report --memory FILE).</p>",
        Some(m) => {
            h += &format!("<p>{} draft(s) from this run wait for a person to verify; {} already verified{}.</p>", m.waiting.len(), m.verified, if m.not_yet_derived > 0 { format!("; {} more could be derived from the run but have not been (Studio: Live, Memory, Derive from this run)", m.not_yet_derived) } else { String::new() });
            if !m.waiting.is_empty() {
                h += "<table><tr><th>Kind</th><th>What the run says</th><th>Id</th></tr>";
                for (id, kind, text, conflict) in m.waiting.iter().take(30) {
                    h += &format!("<tr><td>{}</td><td>{}{}</td><td class=\"mut\">{}</td></tr>", esc(kind), esc(text), if *conflict { " <b class=\"warn\">(in conflict)</b>" } else { "" }, esc(id));
                }
                h += "</table><p class=\"mut\">Drafts are what the log says happened, not what anyone has decided is worth remembering. They count for little until a person verifies them.</p>";
            }
        }
    }

    // replay
    h += "<h2>Does the recording hold up?</h2>";
    h += &format!("<p class=\"{}\">{}</p>", if replay["matches"] == true { "ok" } else { "bad" }, esc(replay["verdict"].as_str().unwrap_or("")));

    h += "<h2>What this report does not show</h2><ul class=\"mut\"><li>Whether anything looked or sounded right. \"Sent\" means the message left the runtime; whether the tool did the right thing is for the gear check and for people in the room.</li><li>Anything the runtime was not told: cues operated by hand on a desk, or inputs that never reached it.</li><li>Names and reasons are as typed by the people and agents; the recording holds them, so treat this report like the show file.</li></ul>";
    h += "</main></body></html>";
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: i64, ts: i64, kind: &str, payload: Value) -> EventRecord {
        EventRecord { id, timestamp: ts, source: "t".into(), target: None, event_type: kind.into(), payload }
    }

    fn run() -> Vec<EventRecord> {
        let chart = json!({"id": "demo", "name": "Demo", "version": "0.1.0", "initial": "PRE", "states": [{"id": "PRE"}, {"id": "RUN"}], "transitions": [{"source": "PRE", "target": "RUN", "event": "GO"}]});
        vec![
            rec(1, 1_000_000, "run.start", json!({"chart": chart, "context": {}})),
            rec(2, 1_010_000, "show.input", json!({"event": "GO", "from": "PRE", "to": "RUN", "applied": true})),
            rec(3, 1_011_000, "show.cue", json!({"scene": "RUN", "lane": "l", "cue": "c1", "kind": "lighting.look", "planned_ms": 1000, "actual_ms": 1400, "drift_ms": 400})),
            rec(4, 1_011_000, "show.delivery", json!({"status": "failed", "label": "Desk", "kind": "lighting.look", "error": "no route <to> host"})),
            rec(5, 1_012_000, "show.decision", json!({"rule": "failover.fenced", "gate": "ALLOW", "by": "", "text": "nova-b took over the show"})),
            rec(6, 1_020_000, "show.decision", json!({"rule": "poll.open", "gate": "ALLOW", "by": "Alex", "text": "opened poll"})),
        ]
    }

    #[test]
    fn the_report_says_what_went_wrong_and_escapes_what_it_quotes() {
        let page = html(&run(), None, None).unwrap();
        assert!(page.contains("<title>Run report: demo</title>"));
        assert!(page.contains("did not reach a device"), "a failed delivery should be flagged");
        assert!(page.contains("400 ms off"), "the late cue should be flagged");
        assert!(page.contains("took over the show"), "the takeover should be listed");
        assert!(page.contains("no route &lt;to&gt; host") && !page.contains("<to>"), "quoted text must be escaped");
        assert!(page.contains("Alex") && page.contains("No signed receipts were supplied"));
        assert!(!page.contains("<script"), "the report carries no scripts");
    }

    #[test]
    fn a_clean_run_says_so() {
        let mut r = run();
        r.truncate(2);
        let page = html(&r, None, None).unwrap();
        assert!(page.contains("Nothing to report"), "{page}");
        assert!(page.contains("None: this runtime ran the whole show"));
    }

    #[test]
    fn the_report_says_what_the_run_left_in_memory_for_a_person_to_verify() {
        let mem = pchi_memory::Memory::in_memory().unwrap();
        let view = memory_view(&mem, &run()).unwrap();
        assert_eq!((view.waiting.len(), view.not_yet_derived), (0, 3), "nothing derived yet: the report says what could be");
        pchi_memory::derive::derive_into(&mem, &run(), &pchi_memory::Who::agent("derive"), 1).unwrap();
        let view = memory_view(&mem, &run()).unwrap();
        assert_eq!((view.waiting.len(), view.verified, view.not_yet_derived), (3, 0, 0));
        let id = view.waiting[0].0.clone();
        mem.verify(&pchi_memory::Who::person("Alex"), &id, 2).unwrap();
        let view = memory_view(&mem, &run()).unwrap();
        assert_eq!((view.waiting.len(), view.verified), (2, 1));
        let page = html(&run(), None, Some(&view)).unwrap();
        assert!(page.contains("Memory from this run") && page.contains("2 draft(s) from this run wait for a person to verify; 1 already verified"), "{page}");
        assert!(!html(&run(), None, None).unwrap().contains("wait for a person to verify"));
    }

    #[test]
    fn a_broken_chain_is_called_out() {
        let bad = Chain { count: 2, ok: false, text: "receipt 2 does not follow the one before it".into() };
        let page = html(&run(), Some(&bad), None).unwrap();
        assert!(page.contains("The signed record does not verify"));
        let none = check_chain(&json!({"receipts": [], "count": 0, "verifying_key": null, "note": "receipts are off"}));
        assert!(none.ok && none.count == 0);
    }
}
