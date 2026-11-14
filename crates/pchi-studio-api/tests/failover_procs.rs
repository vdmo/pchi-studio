//! Failover between real processes: two runtimes, a fake lighting desk they both send to, and a link between them that can be cut.
//!
//! These start the actual `pchi-studio-api` binary twice on loopback. They are what loopback can honestly say about failover: that a runtime
//! killed outright is replaced, that the new one carries the show, and what the two do when the link between them is cut while both can still reach the desk
//! (the case `docs/failover.md` warns about). They are not two machines in a venue.

use pchi_studio_api::failover::http;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, UdpSocket};

const KEY: &str = "fk-test-key-0123456789";

struct Proc(Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(args: &[String]) -> Proc {
    Proc(Command::new(env!("CARGO_BIN_EXE_pchi-studio-api")).args(args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start the runtime"))
}

async fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port()
}

/// A TCP forwarder in front of the primary that can be cut: while cut, every connection is dropped at once.
struct Link {
    cut: Arc<AtomicBool>,
    port: u16,
}

async fn link_to(target: u16) -> Link {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let cut = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cut);
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else { break };
            if flag.load(Ordering::SeqCst) {
                continue; // dropped: the other end sees the connection close with nothing
            }
            tokio::spawn(async move {
                if let Ok(mut up) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await {
                    let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
                }
            });
        }
    });
    Link { cut, port }
}

/// A fake lighting desk: remembers when each datagram came and from which port (each runtime has its own socket).
fn desk(sock: UdpSocket) -> Arc<Mutex<Vec<(Instant, u16)>>> {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let h = Arc::clone(&heard);
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = sock.recv_from(&mut buf).await {
            if buf[..n].starts_with(b"/pchi/lighting") {
                h.lock().unwrap().push((Instant::now(), from.port()));
            }
        }
    });
    heard
}

fn sources_since(heard: &Arc<Mutex<Vec<(Instant, u16)>>>, t: Instant) -> BTreeSet<u16> {
    heard.lock().unwrap().iter().filter(|(at, _)| *at >= t).map(|(_, p)| *p).collect()
}

async fn role(port: u16) -> Option<Value> {
    match http(&format!("127.0.0.1:{port}"), "GET", "/role", &[], None).await {
        Ok((200, v)) => Some(v),
        _ => None,
    }
}

async fn until<F: FnMut() -> bool>(secs: u64, mut ok: F) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    ok()
}

async fn until_role(port: u16, secs: u64, want: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if role(port).await.is_some_and(|r| r["role"] == want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// A show that sends a lighting cue to the desk every 400 ms for ninety seconds, once START is pressed.
fn show_file(dir: &std::path::Path, desk_port: u16) -> String {
    let cues: Vec<Value> = (1..=225).map(|i| json!({"id": format!("c{i}"), "at_ms": i * 400, "action": {"kind": "lighting.look", "payload": {"look": "warm"}}})).collect();
    let file = json!({
        "version": "1.0", "project": "test", "name": "split", "edges": [], "timeline": [], "assets": [],
        "nodes": [{"id": "desk", "type": "lighting", "label": "Desk", "position": {"x": 0, "y": 0}, "params": {"host": "127.0.0.1", "port": desk_port}}],
        "showchart": {"id": "split", "name": "Split", "version": "0.1.0", "initial": "PRESHOW",
            "states": [{"id": "PRESHOW"}, {"id": "RUN", "timeline": {"duration_ms": 90000, "lanes": [{"id": "lighting", "name": "Lights", "dept": "lighting", "cues": cues}]}}],
            "transitions": [{"source": "PRESHOW", "target": "RUN", "event": "START"}]},
    });
    let path = dir.join("show.pchi");
    std::fs::write(&path, file.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

fn args(show: &str, db: &std::path::Path, port: u16, extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = vec![show.into(), "--db".into(), db.to_string_lossy().into_owned(), "--bind".into(), format!("127.0.0.1:{port}"), "--send".into(), "--failover-key".into(), KEY.into()];
    a.extend(extra.iter().map(|s| s.to_string()));
    a
}

fn workdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pchi-fo-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Rig {
    _primary: Option<Proc>,
    primary: Proc,
    _standby: Proc,
    p1: u16,
    p2: u16,
    heard: Arc<Mutex<Vec<(Instant, u16)>>>,
    primary_src: u16,
    link: Option<Link>,
}

/// Both runtimes up, the show started on the primary, the desk hearing the primary, the standby caught up.
async fn start(tag: &str, through_link: bool) -> Rig {
    let dir = workdir(tag);
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let desk_port = sock.local_addr().unwrap().port();
    let heard = desk(sock);
    let show = show_file(&dir, desk_port);
    let (p1, p2) = (free_tcp().await, free_tcp().await);
    let primary = spawn(&args(&show, &dir.join("a.db"), p1, &[]));
    let link = if through_link { Some(link_to(p1).await) } else { None };
    let follow = format!("127.0.0.1:{}", link.as_ref().map_or(p1, |l| l.port));
    assert!(until_role(p1, 20, "active").await, "the primary did not start");
    let standby = spawn(&args(&show, &dir.join("b.db"), p2, &["--standby-of", &follow, "--takeover-after", "2", "--standby-name", "nova-b"]));
    assert!(until_role(p2, 20, "standby").await, "the standby did not start");

    let (code, _) = http(&format!("127.0.0.1:{p1}"), "POST", "/input", &[], Some("{\"event\":\"START\"}")).await.unwrap();
    assert_eq!(code, 200);
    assert!(until(10, || heard.lock().unwrap().len() >= 3).await, "the desk heard nothing from the primary");
    let primary_src = heard.lock().unwrap()[0].1;
    // the standby has caught up with a run that is in progress
    let mut caught = false;
    for _ in 0..100 {
        if role(p2).await.is_some_and(|r| r["primary_state"] == "RUN" && r["copied_records"].as_u64().unwrap_or(0) >= 6) {
            caught = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(caught, "the standby did not catch up");
    Rig { _primary: None, primary, _standby: standby, p1, p2, heard, primary_src, link }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_primary_that_is_killed_is_replaced_and_the_desk_hears_only_the_standby() {
    let mut rig = start("kill", false).await;
    let killed = Instant::now();
    let _ = rig.primary.0.kill();
    let _ = rig.primary.0.wait();
    assert!(until_role(rig.p2, 15, "active").await, "the standby did not take over");
    let took = killed.elapsed();
    // the desk now hears a different sender, and keeps hearing it
    assert!(until(10, || sources_since(&rig.heard, killed).iter().any(|p| *p != rig.primary_src)).await, "the desk never heard the standby");
    let first_new = rig.heard.lock().unwrap().iter().find(|(at, p)| *at >= killed && *p != rig.primary_src).map(|(at, _)| at.duration_since(killed)).unwrap();
    eprintln!("primary killed: standby active after {took:?}, desk heard it after {first_new:?}");
    assert!(first_new < Duration::from_secs(10), "{first_new:?}");
    let mark = Instant::now();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let after = sources_since(&rig.heard, mark);
    assert_eq!(after.len(), 1, "after the takeover the desk should hear one sender: {after:?}");
    assert!(!after.contains(&rig.primary_src));
    // it is the same show, carried on: the standby sent cues of the running scene
    let state = http(&format!("127.0.0.1:{}", rig.p2), "GET", "/state", &[], None).await.unwrap().1;
    assert_eq!(state["state"], "RUN");
    assert_eq!(state["resumed"]["state"], "RUN");
    let _ = rig.p1;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_the_link_is_cut_both_send_until_it_is_mended_and_then_only_the_standby_does() {
    let rig = start("split", true).await;
    let link = rig.link.as_ref().unwrap();

    // the link is cut; both can still reach the desk
    link.cut.store(true, Ordering::SeqCst);
    let cut_at = Instant::now();
    assert!(until_role(rig.p2, 15, "active").await, "the standby did not take over when it lost the primary");
    let took = cut_at.elapsed();
    let taken_at = Instant::now();
    assert_eq!(role(rig.p1).await.unwrap()["role"], "active", "the primary cannot be told while the link is cut");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let during = sources_since(&rig.heard, taken_at);
    eprintln!("link cut: standby took over after {took:?}; desk heard {} sender(s) during the cut", during.len());
    assert_eq!(during.len(), 2, "this is the case failover.md warns about: both send while the link is cut ({during:?})");

    // the link is mended: the standby tells the primary, and the primary stops
    link.cut.store(false, Ordering::SeqCst);
    let mended = Instant::now();
    assert!(until_role(rig.p1, 15, "fenced").await, "the primary was not stopped after the link came back");
    eprintln!("link mended: primary stopped after {:?}", mended.elapsed());
    tokio::time::sleep(Duration::from_millis(600)).await;
    let mark = Instant::now();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let after = sources_since(&rig.heard, mark);
    assert_eq!(after.len(), 1, "after the fence the desk should hear one sender: {after:?}");
    assert!(!after.contains(&rig.primary_src), "the stopped primary is still sending");
    let r = role(rig.p1).await.unwrap();
    assert_eq!((r["role"].as_str(), r["fenced_by"]["by"].as_str()), (Some("fenced"), Some("nova-b")));
}
