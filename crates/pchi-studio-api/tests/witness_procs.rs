//! The witness, the follower and the failover-day check, between real processes on loopback.
//!
//! Loopback cannot show a venue's network, but it can show the logic: that a standby that merely cannot reach a living primary is told no, that a primary cut off from
//! the witness stops BEFORE the standby starts (no moment with two senders), that a fixed address follows the show, and that `pchi-netcheck` measures a real takeover.

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

fn spawn_bin(bin: &str, args: &[String]) -> Proc {
    Proc(Command::new(bin).args(args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start the program"))
}

fn spawn(args: &[String]) -> Proc {
    spawn_bin(env!("CARGO_BIN_EXE_pchi-studio-api"), args)
}

async fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port()
}

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
                continue;
            }
            let flag2 = Arc::clone(&flag);
            tokio::spawn(async move {
                if let Ok(mut up) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await {
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut c, &mut up) => {}
                        _ = async { while !flag2.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(50)).await } } => {}
                    }
                }
            });
        }
    });
    Link { cut, port }
}

type Heard = Arc<Mutex<Vec<(Instant, u16)>>>;

fn desk(sock: UdpSocket) -> Heard {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
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

fn sources_since(heard: &Heard, t: Instant) -> BTreeSet<u16> {
    heard.lock().unwrap().iter().filter(|(at, _)| *at >= t).map(|(_, p)| *p).collect()
}

async fn get(addr: &str, path: &str) -> Option<Value> {
    match http(addr, "GET", path, &[], None).await {
        Ok((200, v)) => Some(v),
        _ => None,
    }
}

async fn role(port: u16) -> Option<Value> {
    get(&format!("127.0.0.1:{port}"), "/role").await
}

async fn until_async<F: FnMut() -> Fut, Fut: std::future::Future<Output = bool>>(secs: u64, mut ok: F) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if ok().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ok().await
}

async fn until_role(port: u16, secs: u64, want: &str) -> bool {
    until_async(secs, || async move { role(port).await.is_some_and(|r| r["role"] == want) }).await
}

fn show_file(dir: &std::path::Path, desk_port: u16) -> String {
    let cues: Vec<Value> = (1..=450).map(|i| json!({"id": format!("c{i}"), "at_ms": i * 200, "action": {"kind": "lighting.look", "payload": {"look": "warm"}}})).collect();
    let file = json!({
        "version": "1.0", "project": "test", "name": "witness", "edges": [], "timeline": [], "assets": [],
        "nodes": [{"id": "desk", "type": "lighting", "label": "Desk", "position": {"x": 0, "y": 0}, "params": {"host": "127.0.0.1", "port": desk_port}}],
        "showchart": {"id": "witness", "name": "Witness", "version": "0.1.0", "initial": "PRESHOW",
            "states": [{"id": "PRESHOW"}, {"id": "RUN", "timeline": {"duration_ms": 90000, "lanes": [{"id": "lighting", "name": "Lights", "dept": "lighting", "cues": cues}]}}],
            "transitions": [{"source": "PRESHOW", "target": "RUN", "event": "START"}]},
    });
    let path = dir.join("show.pchi");
    std::fs::write(&path, file.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

fn workdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pchi-wit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn args(show: &str, db: &std::path::Path, port: u16, extra: &[&str]) -> Vec<String> {
    let mut a: Vec<String> = vec![show.into(), "--db".into(), db.to_string_lossy().into_owned(), "--bind".into(), format!("127.0.0.1:{port}"), "--send".into(), "--failover-key".into(), KEY.into()];
    a.extend(extra.iter().map(|s| s.to_string()));
    a
}

struct Rig {
    dir: std::path::PathBuf,
    show: String,
    primary: Proc,
    _standby: Proc,
    _witness: Proc,
    p1: u16,
    p2: u16,
    wp: u16,
    /// standby -> primary
    ls: Link,
    /// primary -> witness
    lw: Link,
    heard: Heard,
    primary_src: u16,
}

/// Witness, primary and standby up (each reaching the others through a link that can be cut), the show started, the desk hearing the primary, the standby caught up.
async fn start(tag: &str) -> Rig {
    let dir = workdir(tag);
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let desk_port = sock.local_addr().unwrap().port();
    let heard = desk(sock);
    let show = show_file(&dir, desk_port);
    let (p1, p2, wp) = (free_tcp().await, free_tcp().await, free_tcp().await);
    let witness = spawn_bin(env!("CARGO_BIN_EXE_pchi-witness"), &["--bind".into(), format!("127.0.0.1:{wp}"), "--failover-key".into(), KEY.into()]);
    assert!(until_async(20, || async move { get(&format!("127.0.0.1:{wp}"), "/primary").await.is_some() }).await, "the witness did not start");
    let lw = link_to(wp).await;
    let primary = spawn(&args(&show, &dir.join("a.db"), p1, &["--witness", &format!("127.0.0.1:{}", lw.port), "--witness-ttl", "3", "--advertise", &format!("127.0.0.1:{p1}")]));
    assert!(until_role(p1, 20, "active").await, "the primary did not start");
    let ls = link_to(p1).await;
    let standby = spawn(&args(
        &show,
        &dir.join("b.db"),
        p2,
        &["--standby-of", &format!("127.0.0.1:{}", ls.port), "--takeover-after", "2", "--stall-grace", "0", "--standby-name", "nova-b", "--witness", &format!("127.0.0.1:{wp}"), "--witness-ttl", "3", "--advertise", &format!("127.0.0.1:{p2}")],
    ));
    assert!(until_role(p2, 20, "standby").await, "the standby did not start");
    let (code, _) = http(&format!("127.0.0.1:{p1}"), "POST", "/input", &[], Some("{\"event\":\"START\"}")).await.unwrap();
    assert_eq!(code, 200);
    assert!(until_async(10, || async { heard.lock().unwrap().len() >= 3 }).await, "the desk heard nothing from the primary");
    let primary_src = heard.lock().unwrap()[0].1;
    assert!(until_async(15, || async move { role(p2).await.is_some_and(|r| r["primary_state"] == "RUN" && r["copied_records"].as_u64().unwrap_or(0) >= 6) }).await, "the standby did not catch up");
    Rig { dir, show, primary, _standby: standby, _witness: witness, p1, p2, wp, ls, lw, heard, primary_src }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standby_that_cannot_reach_a_living_primary_is_told_no_by_the_witness() {
    let rig = start("alive").await;
    let w = format!("127.0.0.1:{}", rig.wp);
    assert_eq!(get(&w, "/primary").await.unwrap()["primary"], json!(format!("127.0.0.1:{}", rig.p1)), "the witness should say where the show is");

    // only the link between the two is cut: the primary still reaches the witness and the desk
    rig.ls.cut.store(true, Ordering::SeqCst);
    let from = Instant::now();
    tokio::time::sleep(Duration::from_secs(9)).await; // takeover-after is 2 s: the standby wants to, and has had time to try many times
    let r = role(rig.p2).await.unwrap();
    assert_eq!(r["role"], "standby", "the standby took over from a primary that the witness still sees: {r}");
    let note = r["witness"]["note"].as_str().unwrap_or("");
    assert!(note.contains("holds the show's lease"), "the standby should say why it did not: {r}");
    assert_eq!(role(rig.p1).await.unwrap()["role"], "active");
    assert_eq!(sources_since(&rig.heard, from + Duration::from_secs(1)), BTreeSet::from([rig.primary_src]), "the desk should hear only the primary");

    // and a primary that really dies is replaced once its lease runs out
    let mut rig = rig;
    let killed = Instant::now();
    let _ = rig.primary.0.kill();
    let _ = rig.primary.0.wait();
    rig.ls.cut.store(false, Ordering::SeqCst);
    assert!(until_role(rig.p2, 20, "active").await, "the standby did not take over from a dead primary");
    eprintln!("primary killed with a witness: standby active after {:?}", killed.elapsed());
    assert_eq!(get(&w, "/primary").await.unwrap()["primary"], json!(format!("127.0.0.1:{}", rig.p2)), "the witness should now say the show is on the standby");
    // the standby runs under the lease: asked again, it keeps it
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(role(rig.p2).await.unwrap()["role"], "active", "the new primary lost its lease");
    assert_eq!(get(&format!("127.0.0.1:{}", rig.p2), "/primary").await.unwrap()["self"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_primary_cut_off_from_the_witness_stops_before_the_standby_starts() {
    let rig = start("isolate").await;
    // the primary loses both the standby and the witness; the standby still reaches the witness
    rig.ls.cut.store(true, Ordering::SeqCst);
    rig.lw.cut.store(true, Ordering::SeqCst);
    let cut_at = Instant::now();
    assert!(until_role(rig.p1, 15, "paused").await, "the primary kept sending without a lease");
    eprintln!("isolated primary paused after {:?}", cut_at.elapsed());
    assert!(until_role(rig.p2, 20, "active").await, "the standby did not take over");
    eprintln!("standby active after {:?}", cut_at.elapsed());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (last_primary, first_standby) = {
        let h = rig.heard.lock().unwrap();
        let lp = h.iter().filter(|(_, p)| *p == rig.primary_src).map(|(t, _)| *t).max().unwrap();
        let fs = h.iter().filter(|(t, p)| *t >= cut_at && *p != rig.primary_src).map(|(t, _)| *t).min().expect("the desk never heard the standby");
        (lp, fs)
    };
    eprintln!("last primary datagram {:?} before the first standby datagram", first_standby.saturating_duration_since(last_primary));
    assert!(last_primary < first_standby, "the desk heard the primary AFTER the standby began: two senders at once");

    // the links are mended: the paused primary does not start again, and is told it was replaced
    rig.ls.cut.store(false, Ordering::SeqCst);
    rig.lw.cut.store(false, Ordering::SeqCst);
    assert!(until_role(rig.p1, 20, "fenced").await, "the old primary was not fenced");
    let mark = Instant::now();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let after = sources_since(&rig.heard, mark);
    assert_eq!(after.len(), 1, "after the heal the desk should hear one sender: {after:?}");
    assert!(!after.contains(&rig.primary_src));
    // GET /primary on the old one says where the show went
    let p = get(&format!("127.0.0.1:{}", rig.p1), "/primary").await.unwrap();
    assert_eq!((p["self"].clone(), p["primary"].clone()), (json!(false), json!(format!("127.0.0.1:{}", rig.p2))), "{p}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_primary_goes_on_by_itself_when_the_witness_comes_back() {
    let rig = start("blip").await;
    rig.lw.cut.store(true, Ordering::SeqCst); // only the witness is out of reach; the standby cannot take over without the witness's say either
    assert!(until_role(rig.p1, 15, "paused").await, "it should pause when it cannot prove it holds the lease");
    let mark = Instant::now();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(sources_since(&rig.heard, mark).is_empty(), "a paused runtime must not send");
    rig.lw.cut.store(false, Ordering::SeqCst);
    assert!(until_role(rig.p1, 15, "active").await, "it should go on when the lease is held again");
    let mark = Instant::now();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(sources_since(&rig.heard, mark), BTreeSet::from([rig.primary_src]), "after the blip the same runtime sends again");
    assert_eq!(role(rig.p2).await.unwrap()["role"], "standby", "the standby must not have taken over during a blip it could not confirm");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_follower_leads_to_whichever_runtime_runs_the_show_and_netcheck_times_a_real_takeover() {
    let mut rig = start("follow").await;
    let fport = free_tcp().await;
    let _follow = spawn_bin(
        env!("CARGO_BIN_EXE_pchi-follow"),
        &["--listen".into(), format!("127.0.0.1:{fport}"), "--runtime".into(), format!("127.0.0.1:{}", rig.p1), "--runtime".into(), format!("127.0.0.1:{}", rig.p2), "--every-ms".into(), "200".into()],
    );
    let f = format!("127.0.0.1:{fport}");
    assert!(until_async(10, || async { get(&f, "/role").await.is_some_and(|r| r["role"] == "active" && r["epoch"] == 0) }).await, "the follower should lead to the primary");

    // the failover-day check, without the cut: every check that changes nothing
    let dir = rig.dir.clone();
    let report = dir.join("report.md");
    let out = Command::new(env!("CARGO_BIN_EXE_pchi-netcheck"))
        .args(["--primary", &format!("127.0.0.1:{}", rig.p1), "--standby", &format!("127.0.0.1:{}", rig.p2), "--witness", &format!("127.0.0.1:{}", rig.wp), "--failover-key", KEY, "--no-cut", "--samples", "5", "--report", report.to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    eprintln!("{text}");
    assert!(out.status.success(), "netcheck found a failure on a healthy pair:\n{text}");
    assert!(text.contains("the witness refuses a second sender") && text.contains("rehearse a takeover"), "{text}");
    let md = std::fs::read_to_string(&report).unwrap();
    assert!(md.contains("| Check | Result |"), "{md}");

    // the primary dies: the follower moves to the standby
    let killed = Instant::now();
    let _ = rig.primary.0.kill();
    let _ = rig.primary.0.wait();
    assert!(until_async(25, || async { get(&f, "/role").await.is_some_and(|r| r["role"] == "active" && r["epoch"] == 1) }).await, "the follower did not move to the standby");
    eprintln!("primary killed: the fixed address led to the standby after {:?}", killed.elapsed());
    let _ = &rig.show;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn netcheck_cut_test_times_the_takeover_and_the_old_primary_stopping() {
    use std::io::Write;
    let mut rig = start("netcheck").await;
    let report = rig.dir.join("cut.md");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pchi-netcheck"))
        .args(["--primary", &format!("127.0.0.1:{}", rig.p1), "--standby", &format!("127.0.0.1:{}", rig.p2), "--witness", &format!("127.0.0.1:{}", rig.wp), "--failover-key", KEY, "--samples", "3", "--report", report.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // the person cuts the primary (here: stops it) and says so
    tokio::time::sleep(Duration::from_secs(4)).await; // the checks that change nothing run first
    let _ = rig.primary.0.kill();
    let _ = rig.primary.0.wait();
    writeln!(stdin, "CUT").unwrap();
    assert!(until_role(rig.p2, 25, "active").await, "the standby did not take over");
    // the person restores the old primary (started again from its own log) and says so
    let dir = rig.dir.clone();
    rig.primary = spawn(&args(&rig.show, &dir.join("a.db"), rig.p1, &["--resume", "--witness", &format!("127.0.0.1:{}", rig.lw.port), "--witness-ttl", "3", "--advertise", &format!("127.0.0.1:{}", rig.p1)]));
    assert!(until_async(20, || async { role(rig.p1).await.is_some() }).await, "the old primary did not come back");
    writeln!(stdin, "DONE").unwrap();
    drop(stdin);
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().unwrap()).await.unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    eprintln!("{text}");
    let md = std::fs::read_to_string(&report).unwrap();
    assert!(md.contains("the standby takes over | pass"), "{md}");
    assert!(md.contains("the old primary stops | pass"), "{md}");
    assert!(md.contains("only one is sending at the end | pass"), "{md}");
}
