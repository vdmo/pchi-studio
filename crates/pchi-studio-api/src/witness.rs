//! A witness: a third process that decides which runtime may send, so a cut network cannot put two runtimes on the same lights.
//!
//! Without a witness, a standby that cannot reach the primary has to guess whether the primary is dead or only out of reach, and when it guesses
//! wrong both send until the link is mended (docs/failover.md). With one, sending is a lease:
//!
//! * The runtime that runs the show claims the lease from the witness every few seconds. While the witness says yes it sends. If the witness
//!   says no, or cannot be reached for as long as the lease lasts, it *pauses*: it sends nothing, disarms its lasers, and goes on by itself
//!   when it holds the lease again.
//! * A standby that wants to take over must first be granted the lease, and the witness grants it only after the primary's lease has run out.
//!   A primary that is alive and can reach the witness keeps its lease, so a standby that merely cannot reach the primary is told no.
//!
//! The witness keeps nothing but who holds the lease and until when, in memory. It is a few hundred lines on purpose: put it on a machine of its
//! own (a small board is enough) that fails separately from both runtimes.
//!
//! What this buys, and what it does not: the primary stops sending at most one lease length after it last held the lease, and the standby
//! starts no sooner than that, so the two never send at once (to within the clocks' running rate; the primary counts its lease from when it
//! *asked*, not from when the witness answered). If the witness is down, nothing can take over: the runtime that holds the lease keeps it
//! only until it runs out, and then pauses. That is chosen: a show that stops is safer than a show that is doubled. `POST /takeover?witness=ignore`
//! is for a person who knows the witness is down and the primary is dead.

use crate::failover::http_t;
use crate::Show;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MIN_TTL_MS: u64 = 2_000;
pub const MAX_TTL_MS: u64 = 120_000;
pub const DEFAULT_TTL_MS: u64 = 6_000;

/// How a runtime reaches the witness.
#[derive(Clone, Debug)]
pub struct Cfg {
    /// `host:port` of the witness.
    pub addr: String,
    /// The failover key (the witness is started with the same one).
    pub key: String,
    /// This runtime's name as the witness shows it.
    pub name: String,
    /// Where others can reach this runtime (`--advertise`), told to the witness so `GET /primary` there can say where the show is.
    pub url: Option<String>,
    pub ttl: Duration,
}

impl Cfg {
    pub fn new(addr: &str, key: &str, name: &str, url: Option<String>, ttl_ms: u64) -> Self {
        Cfg { addr: addr.to_string(), key: key.to_string(), name: name.to_string(), url, ttl: Duration::from_millis(ttl_ms.clamp(MIN_TTL_MS, MAX_TTL_MS)) }
    }
}

#[derive(Clone, Debug)]
struct Held {
    holder: String,
    url: Option<String>,
    until_ms: i64,
}

/// The answer to a claim.
#[derive(Clone, Debug)]
pub struct Claim {
    pub granted: bool,
    pub holder: String,
    pub url: Option<String>,
    /// How many times the lease has changed hands.
    pub changes: u64,
    /// How much of the lease is left (to the holder: the time it may keep sending without asking again).
    pub left_ms: i64,
}

impl Claim {
    fn to_json(&self) -> Value {
        json!({"granted": self.granted, "holder": self.holder, "url": self.url, "changes": self.changes, "left_ms": self.left_ms})
    }
}

/// Who holds the lease. Time is passed in, so the rules can be tested without waiting.
#[derive(Default)]
pub struct Registry {
    held: Option<Held>,
    changes: u64,
}

impl Registry {
    /// `name` asks for the lease for `ttl_ms`. Granted if nobody holds it, the holder's time has run out, or `name` already holds it (then it is extended).
    pub fn claim(&mut self, now: i64, name: &str, url: Option<&str>, ttl_ms: i64) -> Claim {
        if let Some(h) = &self.held {
            if h.until_ms > now && h.holder != name {
                return Claim { granted: false, holder: h.holder.clone(), url: h.url.clone(), changes: self.changes, left_ms: h.until_ms - now };
            }
        }
        if self.held.as_ref().map_or(true, |h| h.holder != name) {
            self.changes += 1;
        }
        let url = url.map(String::from).or_else(|| self.held.as_ref().filter(|h| h.holder == name).and_then(|h| h.url.clone()));
        self.held = Some(Held { holder: name.to_string(), url: url.clone(), until_ms: now + ttl_ms });
        Claim { granted: true, holder: name.to_string(), url, changes: self.changes, left_ms: ttl_ms }
    }

    /// `name` gives the lease up (a clean handover), so the next to ask has it at once.
    pub fn release(&mut self, name: &str) -> bool {
        if self.held.as_ref().is_some_and(|h| h.holder == name) {
            self.held = None;
            return true;
        }
        false
    }

    pub fn view(&self, now: i64) -> Value {
        match &self.held {
            Some(h) => json!({"holder": h.holder, "url": h.url, "valid": h.until_ms > now, "left_ms": (h.until_ms - now).max(0), "changes": self.changes}),
            None => json!({"holder": null, "url": null, "valid": false, "left_ms": 0, "changes": self.changes}),
        }
    }
}

pub struct Witness {
    reg: Mutex<Registry>,
    key: String,
}

impl Witness {
    pub fn new(key: &str) -> Arc<Self> {
        Arc::new(Witness { reg: Mutex::new(Registry::default()), key: key.to_string() })
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn key_ok(w: &Witness, h: &HeaderMap) -> bool {
    let given = h.get("x-pchi-failover-key").and_then(|v| v.to_str().ok()).unwrap_or("");
    !w.key.is_empty() && given.len() == w.key.len() && given.bytes().zip(w.key.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

/// The witness's HTTP face. `POST /lease` and `POST /release` need the failover key; `GET /primary` and `GET /lease` say who holds the lease and are open,
/// so a button box or a gateway can ask where the show is.
pub fn router(w: Arc<Witness>) -> Router {
    Router::new().route("/lease", post(post_lease).get(get_lease)).route("/release", post(post_release)).route("/primary", get(get_primary)).with_state(w)
}

async fn post_lease(State(w): State<Arc<Witness>>, h: HeaderMap, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    if !key_ok(&w, &h) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "the failover key is wrong"})));
    }
    let Some(name) = b["name"].as_str().filter(|n| !n.is_empty() && n.len() <= 60) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "send {\"name\": \"who you are\", \"ttl_ms\": N}"})));
    };
    let ttl = b["ttl_ms"].as_u64().unwrap_or(DEFAULT_TTL_MS).clamp(MIN_TTL_MS, MAX_TTL_MS) as i64;
    let url = b["url"].as_str().filter(|u| u.len() <= 200);
    let c = w.reg.lock().unwrap().claim(now_ms(), name, url, ttl);
    (if c.granted { StatusCode::OK } else { StatusCode::CONFLICT }, Json(c.to_json()))
}

async fn post_release(State(w): State<Arc<Witness>>, h: HeaderMap, Json(b): Json<Value>) -> (StatusCode, Json<Value>) {
    if !key_ok(&w, &h) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "the failover key is wrong"})));
    }
    let released = b["name"].as_str().is_some_and(|n| w.reg.lock().unwrap().release(n));
    (StatusCode::OK, Json(json!({"released": released})))
}

async fn get_lease(State(w): State<Arc<Witness>>) -> Json<Value> {
    Json(w.reg.lock().unwrap().view(now_ms()))
}

/// Where the show is: the holder's address if the lease is valid, null if nobody holds it (a takeover may be under way).
async fn get_primary(State(w): State<Arc<Witness>>) -> Json<Value> {
    let v = w.reg.lock().unwrap().view(now_ms());
    let valid = v["valid"].as_bool().unwrap_or(false);
    Json(json!({"primary": if valid { v["url"].clone() } else { Value::Null }, "holder": v["holder"], "valid": valid, "left_ms": v["left_ms"], "changes": v["changes"], "ts": now_ms()}))
}

/// Ask the witness for the lease. `Err` means the witness could not be reached or did not make sense.
pub async fn claim(cfg: &Cfg) -> Result<Claim, String> {
    let body = json!({"name": cfg.name, "url": cfg.url, "ttl_ms": cfg.ttl.as_millis() as u64}).to_string();
    let wait = (cfg.ttl / 3).clamp(Duration::from_millis(700), Duration::from_secs(3));
    let (code, v) = http_t(&cfg.addr, "POST", "/lease", &[("X-Pchi-Failover-Key", &cfg.key)], Some(&body), wait).await?;
    if code == 403 {
        return Err(format!("the witness at {} refused the failover key", cfg.addr));
    }
    if code != 200 && code != 409 {
        return Err(format!("the witness at {} answered {code}: {}", cfg.addr, v["error"].as_str().unwrap_or("?")));
    }
    Ok(Claim {
        granted: v["granted"].as_bool().unwrap_or(false),
        holder: v["holder"].as_str().unwrap_or("").to_string(),
        url: v["url"].as_str().map(String::from),
        changes: v["changes"].as_u64().unwrap_or(0),
        left_ms: v["left_ms"].as_i64().unwrap_or(0),
    })
}

/// Give the lease up (best effort), so a handover on purpose does not wait for it to run out.
pub async fn release(cfg: &Cfg) {
    let body = json!({"name": cfg.name}).to_string();
    let _ = http_t(&cfg.addr, "POST", "/release", &[("X-Pchi-Failover-Key", &cfg.key)], Some(&body), Duration::from_secs(2)).await;
}

/// Hold the lease for `show` for as long as it runs: ask again every quarter of a lease; pause the show the moment the witness says someone else holds
/// it, or when it has been silent for the whole lease; go on when it is held again. `held_since` is when a claim that already succeeded was sent
/// (the lease is counted from then), or `None` if there has been none yet (then the show is paused until the first).
pub fn hold(show: Arc<Show>, cfg: Cfg, held_since: Option<Instant>) {
    let valid_for = cfg.ttl.mul_f64(0.9);
    let mut valid_until = held_since.map(|t| t + valid_for);
    show.set_witness_view(json!({"addr": cfg.addr, "holding": valid_until.is_some(), "note": "holding the lease", "ttl_ms": cfg.ttl.as_millis() as u64}));
    if valid_until.is_none() {
        show.pause_lease(&format!("it has not yet been granted the lease by the witness at {}", cfg.addr));
    }
    tokio::spawn(async move {
        let every = (cfg.ttl / 4).max(Duration::from_millis(250));
        loop {
            tokio::time::sleep(every).await;
            if show.is_taken_over() {
                return; // fenced for good: another runtime runs the show
            }
            let sent = Instant::now();
            match claim(&cfg).await {
                Ok(c) if c.granted => {
                    valid_until = Some(sent + valid_for);
                    show.set_witness_view(json!({"addr": cfg.addr, "holding": true, "note": "holding the lease", "ttl_ms": cfg.ttl.as_millis() as u64, "changes": c.changes}));
                    show.resume_lease();
                }
                Ok(c) => {
                    valid_until = None;
                    let note = format!("the witness says {} holds the lease ({:.0} s left)", c.holder, c.left_ms as f64 / 1000.0);
                    show.set_witness_view(json!({"addr": cfg.addr, "holding": false, "note": note, "holder": c.holder, "ttl_ms": cfg.ttl.as_millis() as u64}));
                    show.pause_lease(&note);
                }
                Err(e) => {
                    let lapsed = valid_until.map_or(true, |u| Instant::now() >= u);
                    let note = if lapsed { format!("the witness could not be reached and the lease has run out: {e}") } else { format!("the witness could not be reached (the lease still holds): {e}") };
                    show.set_witness_view(json!({"addr": cfg.addr, "holding": !lapsed, "note": note, "ttl_ms": cfg.ttl.as_millis() as u64}));
                    if lapsed {
                        show.pause_lease(&note);
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_to_ask_holds_it_and_another_is_refused_until_it_runs_out() {
        let mut r = Registry::default();
        let a = r.claim(1_000, "primary", Some("10.0.0.1:3100"), 6_000);
        assert!(a.granted && a.changes == 1, "{a:?}");
        let b = r.claim(2_000, "standby", None, 6_000);
        assert!(!b.granted && b.holder == "primary" && b.left_ms == 5_000, "{b:?}");
        // the holder renews: extended, not a change of hands
        let a2 = r.claim(5_000, "primary", None, 6_000);
        assert!(a2.granted && a2.changes == 1 && a2.url.as_deref() == Some("10.0.0.1:3100"), "{a2:?}");
        // it stops asking: the standby is refused until the lease runs out, then granted
        assert!(!r.claim(10_999, "standby", None, 6_000).granted);
        let s = r.claim(11_001, "standby", Some("10.0.0.2:3100"), 6_000);
        assert!(s.granted && s.changes == 2, "{s:?}");
        // and now the old primary is the one refused
        assert!(!r.claim(12_000, "primary", None, 6_000).granted);
    }

    #[test]
    fn releasing_hands_it_over_at_once_and_only_for_the_holder() {
        let mut r = Registry::default();
        r.claim(0, "a", None, 6_000);
        assert!(!r.release("b"));
        assert!(!r.claim(1, "b", None, 6_000).granted);
        assert!(r.release("a"));
        assert!(r.claim(2, "b", None, 6_000).granted);
    }

    #[test]
    fn the_view_says_where_the_show_is_only_while_the_lease_is_valid() {
        let mut r = Registry::default();
        assert_eq!(r.view(0)["valid"], false);
        r.claim(0, "a", Some("h:1"), 3_000);
        assert_eq!((r.view(1_000)["valid"].clone(), r.view(1_000)["url"].clone()), (json!(true), json!("h:1")));
        assert_eq!(r.view(3_001)["valid"], false);
    }
}
