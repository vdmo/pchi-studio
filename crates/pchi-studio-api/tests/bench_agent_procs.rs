//! The Claude Code launcher, repeats, cost and evidence, end to end: `pchi-bench` runs `bench/agents/claude.sh`, which runs a stand-in for the `claude`
//! command (no model involved), and the run is repeated, costed, written out as evidence, verified, and caught when altered.

#![cfg(unix)]

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn agents() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/agents")
}

#[test]
fn claude_sh_runs_an_agent_repeats_it_costs_it_and_leaves_checkable_evidence() {
    let dir = std::env::temp_dir().join(format!("pchi-claude-sh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("claude");
    std::fs::copy(agents().join("fake-claude.sh"), &fake).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launcher = agents().join("claude.sh");
    let ev = dir.join("evidence");
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_pchi-bench"))
            .args(["--only", "stop-lasers,go-is-not-yours", "--repeat", "2", "--agent-cmd", &format!("bash {}", launcher.display())])
            .args(extra)
            .env("PCHI_CLAUDE_BIN", &fake)
            .env("PCHI_MODEL", "label-model")
            .output()
            .unwrap()
    };
    let out = run(&["--evidence", ev.to_str().unwrap()]);
    let t = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{t}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(t.contains("4 of 4 runs passed") && t.contains("Pass rate") && t.contains("$0.05"), "four runs at $0.0123 each:\n{t}");

    // one evidence file per run, and an index, each with its receipt
    let mut names: Vec<String> = std::fs::read_dir(&ev).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names.len(), 5, "{names:?}");
    let go2 = names.iter().find(|n| n.contains("go-is-not-yours__02")).unwrap();
    let case: Value = serde_json::from_str(&std::fs::read_to_string(ev.join(go2)).unwrap()).unwrap();
    assert_eq!(case["case_id"], "PCHI-BENCH-go-is-not-yours");
    assert_eq!(case["negative_test"], true, "declining is a negative test");
    assert_eq!(case["execution"]["model"], "fake-model", "the model the agent reported wins over the label");
    assert_eq!(case["execution"]["usage"]["tokens_out"], 340);
    assert_eq!(case["trace"][0]["path"], "/describe");
    assert_eq!(case["execution"]["binary_checksum"].as_str().unwrap().len(), 64);

    let idx = ev.join("index.json");
    let ok = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).args(["--verify", idx.to_str().unwrap()]).output().unwrap();
    assert!(ok.status.success() && String::from_utf8_lossy(&ok.stdout).contains("4 case file(s) match"), "{}", String::from_utf8_lossy(&ok.stdout));

    // evidence is never overwritten
    let again = run(&["--evidence", ev.to_str().unwrap()]);
    assert!(!again.status.success() && String::from_utf8_lossy(&again.stderr).contains("never overwritten"), "{}", String::from_utf8_lossy(&again.stderr));

    // changing a file after the fact no longer matches its receipt
    let target = ev.join(names.iter().find(|n| n.contains("stop-lasers__01")).unwrap());
    let text = std::fs::read_to_string(&target).unwrap().replace("\"pass\": true", "\"pass\": false");
    std::fs::write(&target, text).unwrap();
    let bad = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).args(["--verify", idx.to_str().unwrap()]).output().unwrap();
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stdout).contains("no longer matches its receipt"), "{}", String::from_utf8_lossy(&bad.stdout));
}

#[test]
fn the_launcher_says_what_is_wrong_when_it_cannot_run() {
    let launcher = agents().join("claude.sh");
    let out = Command::new("bash").arg(&launcher).output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("PCHI_RUNTIME"), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new("bash").arg(&launcher).env("PCHI_RUNTIME", "127.0.0.1:1").env("PCHI_TASK", "t").env("PCHI_CLAUDE_BIN", "definitely-not-installed").output().unwrap();
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("install Claude Code"));
}
