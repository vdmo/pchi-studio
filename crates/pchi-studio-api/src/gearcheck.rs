//! A guided check of each rig device against the real tool (or its software), one device at a time.
//!
//! It does what `docs/foh-proof.md` section 4 asks, with an operator at the keyboard: confirm it is safe, send ONE cue
//! through the tool's profile and show the exact OSC message, ask whether the tool reacted, listen for what the tool
//! sends back and ask whether it matches the tool's own OSC monitor, and (where the tool can be probed) ask for the
//! cable to be pulled and plugged back to see the link notice it. It writes a report and can tick the status column of
//! the checklist, but only for what the operator confirmed.
//!
//! What it cannot do: it cannot see or hear the tool. "The tool reacted" is the operator's word, and the report says so.

use crate::{output, output::Output, profiles, rig::Device};
use pchi_showchart::Action;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the questions go and the answers come from: a terminal in real use, a script in tests.
pub trait Console {
    fn say(&mut self, text: &str);
    fn ask(&mut self, prompt: &str) -> String;
}

pub struct Options {
    pub poll: Duration,
    /// How long to listen for a tool's feedback before giving up.
    pub feedback_wait: Duration,
    /// How long after the cable is pulled the link may take to show as lost.
    pub cable_wait: Duration,
    /// How long after it is plugged back the link may take to recover.
    pub recover_wait: Duration,
}

impl Options {
    pub fn for_output(out: &Output) -> Self {
        Options { poll: Duration::from_millis(250), feedback_wait: Duration::from_secs(12), cable_wait: out.link_timeout + Duration::from_secs(10), recover_wait: Duration::from_secs(30) }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Feedback {
    pub heard: bool,
    pub addresses: Vec<String>,
    /// The operator's answer: does it match the tool's own OSC monitor? None if not asked.
    pub matches_monitor: Option<bool>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Cable {
    pub lost_after_s: Option<f64>,
    pub recovered_after_s: Option<f64>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DeviceResult {
    pub id: String,
    pub label: String,
    pub tool: String,
    pub address: Option<String>,
    pub value: Option<String>,
    pub sent: Vec<String>,
    pub send_error: Option<String>,
    /// "yes", "no" or "unsure": what the operator saw. Empty if nothing was sent.
    pub reacted: String,
    pub note: String,
    pub feedback: Option<Feedback>,
    pub cable: Option<Cable>,
    /// "ticked", "not ticked" or "skipped".
    pub verdict: String,
    pub why: String,
}

impl DeviceResult {
    fn new(dev: &Device) -> Self {
        DeviceResult { id: dev.id.clone(), label: dev.label.clone(), tool: dev.profile_key().to_string(), address: dev.address(), ..Default::default() }
    }
    fn skip(mut self, why: &str) -> Self {
        self.verdict = "skipped".into();
        self.why = why.into();
        self
    }
    fn finish(mut self) -> Self {
        let fb_ok = self.feedback.as_ref().map_or(true, |f| f.matches_monitor != Some(false));
        let cable_ok = self.cable.as_ref().map_or(true, |c| c.lost_after_s.is_some() && c.recovered_after_s.is_some());
        let ok = self.send_error.is_none() && self.reacted == "yes" && fb_ok && cable_ok;
        self.verdict = if ok { "ticked" } else { "not ticked" }.into();
        if !ok && self.why.is_empty() {
            self.why = if self.send_error.is_some() { "the message could not be sent".into() } else if self.reacted != "yes" { "the tool did not clearly react".into() } else if !fb_ok { "feedback did not match the tool's own monitor".into() } else { "the link did not notice the cable being pulled and restored".into() };
        }
        self
    }
}

fn what_for(dev: &Device) -> (&'static str, &'static str, &'static str) {
    match dev.dept.as_str() {
        "visual" => ("scene.activate", "scene", "column (3) or layer/clip (2/5)"),
        "lighting" => ("lighting.look", "look", "cue or executor number"),
        "laser" => ("laser.cue", "cue", "cue name"),
        _ => ("audio.snapshot", "snapshot", "scene or snapshot number"),
    }
}

fn link_state(out: &Output, dev: &Device) -> String {
    out.link_state(dev).and_then(|l| l["state"].as_str().map(String::from)).unwrap_or_else(|| "none".into())
}

/// Check each device in turn (all of them, or only `only`). Sends for real, one device at a time, only after being told to.
pub async fn run(c: &mut dyn Console, out: &Output, only: &[String], o: &Options) -> Vec<DeviceResult> {
    let mut results = Vec::new();
    for dev in out.devices() {
        if !only.is_empty() && !only.contains(&dev.id) {
            continue;
        }
        results.push(check_device(c, out, dev, o).await);
    }
    results
}

async fn check_device(c: &mut dyn Console, out: &Output, dev: &Device, o: &Options) -> DeviceResult {
    let mut r = DeviceResult::new(dev);
    c.say(&format!("\n=== {} ({}) at {} ===", dev.label, dev.kind, dev.address().unwrap_or_else(|| "NO ADDRESS".into())));
    if dev.address().is_none() {
        return r.skip("no host and port in the rig");
    }
    if !profiles::exists(dev.profile_key()) && !dev.is_dmx() {
        return r.skip("this type has no profile: it gets generic OSC, which only the tool's own manual can confirm");
    }
    // safety first
    match dev.dept.as_str() {
        "laser" => {
            c.say("This is a LASER. The beam must be safe: output off or blocked at the laser software, nobody in the beam path.");
            if c.ask("Type LASER SAFE to go on, or press Enter to skip this device:").trim() != "LASER SAFE" {
                return r.skip("laser safety not confirmed");
            }
        }
        "audio" => {
            c.say("This is an audio desk. A scene or snapshot recall can change what goes to the PA.");
            if c.ask("Mute the PA, then type MUTED to go on, or press Enter to skip this device:").trim() != "MUTED" {
                return r.skip("PA mute not confirmed");
            }
        }
        _ => {
            if c.ask("Press Enter to start, or s to skip this device:").trim().eq_ignore_ascii_case("s") {
                return r.skip("skipped by the operator");
            }
        }
    }
    let (kind, field, hint) = what_for(dev);
    let hint = if dev.is_dmx() { "levels (for example 1=255 or 1-3=50%)" } else { hint };
    let value = c.ask(&format!("Which {hint} should it try? Pick a harmless one (Enter to skip):")).trim().to_string();
    if value.is_empty() {
        return r.skip("no value given");
    }
    r.value = Some(value.clone());
    let action = Action::new(kind, json!({ field: value }));
    let lines = match output::wire_lines(dev, &action) {
        Ok(l) => l,
        Err(e) => {
            c.say(&format!("PCHI cannot build a message for that: {e}"));
            r.send_error = Some(e);
            return r.finish();
        }
    };
    let packets = if dev.is_dmx() { Vec::new() } else { output::packets_for(dev, &action).unwrap_or_default() };
    r.sent = lines.iter().map(|(a, args)| if args.is_empty() { a.clone() } else { format!("{a} {}", args.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")) }).collect();
    c.say("It will send exactly this:");
    for l in &r.sent {
        c.say(&format!("    {l}"));
    }
    if !c.ask("Send it now? (y/n):").trim().eq_ignore_ascii_case("y") {
        return r.skip("not sent");
    }
    if dev.dept == "laser" {
        out.set_armed(&dev.id, true); // only inside this check: the real runtime always starts disarmed
    }
    match if dev.is_dmx() { out.deliver(dev, &action).await } else { out.send_packets(dev, packets).await } {
        Ok(_) => c.say("Sent."),
        Err(e) => {
            c.say(&format!("Could not send: {e}"));
            r.send_error = Some(e);
        }
    }
    if dev.dept == "laser" {
        out.set_armed(&dev.id, false);
        c.say("Disarmed again. If the laser software's output is on, use its own stop.");
    }
    if r.send_error.is_none() {
        let a = c.ask("Did the tool react as expected? (y = yes, n = no, ? = not sure):").trim().to_ascii_lowercase();
        r.reacted = match a.as_str() {
            "y" | "yes" => "yes",
            "n" | "no" => "no",
            _ => "unsure",
        }
        .into();
        if r.reacted != "yes" {
            r.note = c.ask("What did you see? What does the tool's own OSC monitor show arriving? (Enter to skip):").trim().to_string();
        }
    }

    // what the tool sends back
    if r.send_error.is_none() && Output::expects_feedback(dev) {
        r.feedback = Some(listen_for_feedback(c, out, dev, o).await);
    } else if r.send_error.is_none() {
        c.say("This profile gets no feedback from the tool (set a listen port in the Rig tab to try).");
    }
    // the cable test needs a tool that answers a probe: only then can the link notice a loss
    if r.send_error.is_none() && profiles::probe(dev.profile_key()).is_some() && r.feedback.as_ref().is_some_and(|f| f.heard) {
        r.cable = cable_test(c, out, dev, o).await;
    }
    r.finish()
}

async fn listen_for_feedback(c: &mut dyn Console, out: &Output, dev: &Device, o: &Options) -> Feedback {
    let mut fb = Feedback::default();
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink_store = Arc::clone(&heard);
    let id = dev.id.clone();
    out.links().set_sink(Arc::new(move |m: Value| {
        if m["device"] == json!(id) {
            if let Some(a) = m["address"].as_str() {
                let mut v = sink_store.lock().unwrap();
                if v.len() < 20 && !v.iter().any(|x| x == a) {
                    v.push(a.to_string());
                }
            }
        }
    }));
    if let Err(e) = out.links().get(dev).await {
        c.say(&format!("Cannot listen for feedback: {e}"));
        return fb;
    }
    let probe = profiles::probe(dev.profile_key());
    if probe.is_none() {
        c.say("Now make the tool send something by itself (fire a cue or move a fader in the tool). Listening…");
    } else {
        c.say("Asking the tool to answer, and listening…");
    }
    let end = Instant::now() + o.feedback_wait;
    let mut last_probe: Option<Instant> = None;
    while Instant::now() < end {
        if let Some(p) = &probe {
            if last_probe.map_or(true, |t| t.elapsed() >= out.probe_every) {
                last_probe = Some(Instant::now());
                let _ = out.send_packets(dev, vec![p.clone()]).await;
            }
        }
        if matches!(link_state(out, dev).as_str(), "linked" | "heard") {
            tokio::time::sleep(o.poll * 2).await; // let a few more messages in
            break;
        }
        tokio::time::sleep(o.poll).await;
    }
    fb.addresses = heard.lock().unwrap().clone();
    fb.heard = matches!(link_state(out, dev).as_str(), "linked" | "heard") || !fb.addresses.is_empty();
    if fb.heard {
        c.say(&format!("Heard from the tool: {}", if fb.addresses.is_empty() { "(a reply, no readable address)".to_string() } else { fb.addresses.join(", ") }));
        fb.matches_monitor = match c.ask("Does that match what the tool's own OSC monitor shows it sending? (y/n/Enter to skip):").trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Some(true),
            "n" | "no" => Some(false),
            _ => None,
        };
    } else {
        c.say("Nothing heard. Check the tool's OSC output address and port, and the listen port in the Rig tab.");
    }
    fb
}

async fn cable_test(c: &mut dyn Console, out: &Output, dev: &Device, o: &Options) -> Option<Cable> {
    if c.ask("Cable test: unplug the tool's network cable now, then press Enter (s to skip):").trim().eq_ignore_ascii_case("s") {
        return None;
    }
    let probe = profiles::probe(dev.profile_key())?;
    let mut cable = Cable::default();
    let start = Instant::now();
    let mut last_probe: Option<Instant> = None;
    while start.elapsed() < o.cable_wait {
        if last_probe.map_or(true, |t| t.elapsed() >= out.probe_every) {
            last_probe = Some(Instant::now());
            let _ = out.send_packets(dev, vec![probe.clone()]).await;
        }
        if link_state(out, dev) == "lost" {
            cable.lost_after_s = Some(start.elapsed().as_secs_f64());
            break;
        }
        tokio::time::sleep(o.poll).await;
    }
    c.say(&match cable.lost_after_s {
        Some(s) => format!("The link noticed after {s:.1} s."),
        None => "The link did NOT notice the cable being pulled.".to_string(),
    });
    c.ask("Plug the cable back in, then press Enter:");
    let start = Instant::now();
    last_probe = None;
    while start.elapsed() < o.recover_wait {
        if last_probe.map_or(true, |t| t.elapsed() >= out.probe_every) {
            last_probe = Some(Instant::now());
            let _ = out.send_packets(dev, vec![probe.clone()]).await;
        }
        if link_state(out, dev) == "linked" {
            cable.recovered_after_s = Some(start.elapsed().as_secs_f64());
            break;
        }
        tokio::time::sleep(o.poll).await;
    }
    c.say(&match cable.recovered_after_s {
        Some(s) => format!("The link recovered after {s:.1} s."),
        None => "The link did NOT recover.".to_string(),
    });
    Some(cable)
}

// ---- the written record ----

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// The checklist's row name for a tool, as written in docs/foh-proof.md.
pub fn doc_label(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "resolume" => "Resolume Arena",
        "grandma3" => "grandMA3",
        "eos" => "ETC Eos",
        "x32" => "Behringer X32 / M32",
        "qlab" => "QLab",
        "magicq" => "ChamSys MagicQ",
        "beyond" => "BEYOND",
        "artnet" => "Art-Net node / fixture",
        "sacn" => "sACN node / fixture",
        _ => return None,
    })
}

fn status_text(r: &DeviceResult, date: &str) -> Option<String> {
    match r.verdict.as_str() {
        "ticked" => {
            let mut s = format!("ticked {date}: sent `{}`, reacted", r.sent.first().map_or("", String::as_str));
            if r.feedback.as_ref().is_some_and(|f| f.heard) {
                s += ", feedback heard";
            }
            if r.cable.is_some() {
                s += ", cable pull noticed and recovered";
            }
            Some(s + " (operator's word)")
        }
        "not ticked" => Some(format!("tried {date}, NOT ok: {}", r.why)),
        _ => None, // skipped: the status stays as it was
    }
}

/// Rewrite the Status cell of each tool's row in the checklist for what was actually tried. Skipped devices change nothing.
pub fn update_doc(doc: &str, results: &[DeviceResult], date: &str) -> String {
    let mut out = String::new();
    for line in doc.lines() {
        let mut line = line.to_string();
        if line.starts_with("| ") && line.ends_with(" |") {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            if cells.len() == 4 {
                if let Some(r) = results.iter().filter(|r| doc_label(&r.tool) == Some(cells[0])).filter_map(|r| status_text(r, date).map(|s| (r, s))).last() {
                    line = format!("| {} | {} | {} | {} |", cells[0], cells[1], cells[2], r.1);
                }
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn report_markdown(results: &[DeviceResult], date: &str) -> String {
    let mut s = format!("# Gear check {date}\n\n\"Reacted\" is what the operator said they saw or heard: this check cannot see the tool.\n\n");
    for r in results {
        s += &format!("## {} ({}): {}\n\n", r.label, r.tool, r.verdict.to_uppercase());
        if !r.why.is_empty() {
            s += &format!("- {}\n", r.why);
        }
        if let Some(a) = &r.address {
            s += &format!("- address: {a}\n");
        }
        for l in &r.sent {
            s += &format!("- sent: `{l}`\n");
        }
        if let Some(e) = &r.send_error {
            s += &format!("- send error: {e}\n");
        }
        if !r.reacted.is_empty() {
            s += &format!("- tool reacted: {}\n", r.reacted);
        }
        if !r.note.is_empty() {
            s += &format!("- note: {}\n", r.note);
        }
        if let Some(f) = &r.feedback {
            s += &format!("- feedback: {}{}\n", if f.heard { f.addresses.join(", ") } else { "nothing heard".into() }, match f.matches_monitor { Some(true) => " (matches the tool's monitor)", Some(false) => " (does NOT match the tool's monitor)", None => "" });
        }
        if let Some(cb) = &r.cable {
            s += &format!("- cable pull: noticed after {}, recovered after {}\n", cb.lost_after_s.map_or("never".to_string(), |x| format!("{x:.1} s")), cb.recovered_after_s.map_or("never".to_string(), |x| format!("{x:.1} s")));
        }
        s += "\n";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links;
    use crate::rig::dept_of_type;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::UdpSocket;

    struct Script {
        answers: VecDeque<String>,
        said: Vec<String>,
        asked: Vec<String>,
        /// Runs when a prompt containing the text is asked (to pull the "cable" in a test).
        hooks: Vec<(String, Box<dyn Fn() + Send>)>,
    }
    impl Script {
        fn new(a: &[&str]) -> Self {
            Script { answers: a.iter().map(|s| s.to_string()).collect(), said: vec![], asked: vec![], hooks: vec![] }
        }
    }
    impl Console for Script {
        fn say(&mut self, t: &str) {
            self.said.push(t.into());
        }
        fn ask(&mut self, p: &str) -> String {
            self.asked.push(p.into());
            for (needle, f) in &self.hooks {
                if p.contains(needle.as_str()) {
                    f();
                }
            }
            self.answers.pop_front().unwrap_or_default()
        }
    }

    fn dev(id: &str, kind: &str, port: u16) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: None }
    }

    /// A pretend X32: answers /info to whoever asked, while `up` is true; records what it received.
    async fn fake_x32(up: Arc<AtomicBool>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let got: Arc<Mutex<Vec<String>>> = Arc::default();
        let g = Arc::clone(&got);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else { break };
                for (a, _) in links::flatten(&buf[..n]) {
                    g.lock().unwrap().push(a.clone());
                    if a == "/info" && up.load(Ordering::Relaxed) {
                        let reply = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: "/info".into(), args: vec![rosc::OscType::String("V2.07".into()), rosc::OscType::String("X32".into())] })).unwrap();
                        let _ = sock.send_to(&reply, from).await;
                    }
                }
            }
        });
        (port, got)
    }

    fn fast() -> Options {
        Options { poll: Duration::from_millis(50), feedback_wait: Duration::from_secs(3), cable_wait: Duration::from_secs(4), recover_wait: Duration::from_secs(4) }
    }

    #[tokio::test]
    async fn a_good_x32_is_ticked_after_a_cue_a_reply_and_a_cable_pull() {
        let up = Arc::new(AtomicBool::new(true));
        let (port, got) = fake_x32(Arc::clone(&up)).await;
        let out = Output::new(vec![dev("desk", "x32", port)], true).with_timing(Duration::from_millis(200), Duration::from_millis(800));
        let mut c = Script::new(&["MUTED", "3", "y", "y", "y", "y", "", ""]);
        let (u1, u2) = (Arc::clone(&up), Arc::clone(&up));
        c.hooks.push(("unplug".into(), Box::new(move || u1.store(false, Ordering::Relaxed))));
        c.hooks.push(("Plug the cable back".into(), Box::new(move || u2.store(true, Ordering::Relaxed))));
        let res = run(&mut c, &out, &[], &fast()).await;
        let r = &res[0];
        assert_eq!(r.verdict, "ticked", "{r:?}\n{:?}", c.said);
        assert!(r.sent[0].starts_with("/-action/goscene"), "{:?}", r.sent);
        assert!(r.feedback.as_ref().unwrap().heard);
        let cable = r.cable.as_ref().unwrap();
        assert!(cable.lost_after_s.is_some() && cable.recovered_after_s.is_some(), "{cable:?}");
        assert!(got.lock().unwrap().iter().any(|a| a.starts_with("/-action/goscene")), "the cue really arrived");
    }

    #[tokio::test]
    async fn a_tool_that_did_not_react_or_a_link_that_never_notices_is_not_ticked() {
        let up = Arc::new(AtomicBool::new(true));
        let (port, _) = fake_x32(Arc::clone(&up)).await;
        // the operator says the tool did nothing
        let out = Output::new(vec![dev("desk", "x32", port)], true).with_timing(Duration::from_millis(200), Duration::from_millis(800));
        let mut c = Script::new(&["MUTED", "3", "y", "n", "nothing on the desk", "y", "s"]);
        let r = run(&mut c, &out, &[], &fast()).await.remove(0);
        assert_eq!((r.verdict.as_str(), r.reacted.as_str(), r.note.as_str()), ("not ticked", "no", "nothing on the desk"));
        assert!(r.cable.is_none(), "the cable test was skipped");
        // the tool reacted, but the "cable pull" is never noticed because the fake keeps answering
        let out = Output::new(vec![dev("desk", "x32", port)], true).with_timing(Duration::from_millis(200), Duration::from_millis(800));
        let mut c = Script::new(&["MUTED", "3", "y", "y", "y", "", "", ""]);
        let r = run(&mut c, &out, &[], &Options { cable_wait: Duration::from_millis(1500), recover_wait: Duration::from_millis(500), ..fast() }).await.remove(0);
        assert_eq!(r.verdict, "not ticked", "{r:?}");
        assert!(r.why.contains("cable"), "{}", r.why);
    }

    #[tokio::test]
    async fn safety_prompts_gate_lasers_and_audio_and_nothing_is_sent_without_them() {
        let up = Arc::new(AtomicBool::new(true));
        let (port, got) = fake_x32(up).await;
        let out = Output::new(vec![dev("b1", "beyond", port), dev("desk", "x32", port)], true);
        // no LASER SAFE, no MUTED: both skipped
        let mut c = Script::new(&["", ""]);
        let res = run(&mut c, &out, &[], &fast()).await;
        assert_eq!(res.iter().map(|r| (r.verdict.clone(), r.why.clone())).collect::<Vec<_>>(), vec![("skipped".into(), "laser safety not confirmed".into()), ("skipped".into(), "PA mute not confirmed".into())]);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(got.lock().unwrap().is_empty(), "nothing reached either device");
        // a device the operator declines to send to is skipped too, and a laser is disarmed again after
        let mut c = Script::new(&["LASER SAFE", "intro", "n"]);
        let res = run(&mut c, &out, &["b1".into()], &fast()).await;
        assert_eq!(res[0].why, "not sent");
        assert!(!out.is_armed("b1"));
    }

    #[tokio::test]
    async fn only_what_was_tried_changes_the_checklist() {
        let doc = "| Tool | Try a cue with | Address to confirm | Status |\n|---|---|---|---|\n| Resolume Arena | `3` | `/composition/columns/3/connect` | not yet tried |\n| QLab | a cue number | `/cue/<n>/start` (re-check) | not yet tried |\n| ChamSys MagicQ | `3` | `/pb/3/go` | not yet tried |\n";
        let ok = DeviceResult { tool: "resolume".into(), sent: vec!["/composition/columns/3/connect 1".into()], reacted: "yes".into(), verdict: "ticked".into(), ..Default::default() };
        let bad = DeviceResult { tool: "qlab".into(), verdict: "not ticked".into(), why: "the tool did not clearly react".into(), ..Default::default() };
        let skipped = DeviceResult { tool: "magicq".into(), verdict: "skipped".into(), ..Default::default() };
        let new = update_doc(doc, &[ok, bad, skipped], "2026-11-14");
        assert!(new.contains("| Resolume Arena | `3` | `/composition/columns/3/connect` | ticked 2026-11-14: sent `/composition/columns/3/connect 1`, reacted (operator's word) |"), "{new}");
        assert!(new.contains("| QLab | a cue number | `/cue/<n>/start` (re-check) | tried 2026-11-14, NOT ok: the tool did not clearly react |"));
        assert!(new.contains("| ChamSys MagicQ | `3` | `/pb/3/go` | not yet tried |"), "a skipped tool keeps its status");
        assert_eq!(new.lines().count(), doc.lines().count());
    }

    #[test]
    fn dates_and_the_report_read_well() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_453), (2025, 12, 31));
        assert_eq!(today().len(), 10);
        let r = DeviceResult { id: "d".into(), label: "Desk".into(), tool: "x32".into(), verdict: "not ticked".into(), why: "x".into(), reacted: "no".into(), ..Default::default() };
        let md = report_markdown(&[r], "2026-11-14");
        assert!(md.contains("# Gear check 2026-11-14") && md.contains("## Desk (x32): NOT TICKED") && md.contains("cannot see the tool"));
    }

    #[tokio::test]
    async fn a_dmx_output_is_checked_with_levels_and_ticked_when_the_fixture_reacts() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = sock.local_addr().unwrap().port();
        let d = Device { id: "rack".into(), label: "Rack".into(), kind: "artnet".into(), dept: "lighting".into(), host: Some("127.0.0.1".into()), port: Some(port), listen_port: None, profile: None, universe: Some(1) };
        let out = Output::new(vec![d], true);
        let mut c = Script::new(&["", "1-3=100%", "y", "y"]);
        let r = run(&mut c, &out, &[], &fast()).await.remove(0);
        assert_eq!(r.verdict, "ticked", "{r:?}\n{:?}", c.said);
        assert_eq!(r.tool, "artnet");
        assert!(r.sent[0].starts_with("Art-Net universe 1"), "{:?}", r.sent);
        let mut buf = vec![0u8; 2048];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await.expect("a frame").unwrap();
        assert_eq!((n, buf[14], buf[18], buf[20]), (530, 1, 255, 255));
        // levels that are not channels are caught before anything is sent
        let mut c = Script::new(&["", "warm"]);
        let r = run(&mut c, &out, &[], &fast()).await.remove(0);
        assert!(r.send_error.as_deref().unwrap_or("").contains("use channels like"), "{r:?}");
        assert_eq!(doc_label("artnet"), Some("Art-Net node / fixture"));
    }
}
