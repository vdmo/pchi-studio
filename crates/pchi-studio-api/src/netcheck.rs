//! A failover day: prove, over the real network between the real machines, that the failover works, and write down what was measured.
//!
//! Everything about failover so far was shown on one computer. A network adds what a computer does not: delay, loss, clocks that disagree, firewalls, a switch
//! that is slow to forward after a cable is moved. This walks the checks in order, measures each, and, with the operator at the keyboard, cuts the primary off
//! and times the takeover, then restores it and times how long the old one takes to stop. It sends nothing to any tool: it only asks the runtimes and the
//! witness questions, and the cut is done by a person (a cable, a switch port, stopping the process).
//!
//! What it cannot show: whether the *tools* (consoles, lasers, media servers) can be reached from the standby; that is `pchi-gearcheck`, run against
//! the standby's rig from the standby's machine.

use crate::failover::http_t;
use crate::gearcheck::Console;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct Config {
    pub primary: String,
    pub standby: String,
    pub witness: Option<String>,
    pub key: String,
    pub token: Option<String>,
    /// Answers asked of each runtime for the delay figures.
    pub samples: usize,
    /// How long to wait for a takeover or a fence in the cut test.
    pub wait: Duration,
    pub poll: Duration,
}

impl Config {
    pub fn new(primary: &str, standby: &str, key: &str) -> Self {
        Config { primary: primary.into(), standby: standby.into(), witness: None, key: key.into(), token: None, samples: 10, wait: Duration::from_secs(90), poll: Duration::from_millis(200) }
    }
}

#[derive(Clone, Debug)]
pub struct Step {
    pub name: String,
    /// "pass", "warn", "fail" or "skipped".
    pub verdict: &'static str,
    pub detail: String,
}

fn step(name: &str, verdict: &'static str, detail: impl Into<String>) -> Step {
    Step { name: name.into(), verdict, detail: detail.into() }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

async fn get(addr: &str, path: &str, token: Option<&str>) -> Result<(Value, u128, i64), String> {
    let bearer = token.map(|t| format!("Bearer {t}"));
    let mut h: Vec<(&str, &str)> = Vec::new();
    if let Some(b) = &bearer {
        h.push(("Authorization", b));
    }
    let (t0, began) = (now_ms(), Instant::now());
    let (code, v) = http_t(addr, "GET", path, &h, None, Duration::from_secs(3)).await?;
    let took = began.elapsed().as_millis();
    if code != 200 {
        return Err(format!("{addr}{path} answered {code}: {}", v["error"].as_str().unwrap_or("?")));
    }
    Ok((v, took, (t0 + now_ms()) / 2))
}

fn median(v: &mut [u128]) -> u128 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

/// Delay and loss to one runtime, and how far its clock is from this machine's.
async fn reach(name: &str, addr: &str, cfg: &Config) -> (Step, Option<i64>) {
    let (mut times, mut lost, mut skew) = (Vec::new(), 0usize, Vec::new());
    for _ in 0..cfg.samples.max(1) {
        match get(addr, "/role", cfg.token.as_deref()).await {
            Ok((v, took, mid)) => {
                times.push(took);
                if let Some(ts) = v["ts"].as_i64() {
                    skew.push(ts - mid);
                }
            }
            Err(_) => lost += 1,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let n = cfg.samples.max(1);
    if times.is_empty() {
        return (step(&format!("reach the {name}"), "fail", format!("no answer from {addr} in {n} tries: is it running, and is the port open to this machine?")), None);
    }
    let (min, max, med) = (*times.iter().min().unwrap(), *times.iter().max().unwrap(), median(&mut times.clone()));
    let skew_ms = if skew.is_empty() { None } else { skew.sort_unstable(); Some(skew[skew.len() / 2]) };
    let detail = format!("{addr}: {} of {n} answered; answer time {min}/{med}/{max} ms (best/median/worst)", n - lost);
    let verdict = if lost * 3 > n { "fail" } else if lost > 0 || med > 100 || max > 500 { "warn" } else { "pass" };
    (step(&format!("reach the {name}"), verdict, detail), skew_ms)
}

/// Run every check that changes nothing. Returns the steps, in order.
pub async fn survey(cfg: &Config) -> Vec<Step> {
    let mut out = Vec::new();
    let (s1, skew_p) = reach("primary", &cfg.primary, cfg).await;
    let (s2, skew_s) = reach("standby", &cfg.standby, cfg).await;
    out.push(s1);
    out.push(s2);
    // clocks: the answers carry the runtime's own time
    match (skew_p, skew_s) {
        (Some(p), Some(s)) => {
            let worst = p.abs().max(s.abs());
            let v = if worst > 1_000 { "warn" } else { "pass" };
            out.push(step("clocks agree", v, format!("the primary's clock is {p} ms from this machine's, the standby's {s} ms (to within the answer time). Recordings and receipts carry these clocks; over 1 s apart makes them hard to line up. Cue timing does not depend on it.")));
        }
        _ => out.push(step("clocks agree", "skipped", "a runtime did not answer")),
    }
    // who is what
    let role = |addr: String, token: Option<String>| async move { get(&addr, "/role", token.as_deref()).await.map(|r| r.0) };
    let (p, s) = (role(cfg.primary.clone(), cfg.token.clone()).await, role(cfg.standby.clone(), cfg.token.clone()).await);
    match (&p, &s) {
        (Ok(p), Ok(s)) => {
            let ok = p["role"] == "active" && s["role"] == "standby";
            out.push(step(
                "roles are as expected",
                if ok { "pass" } else { "fail" },
                format!("the primary says {} (takeover {}), the standby says {}{}", p["role"], p["epoch"], s["role"], if ok { "" } else { ". One should be active and the other standby: has a takeover already happened, or are the addresses the wrong way round?" }),
            ));
            let heard = s["last_heard_ms_ago"].as_i64().unwrap_or(-1);
            let healthy = s["primary_health"] == "ok" && s["problem"].is_null() && s["copied_records"].as_i64().unwrap_or(0) > 0 && (0..4_000).contains(&heard);
            out.push(step(
                "the standby is following",
                if healthy { "pass" } else { "fail" },
                format!("copied {} records, last heard from the primary {} ms ago, it sees the primary as {}{}", s["copied_records"], heard, s["primary_health"], s["problem"].as_str().map(|p| format!("; it reports: {p}")).unwrap_or_default()),
            ));
            if p["lag_ms"].as_u64().unwrap_or(0) >= crate::failover::SLOW_MS {
                out.push(step("the primary keeps up", "warn", format!("its event loop is running {} ms behind its timers: the standby would call it slow", p["lag_ms"])));
            } else {
                out.push(step("the primary keeps up", "pass", format!("event loop {} ms behind its timers at worst over the last minute: {}", p["lag_worst_ms"], p["lag_ms"])));
            }
        }
        _ => out.push(step("roles are as expected", "fail", "a runtime did not answer /role")),
    }
    // the standby rebuilds the show
    match get(&cfg.standby, "/takeover-check", cfg.token.as_deref()).await {
        Ok((v, took, _)) => {
            let ready = v["ready"] == true;
            let mut d = format!("{} ({took} ms)", v["verdict"].as_str().unwrap_or(""));
            for p in v["problems"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                d += &format!("; {p}");
            }
            out.push(step("rehearse a takeover", if ready { "pass" } else { "fail" }, d));
        }
        Err(e) => out.push(step("rehearse a takeover", "fail", e)),
    }
    // the witness
    match &cfg.witness {
        None => out.push(step("the witness", "skipped", "no witness given: a cut link between the two can leave both sending until it is mended")),
        Some(w) => out.extend(witness_steps(w, cfg).await),
    }
    out
}

async fn witness_steps(w: &str, cfg: &Config) -> Vec<Step> {
    let mut out = Vec::new();
    let view = match get(w, "/primary", None).await {
        Ok((v, took, _)) => {
            let valid = v["valid"] == true;
            out.push(step("the witness answers", if valid { "pass" } else { "fail" }, format!("{w} answered in {took} ms: holder {}, lease {}", v["holder"], if valid { format!("valid for another {} ms", v["left_ms"]) } else { "not held: no runtime is holding it, so none is sending".to_string() })));
            v
        }
        Err(e) => {
            out.push(step("the witness answers", "fail", e));
            return out;
        }
    };
    // a stranger must be refused while the lease is valid
    if view["valid"] == true && view["left_ms"].as_i64().unwrap_or(0) > 1_500 {
        let body = serde_json::json!({"name": "netcheck", "ttl_ms": 2000}).to_string();
        match http_t(w, "POST", "/lease", &[("X-Pchi-Failover-Key", &cfg.key)], Some(&body), Duration::from_secs(3)).await {
            Ok((409, _)) => out.push(step("the witness refuses a second sender", "pass", "a claim for the lease by a third party was refused while the primary held it")),
            Ok((200, _)) => {
                let _ = http_t(w, "POST", "/release", &[("X-Pchi-Failover-Key", &cfg.key)], Some(&serde_json::json!({"name": "netcheck"}).to_string()), Duration::from_secs(3)).await;
                out.push(step("the witness refuses a second sender", "fail", "a third party was GRANTED the lease while the primary held it (it has been released again)"));
            }
            Ok((403, _)) => out.push(step("the witness refuses a second sender", "fail", "the witness refused this machine's failover key: it is not the same key the runtimes use")),
            Ok((c, _)) => out.push(step("the witness refuses a second sender", "fail", format!("unexpected answer {c}"))),
            Err(e) => out.push(step("the witness refuses a second sender", "fail", e)),
        }
    } else {
        out.push(step("the witness refuses a second sender", "skipped", "the lease was not valid for long enough to test it without taking it"));
    }
    out
}

async fn count_active(cfg: &Config) -> (usize, Vec<String>) {
    let mut who = Vec::new();
    for (name, a) in [("primary", &cfg.primary), ("standby", &cfg.standby)] {
        if let Ok((v, _, _)) = get(a, "/role", cfg.token.as_deref()).await {
            who.push(format!("{name}: {}", v["role"].as_str().unwrap_or("?")));
        } else {
            who.push(format!("{name}: no answer"));
        }
    }
    (who.iter().filter(|w| w.ends_with("active")).count(), who)
}

/// The cut test: a person cuts the primary off, this times the takeover; then the person restores it, and this times how long the old one takes to stop.
/// Returns steps; it changes nothing itself.
pub async fn cut_test(cfg: &Config, con: &mut dyn Console) -> Vec<Step> {
    let mut out = Vec::new();
    con.say("\nThe cut test. This will make the standby take over the show. Do it only when nothing is relying on the primary (no live audience, lasers disarmed).");
    con.say("Cut the primary off from the standby and the witness in the way you want to test: pull its network cable, switch off its port, or stop the program. (Cutting only the link to the standby is the interesting one.)");
    let a = con.ask("Type CUT at the moment you cut it (anything else skips the test):");
    if a.trim().to_ascii_uppercase() != "CUT" {
        return vec![step("the cut test", "skipped", "not run")];
    }
    let t0 = Instant::now();
    let mut took_over = None;
    while t0.elapsed() < cfg.wait {
        if let Ok((v, _, _)) = get(&cfg.standby, "/role", cfg.token.as_deref()).await {
            if v["role"] == "active" {
                took_over = Some(t0.elapsed());
                break;
            }
        }
        tokio::time::sleep(cfg.poll).await;
    }
    match took_over {
        Some(d) => out.push(step("the standby takes over", "pass", format!("{:.1} s after you typed CUT (it waits for the primary to be silent, and with a witness for the primary's lease to run out)", d.as_secs_f64()))),
        None => {
            out.push(step("the standby takes over", "fail", format!("it had not taken over after {:.0} s. With a witness that still saw the primary this is correct (the primary was only cut off from the standby); otherwise check --takeover and the standby's /role", cfg.wait.as_secs_f64())));
            return out;
        }
    }
    if let Some(w) = &cfg.witness {
        if let Ok((v, _, _)) = get(w, "/primary", None).await {
            out.push(step("the witness follows the show", if v["holder"].as_str().is_some_and(|h| h != "primary") { "pass" } else { "warn" }, format!("the witness says the lease is held by {} at {}", v["holder"], v["primary"])));
        }
    }
    let (n, who) = count_active(cfg).await;
    out.push(step("only one is sending", if n == 1 { "pass" } else { "warn" }, format!("{} ({n} say active)", who.join(", "))));
    con.say("\nNow restore the old primary: plug the cable back, or start the program again. Do NOT stop the standby.");
    let a = con.ask("Type DONE when it is back on the network (anything else skips the rest):");
    if a.trim().to_ascii_uppercase() != "DONE" {
        return out;
    }
    let t1 = Instant::now();
    let mut stopped = None;
    let mut ever_two = false;
    while t1.elapsed() < cfg.wait {
        let (n, _) = count_active(cfg).await;
        ever_two |= n > 1;
        if let Ok((v, _, _)) = get(&cfg.primary, "/role", cfg.token.as_deref()).await {
            if v["role"] == "fenced" || v["role"] == "paused" {
                stopped = Some((t1.elapsed(), v["role"].as_str().unwrap_or("").to_string()));
                break;
            }
        }
        tokio::time::sleep(cfg.poll).await;
    }
    match stopped {
        Some((d, role)) => out.push(step("the old primary stops", "pass", format!("{:.1} s after you typed DONE it said {role}{}", d.as_secs_f64(), if ever_two { "; for part of that time BOTH said active (this is the split brain the witness prevents)" } else { "" }))),
        None => out.push(step("the old primary stops", "fail", format!("it was still not fenced or paused after {:.0} s: two runtimes may be sending", cfg.wait.as_secs_f64()))),
    }
    let (n, who) = count_active(cfg).await;
    out.push(step("only one is sending at the end", if n == 1 { "pass" } else { "fail" }, format!("{} ({n} say active)", who.join(", "))));
    out
}

pub fn report_markdown(cfg: &Config, steps: &[Step], date: &str) -> String {
    let mut s = format!("# Failover day report, {date}\n\nPrimary `{}`, standby `{}`{}.\n\n| Check | Result | What was measured |\n|---|---|---|\n", cfg.primary, cfg.standby, cfg.witness.as_deref().map(|w| format!(", witness `{w}`")).unwrap_or_else(|| ", no witness".into()));
    for st in steps {
        let mark = match st.verdict {
            "pass" => "pass",
            "warn" => "**warning**",
            "fail" => "**FAIL**",
            _ => "skipped",
        };
        s += &format!("| {} | {mark} | {} |\n", st.name, st.detail.replace('|', "/"));
    }
    let fails = steps.iter().filter(|s| s.verdict == "fail").count();
    let warns = steps.iter().filter(|s| s.verdict == "warn").count();
    s += &format!("\n{}\n\nWhat this does not show: that the standby can reach the tools (run `pchi-gearcheck` on the standby's machine), or anything about a network other than the one it ran on.\n", if fails == 0 && warns == 0 { "Every check passed.".to_string() } else { format!("{fails} failed, {warns} with a warning.") });
    s
}
