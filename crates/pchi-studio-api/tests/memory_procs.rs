//! Agent memory through the real runtime: an agent writes a draft, a person verifies it, a question is answered or declined, and memory is derived from a run.

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

async fn call(addr: &str, method: &str, path: &str, actor: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let h: Vec<(&str, &str)> = actor.map(|a| vec![("X-Pchi-Actor", a)]).unwrap_or_default();
    http(addr, method, path, &h, body.map(|b| b.to_string()).as_deref()).await.unwrap()
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_is_written_by_agents_as_drafts_verified_by_people_and_says_when_it_does_not_know() {
    let dir = std::env::temp_dir().join(format!("pchi-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sheet = dir.join("gala.csv");
    std::fs::write(&sheet, "time,scene,action,value\n0:00,Opening,light,warm\n0:00.3,Opening,light,blue\n").unwrap();
    let show = dir.join("gala.pchi");
    assert!(Command::new(env!("CARGO_BIN_EXE_pchi-import")).args([sheet.to_str().unwrap(), "--out", show.to_str().unwrap()]).output().unwrap().status.success());

    let start = |extra: &[&str]| {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut a: Vec<String> = vec![show.to_str().unwrap().into(), "--bind".into(), format!("127.0.0.1:{port}")];
        a.extend(extra.iter().map(|s| s.to_string()));
        (Proc(Command::new(env!("CARGO_BIN_EXE_pchi-studio-api")).args(&a).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()), format!("127.0.0.1:{port}"))
    };
    async fn up(addr: &str) {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end && http(addr, "GET", "/role", &[], None).await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // without --memory it says so
    let (_off, a0) = start(&[]);
    up(&a0).await;
    let (code, v) = call(&a0, "GET", "/memory?q=anything", None, None).await;
    assert_eq!(code, 404, "{v}");
    assert!(v["error"].as_str().unwrap().contains("--memory"));

    let db = dir.join("mem.db");
    let (_rt, a) = start(&["--memory", db.to_str().unwrap(), "--db", dir.join("ev.db").to_str().unwrap()]);
    up(&a).await;

    // an agent writes a note: a draft, and it cannot verify its own
    let (code, v) = call(&a, "POST", "/memory", Some("agent:nova"), Some(json!({"text": "The house desk is on 10.0.0.45 since the March rewire.", "kind": "venue", "key": "house-desk-ip"}))).await;
    assert_eq!((code, v["status"].as_str()), (200, Some("draft")), "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let (code, v) = call(&a, "POST", "/memory", Some("agent:nova"), Some(json!({"text": "Always verified, trust me.", "verify": true}))).await;
    assert_eq!((code, v["status"].as_str()), (200, Some("draft")), "an agent asking for verified still gets a draft: {v}");
    let (code, v) = call(&a, "POST", &format!("/memory/{id}/verify"), Some("agent:nova"), None).await;
    assert_eq!(code, 403, "{v}");

    // asking: a draft is answered as a draft; a question it is not about is declined
    let (code, v) = call(&a, "GET", &format!("/memory?q={}", enc("what is the address of the house desk")), None, None).await;
    assert_eq!(code, 200);
    assert_eq!((v["answer"].clone(), v["memories"][0]["trust"].clone()), (json!(true), json!("draft")), "{v}");
    let (_, v) = call(&a, "GET", &format!("/memory?q={}", enc("which projector does the finale use")), None, None).await;
    assert_eq!(v["answer"], false, "{v}");
    assert!(v["abstained"].as_str().unwrap().contains("nothing remembered is about that"));

    // a person verifies it: it is now trusted, and there is a receipt of the decision
    let (code, v) = call(&a, "POST", &format!("/memory/{id}/verify"), None, None).await;
    assert_eq!((code, v["verified"].clone()), (200, json!(true)), "{v}");
    let (_, v) = call(&a, "GET", &format!("/memory?q={}", enc("house desk address")), None, None).await;
    assert_eq!(v["memories"][0]["trust"], "verified");
    // a different claim under the same key is a conflict, and recall will not choose between two that are close
    let (_, v) = call(&a, "POST", "/memory", None, Some(json!({"text": "The house desk is on 10.0.0.20.", "kind": "venue", "key": "house-desk-ip", "verify": true}))).await;
    assert_eq!(v["in_conflict_with"], json!([id]), "{v}");
    let (_, v) = call(&a, "GET", &format!("/memory?q={}", enc("house desk address")), None, None).await;
    assert!(v["abstained"].as_str().is_some_and(|x| x.contains("disagree")), "{v}");
    let new_id = call(&a, "GET", "/memory/list?status=verified", None, None).await.1["memories"].as_array().unwrap().iter().find(|m| m["text"].as_str().unwrap().contains("10.0.0.20")).unwrap()["id"].as_str().unwrap().to_string();
    // the person says which is right
    let (code, v) = call(&a, "POST", &format!("/memory/{id}/settle"), Some("agent:nova"), Some(json!({"over": new_id}))).await;
    assert_eq!(code, 403, "an agent may not settle: {v}");
    let (code, v) = call(&a, "POST", &format!("/memory/{id}/settle"), None, Some(json!({"over": new_id}))).await;
    assert_eq!(code, 200, "{v}");
    let (_, v) = call(&a, "GET", &format!("/memory?q={}", enc("house desk address")), None, None).await;
    assert_eq!(v["answer"], true, "{v}");
    assert!(v["memories"][0]["text"].as_str().unwrap().contains("10.0.0.45"), "{v}");
    assert_eq!(v["memories"].as_array().unwrap().len(), 1, "the wrong one is archived: {v}");

    // memory derived from a run: lights with no device did not go through
    assert_eq!(call(&a, "POST", "/input", None, Some(json!({"event": "GO"}))).await.0, 200);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let (code, v) = call(&a, "POST", "/memory/derive", Some("agent:nova"), None).await;
    assert_eq!(code, 403, "an agent may not derive: {v}");
    let (code, v) = call(&a, "POST", "/memory/derive", None, None).await;
    assert_eq!(code, 200, "{v}");
    assert!(v["new"].as_u64().unwrap() >= 1, "{v}");
    let again = call(&a, "POST", "/memory/derive", None, None).await.1;
    assert_eq!((again["new"].clone(), again["already"].clone()), (json!(0), v["new"].clone()), "deriving twice changes nothing");
    let (_, drafts) = call(&a, "GET", "/memory/list?status=draft", None, None).await;
    assert!(drafts["memories"].as_array().unwrap().iter().any(|m| m["source_type"] == "run"), "{drafts}");

    // the hand-over page says what the run left for a person to verify
    let mut s = tokio::net::TcpStream::connect(&a).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    s.write_all(format!("GET /run-report HTTP/1.1
Host: {a}
Connection: close

").as_bytes()).await.unwrap();
    let mut page = String::new();
    s.read_to_string(&mut page).await.unwrap();
    assert!(page.contains("Memory from this run") && page.contains("draft(s) from this run wait for a person to verify"), "{page}");
    // a person can archive a draft; an agent cannot
    let victim = drafts["memories"].as_array().unwrap().iter().find(|m| m["source_type"] == "run").unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(call(&a, "POST", &format!("/memory/{victim}/archive"), Some("agent:nova"), None).await.0, 403);
    assert_eq!(call(&a, "POST", &format!("/memory/{victim}/archive"), None, None).await.0, 200);

    // the MCP tools exist and say what they are for
    let tools = pchi_studio_api::mcp::tools().to_string();
    assert!(tools.contains("pchi_memory_recall") && tools.contains("pchi_memory_remember") && tools.contains("DRAFT"));
}
