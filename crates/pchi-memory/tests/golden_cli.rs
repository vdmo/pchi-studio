//! The golden-set workflow from the command line: start one from a recording, check it, evaluate it, and hold a baseline.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pchi-memory")).args(args).output().unwrap();
    (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

const RUN: &str = r#"{"format":"pchi-runlog/1"}
{"id":1,"ts":1800000000000,"source":"t","target":null,"type":"run.start","payload":{"chart":{"id":"gala"},"context":{}}}
{"id":2,"ts":1800000001000,"source":"t","target":null,"type":"show.delivery","payload":{"status":"failed","label":"House desk","dept":"lighting","error":"no route to host"}}
{"id":3,"ts":1800000002000,"source":"t","target":null,"type":"show.cue","payload":{"scene":"OPENING","lane":"haze","cue":"c4","kind":"lighting.look","planned_ms":4000,"actual_ms":4400,"drift_ms":400}}
{"id":4,"ts":1800000003000,"source":"t","target":null,"type":"show.resume","payload":{"downtime_ms":12000}}
"#;

#[test]
fn a_golden_set_is_started_from_a_recording_checked_evaluated_and_held_to_a_baseline() {
    let dir = std::env::temp_dir().join(format!("pchi-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (rec, golden, base) = (dir.join("run.jsonl"), dir.join("golden.json"), dir.join("base.json"));
    std::fs::write(&rec, RUN).unwrap();

    let (ok, out) = run(&["golden-init", rec.to_str().unwrap(), "--out", golden.to_str().unwrap()]);
    assert!(ok && out.contains("3 memory(ies)"), "{out}");
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&golden).unwrap()).unwrap();
    assert_eq!(g["questions"].as_array().unwrap().len(), 0, "the questions are for a person to write");
    assert!(g["memories"][0]["text"].as_str().unwrap().contains("gala"));

    // an empty question list is called out
    let (ok, out) = run(&["golden-check", golden.to_str().unwrap(), "--strict"]);
    assert!(!ok && out.contains("only 0 answerable") && out.contains("no question expects"), "{out}");

    // a person writes questions: some in other words, some it should decline
    let mut g = g;
    g["questions"] = serde_json::json!([
        {"q": "why did the house desk fail", "expect": ["m1"]},
        {"q": "which cue ran late", "expect": ["m2"]},
        {"q": "did the runtime restart", "expect": ["m3"]},
        {"q": "who is the caterer", "abstain": true}
    ]);
    std::fs::write(&golden, g.to_string()).unwrap();
    let (ok, out) = run(&["golden-check", golden.to_str().unwrap()]);
    assert!(ok && out.contains("only 3 answerable"), "warnings do not fail without --strict: {out}");

    let (ok, out) = run(&["eval", golden.to_str().unwrap(), "--save-baseline", base.to_str().unwrap()]);
    assert!(ok && out.contains("baseline saved"), "{out}");
    let (ok, out) = run(&["eval", golden.to_str().unwrap(), "--baseline", base.to_str().unwrap()]);
    assert!(ok, "the same set against its own baseline holds: {out}");

    // a worse memory is caught: make the baseline claim more than can be met
    let mut b: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&base).unwrap()).unwrap();
    b["shipped"]["correct"] = serde_json::json!(99);
    std::fs::write(&base, b.to_string()).unwrap();
    let (ok, out) = run(&["eval", golden.to_str().unwrap(), "--baseline", base.to_str().unwrap()]);
    assert!(!ok && out.contains("REGRESSION"), "{out}");
}
