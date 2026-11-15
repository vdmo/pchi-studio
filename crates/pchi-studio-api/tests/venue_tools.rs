//! The venue-day tools end to end, on loopback: a cue sheet imported, the imported show run and recorded, the run report built from the live runtime and from the file,
//! and the venue-day driver collecting it into a folder.

use pchi_studio_api::failover::http;
use serde_json::Value;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Proc(std::process::Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const SHEET: &str = "number,time,scene,action,value,label\n1,0:00,Opening,light,warm,House to half\n2,0:00.4,Opening,light,blue,Blue wash\n3,0:00.8,Opening,Fireworks,big,Pyro\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cue_sheet_is_imported_run_recorded_and_reported() {
    let dir = std::env::temp_dir().join(format!("pchi-venue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sheet = dir.join("gala.csv");
    std::fs::write(&sheet, SHEET).unwrap();
    let show = dir.join("gala.pchi");

    // import
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-import")).args([sheet.to_str().unwrap(), "--out", show.to_str().unwrap()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Fireworks") && text.contains("nothing will be sent"), "{text}");
    assert!(show.exists());
    // a QLab 5 file is refused in words
    let q5 = dir.join("x.qlab5");
    std::fs::write(&q5, "{}").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-import")).arg(&q5).output().unwrap();
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("QLab 5"));

    // run it, recording
    let port = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let rec = dir.join("run.jsonl");
    let _rt = Proc(
        Command::new(env!("CARGO_BIN_EXE_pchi-studio-api"))
            .args([show.to_str().unwrap(), "--db", dir.join("a.db").to_str().unwrap(), "--bind", &format!("127.0.0.1:{port}"), "--record", rec.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let addr = format!("127.0.0.1:{port}");
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end && http(&addr, "GET", "/role", &[], None).await.is_err() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(http(&addr, "POST", "/input", &[], Some("{\"event\":\"GO\"}")).await.unwrap().0, 200);
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // the live page
    let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    s.write_all(format!("GET /run-report HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).await.unwrap();
    assert!(body.contains("200 OK") && body.contains("content-type: text/html"), "{}", &body[..body.len().min(300)]);
    assert!(body.contains("<h1>gala</h1>") && body.contains("Where the show went") && body.contains("Opening"), "{body}");
    assert!(body.contains("3 cue(s)") || body.contains("<b>3</b><span>cues fired"), "all three cues fired: {body}");

    // the same from the file, with the command
    let page = dir.join("report.html");
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-run-report")).args([rec.to_str().unwrap(), "--out", page.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.contains("<h1>gala</h1>") && html.contains("The signed record") && !html.contains("<script"), "{html}");

    // the driver collects it
    let folder = dir.join("results");
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-venueday"))
        .args([show.to_str().unwrap(), "--runtime", &addr, "--recording", rec.to_str().unwrap(), "--skip", "drill,gear,failover", "--out", folder.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let index = std::fs::read_to_string(folder.join("index.md")).unwrap_or_else(|_| format!("no index: {text}"));
    assert!(index.contains("doors check") && index.contains("run report of the rehearsal") && index.contains("Nothing was listed as untried"), "{index}");
    assert!(folder.join("run-report.html").exists() && folder.join("preflight.json").exists());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("preflight.json")).unwrap()).unwrap();
    assert!(v["checks"].is_array());
}
