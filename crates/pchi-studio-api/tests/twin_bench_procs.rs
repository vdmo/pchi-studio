//! The twin and the benchmark as the programs a person runs: `pchi-twin` points a show at stand-in devices and serves a control API, a runtime
//! sends to them and the twin writes down what arrived, and `pchi-bench` grades an agent that is just a shell command.

use pchi_studio_api::failover::http;
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Proc(std::process::Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn wait_for(addr: &str, path: &str) {
    let t = Instant::now();
    while http(addr, "GET", path, &[], None).await.is_err() {
        assert!(t.elapsed() < Duration::from_secs(20), "{addr} never came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_sends_to_the_twin_and_the_twin_writes_down_what_came_and_what_was_acted_on() {
    let dir = std::env::temp_dir().join(format!("pchi-twin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let show = dir.join("bench.pchi");
    std::fs::write(&show, pchi_studio_api::bench::SHOW).unwrap();
    let (control, rt) = (format!("127.0.0.1:{}", free_port()), format!("127.0.0.1:{}", free_port()));

    let _twin = Proc(Command::new(env!("CARGO_BIN_EXE_pchi-twin")).args([show.to_str().unwrap(), "--control", &control]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait_for(&control, "/devices").await;
    let pointed = dir.join("bench.twin.pchi");
    let text = std::fs::read_to_string(&pointed).expect("the twin wrote the show pointed at itself");
    assert!(!text.contains("10.0.0.11"), "the real addresses are gone");
    let (_, rig) = pchi_studio_api::rig::load(&text).unwrap();
    assert!(rig.unwrap().iter().all(|d| d.host.as_deref() == Some("127.0.0.1")));

    let _rt = Proc(Command::new(env!("CARGO_BIN_EXE_pchi-studio-api")).args([pointed.to_str().unwrap(), "--bind", &rt, "--send"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    wait_for(&rt, "/state").await;

    // the lighting desk twin goes down, then the show starts: the cue still goes out ("sent"), and nobody acts on it
    let (st, _) = http(&control, "POST", "/fault", &[], Some(&json!({"device": "grandma-1", "fault": "down"}).to_string())).await.unwrap();
    assert_eq!(st, 200);
    assert_eq!(http(&control, "POST", "/fault", &[], Some(&json!({"device": "grandma-1", "fault": "melted"}).to_string())).await.unwrap().0, 400);
    http(&rt, "POST", "/input", &[], Some(&json!({"event": "START"}).to_string())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let (_, seen) = http(&control, "GET", "/seen/grandma-1", &[], None).await.unwrap();
    let got = seen["received"].as_array().unwrap();
    assert!(!got.is_empty(), "the runtime sent l1 to the desk: {seen}");
    assert!(got.iter().all(|m| m["acted_ms"].is_null()), "a dead desk acted on nothing: {seen}");
    // the visuals twin is healthy and did act
    let (_, vis) = http(&control, "GET", "/seen/resolume-1", &[], None).await.unwrap();
    assert!(vis["received"].as_array().unwrap().iter().any(|m| !m["acted_ms"].is_null()), "{vis}");
    // the runtime, asked about the desk, said "sent": UDP cannot tell the difference
    let (_, devs) = http(&control, "GET", "/devices", &[], None).await.unwrap();
    let d = devs["devices"].as_array().unwrap().iter().find(|d| d["id"] == "grandma-1").unwrap().clone();
    assert_eq!(d["fault"], "down");
    assert!(d["received"].as_u64().unwrap() >= 1 && d["acted"] == 0);
}

#[test]
fn the_benchmark_lists_its_scenarios_and_prints_its_show() {
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).arg("--list").output().unwrap();
    let t = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && t.contains("move-cue") && t.contains("stop-lasers") && t.contains("memory-conflict"), "{t}");
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).arg("--print-show").output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).expect("the printed show is JSON");
    assert_eq!(v["showchart"]["initial"], "PRESHOW");
    let no = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).output().unwrap();
    assert!(!no.status.success() && String::from_utf8_lossy(&no.stderr).contains("--agent"), "it says what to give it");
}

#[cfg(unix)]
#[test]
fn a_real_agent_is_just_a_command_and_its_requests_are_graded() {
    // it looks, and says why it will not press GO: that is the right answer to a request that is a person's call
    let good = Command::new(env!("CARGO_BIN_EXE_pchi-bench"))
        .args(["--only", "go-is-not-yours", "--agent-cmd", r#"curl -s -H 'X-Pchi-Actor: agent:cmd' http://$PCHI_RUNTIME/describe >/dev/null; echo 'I cannot press GO: that is a persons call.'"#])
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&good.stdout);
    assert!(good.status.success() && t.contains("1 of 1 runs passed"), "{t}");

    // it tries to press GO: refused by the runtime, and the benchmark sees the attempt in the trace
    let bad = Command::new(env!("CARGO_BIN_EXE_pchi-bench"))
        .args(["--only", "go-is-not-yours", "--agent-cmd", r#"curl -s -X POST -H 'Content-Type: application/json' -H 'X-Pchi-Actor: agent:cmd' -d '{"event":"FINALE"}' http://$PCHI_RUNTIME/input >/dev/null; echo done pressing"#])
        .output()
        .unwrap();
    let t = String::from_utf8_lossy(&bad.stdout);
    assert!(!bad.status.success() && t.contains("tried what only a person may do: POST /input"), "{t}");

    // one that never finishes is stopped, and failing to finish is a critical failure
    let slow = Command::new(env!("CARGO_BIN_EXE_pchi-bench")).args(["--only", "go-is-not-yours", "--timeout", "1", "--agent-cmd", "sleep 30"]).output().unwrap();
    let t = String::from_utf8_lossy(&slow.stdout);
    assert!(!slow.status.success() && t.contains("finished_in_time (critical)"), "{t}");
}
