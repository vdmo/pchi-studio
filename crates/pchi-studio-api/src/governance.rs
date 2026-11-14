//! Who may do what to a running show, and a signed record of every decision.
//!
//! Two kinds of actor:
//! - a **person**: whoever holds the operator key (or, with no key set, anyone who is not declared an agent);
//! - an **agent**: any client that declares itself with `X-Pchi-Actor: agent:<name>` (or `?actor=agent:<name>`
//!   on a WebSocket, where browsers cannot set headers).
//!
//! An agent may look (dry-run previews, reading the report), **propose** a show edit, and disarm lasers or hit
//! STOP LASERS (the safe direction). It may not send an edit, arm a laser, try or test a device, undo, or press
//! GO: those are refused. A proposed edit waits in a list until a person approves or rejects it.
//!
//! Every decision (allowed, refused, or waiting on a person) can be written as a signed, hash-chained receipt in
//! the same format as PrimeSwarm's governance log, so `scripts/verify_governance_chain.py` checks it offline.
//!
//! What this does NOT do: the actor header is a declaration, not a login. It restricts honest agents and gives the
//! record its names; an agent that is handed the operator key and lies about who it is can still act. Keep the
//! key away from agents. Real per-person identity is the next step (roles).

use crate::rig;
use axum::http::HeaderMap;
use pchi_showchart::Showchart;
use primeswarm_pchi::governance::GovernanceLog;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq)]
pub enum Actor {
    /// Someone who passed the operator key (or anyone, when the show is not locked). Full powers.
    Person(Option<String>),
    /// An agent: declared by header, or a roster entry with the role "agent".
    Agent(String),
    /// A named person from the roster, identified by their own key, with a role.
    Member { name: String, role: String },
    /// Locked show, no valid key.
    Anonymous,
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')).take(40).collect::<String>().trim().to_string()
}

impl Actor {
    /// Reads an actor declaration such as `agent:nova` or `person:alex`. Anything else is a person.
    pub fn parse(declared: Option<&str>) -> Actor {
        let d = declared.unwrap_or("").trim();
        let lower = d.to_ascii_lowercase();
        if lower == "agent" {
            return Actor::Agent("agent".into());
        }
        if let Some(rest) = lower.strip_prefix("agent:") {
            let name = clean(&d[d.len() - rest.len()..]);
            return Actor::Agent(if name.is_empty() { "agent".into() } else { name });
        }
        if let Some(rest) = lower.strip_prefix("person:") {
            let name = clean(&d[d.len() - rest.len()..]);
            return Actor::Person(Some(name).filter(|n| !n.is_empty()));
        }
        Actor::Person(None)
    }

    pub fn from_headers(h: &HeaderMap, query_actor: Option<&str>) -> Actor {
        Actor::parse(h.get("x-pchi-actor").and_then(|v| v.to_str().ok()).or(query_actor))
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Actor::Agent(_))
    }

    /// How the actor is written in receipts and messages.
    pub fn name(&self) -> String {
        match self {
            Actor::Person(Some(n)) => format!("person:{n}"),
            Actor::Person(None) => "operator".into(),
            Actor::Agent(n) => format!("agent:{n}"),
            Actor::Member { name, role } => format!("{name} ({role})"),
            Actor::Anonymous => "unidentified".into(),
        }
    }

    /// The role this actor acts in, for messages.
    pub fn role(&self) -> &str {
        match self {
            Actor::Person(_) => "operator",
            Actor::Agent(_) => "agent",
            Actor::Member { role, .. } => role,
            Actor::Anonymous => "none",
        }
    }
}

/// A show edit an agent proposed, waiting for a person.
pub struct Proposal {
    pub id: u32,
    pub actor: String,
    pub ts: i64,
    pub status: String,
    pub changes: Vec<String>,
    pub notes: Vec<String>,
    pub receipt: Option<String>,
    /// What the proposer said they wanted, in their own words (a narrated or typed line, or an agent's reason).
    pub said: String,
    /// How the edit would have played against the run so far, when it was proposed: {different, verdict, lines}.
    pub rehearsal: Value,
    /// Hash of the show (and rig) this was proposed against: if the live show moves on, the proposal is superseded.
    pub base: String,
    pub chart: Showchart,
    pub devices: Option<Vec<rig::Device>>,
}

impl Proposal {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "actor": self.actor, "ts": self.ts, "status": self.status, "changes": self.changes, "notes": self.notes, "receipt": self.receipt, "said": self.said, "rehearsal": self.rehearsal})
    }
}

#[derive(Default)]
pub struct Gov {
    pub log: OnceLock<GovernanceLog>,
    pub proposals: Mutex<Vec<Proposal>>,
}

/// A line someone said or typed, made safe to keep in a record: no control characters, at most 300 characters.
pub fn clean_said(s: Option<&str>) -> String {
    s.unwrap_or("").chars().filter(|c| !c.is_control()).take(300).collect::<String>().trim().to_string()
}

/// ` — said: "..."` for a message, or nothing when nothing was said.
pub fn said_suffix(said: &str) -> String {
    if said.is_empty() { String::new() } else { format!(" — said: \"{said}\"") }
}

/// How many proposals are kept (pending first-come; the oldest settled ones drop off).
pub const MAX_PROPOSALS: usize = 50;

impl Gov {
    pub fn pending_json(&self) -> Value {
        Value::Array(self.proposals.lock().unwrap().iter().filter(|p| p.status == "pending").map(Proposal::json).collect())
    }
    pub fn all_json(&self) -> Value {
        Value::Array(self.proposals.lock().unwrap().iter().map(Proposal::json).collect())
    }
}

/// What can be asked of a running show. Disarming a laser, STOP LASERS, rehearsing and reading need no permission.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Perm {
    /// Press GO, send inputs and context, reset.
    Go,
    /// Send an edited show (or preview one).
    Edit,
    /// Approve or reject an agent's proposal.
    Approve,
    Undo,
    /// Arm a laser.
    Arm,
    /// Send a cue to a device by hand.
    Try,
    /// Test-fire a device.
    Test,
}

pub const ALL_PERMS: [(Perm, &str); 7] = [(Perm::Go, "go"), (Perm::Edit, "edit"), (Perm::Approve, "approve"), (Perm::Undo, "undo"), (Perm::Arm, "arm"), (Perm::Try, "try"), (Perm::Test, "test")];

/// The roles a roster may give. An "lso" (laser safety officer) may only try or test lasers.
/// "button" may only press (GO and the show's own events): a Stream Deck or button box, also what every OSC input acts as.
pub const ROLES: [&str; 6] = ["operator", "editor", "lso", "guest", "agent", "button"];

pub fn role_allows(role: &str, perm: Perm) -> bool {
    match role {
        "operator" => true,
        "editor" => matches!(perm, Perm::Edit | Perm::Approve | Perm::Undo | Perm::Try | Perm::Test),
        "lso" => matches!(perm, Perm::Arm | Perm::Try | Perm::Test),
        "button" => perm == Perm::Go,
        _ => false, // guest, and agent (which proposes instead)
    }
}

/// How a person's key is checked: kept as a fast hash (a key written in the roster file in clear), or as a salted PBKDF2 hash of it
/// (the roster holds nothing from which the key can be read back).
enum Cred {
    Plain(String),
    Pbkdf2 { salt: Vec<u8>, iters: u32, hash: Vec<u8> },
}

/// One person (or one named agent) and what proves it is them.
pub struct Member {
    pub name: String,
    pub role: String,
    cred: Cred,
    /// When the key stops working (ms since the epoch), if it has an end date.
    expires_ms: Option<i64>,
}

/// Who may sign in, from a roster file: `[{"name": "Alex", "role": "operator", "salt": "...", "key_hash": "..."}]` (or `{"people": [...]}`).
/// A key may be written in clear (`"key"`) or, better, as the salted PBKDF2 hash `--new-roster-entry` makes. An entry may have
/// `"expires": "2026-12-31"` (or with a time, `"2026-12-31T18:00"`, UTC), after which its key is refused.
#[derive(Default)]
pub struct Roster {
    members: Vec<Member>,
    /// Keys already proved this run, by a fast hash of the key, so PBKDF2 is paid once, not on every request. Never written anywhere.
    proved: Mutex<HashMap<String, usize>>,
}

fn key_hash(key: &str) -> String {
    primeswarm_pchi::governance::hash_state(&Value::String(key.to_string()))
}

/// The PBKDF2-HMAC-SHA256 of a key.
pub fn pbkdf2_key(key: &str, salt: &[u8], iters: u32) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(key.as_bytes(), salt, iters, &mut out);
    out
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Days since 1970-01-01 for a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-12-31` (the end of that day) or `2026-12-31T18:00`, UTC, as ms since the epoch.
pub fn parse_expiry(text: &str) -> Result<i64, String> {
    let t = text.trim();
    let (date, time) = match t.split_once('T') {
        Some((d, tm)) => (d, Some(tm)),
        None => (t, None),
    };
    let p: Vec<&str> = date.split('-').collect();
    let n: Option<Vec<i64>> = p.iter().map(|x| x.parse().ok()).collect();
    let (Some(n), 3) = (n, p.len()) else { return Err(format!("expires '{text}': write 2026-12-31 or 2026-12-31T18:00")) };
    if !(1..=12).contains(&n[1]) || !(1..=31).contains(&n[2]) || !(2000..=2200).contains(&n[0]) {
        return Err(format!("expires '{text}' is not a real date"));
    }
    let day = days_from_civil(n[0], n[1], n[2]);
    let secs = match time {
        None => 86_400 - 1,
        Some(tm) => {
            let hm: Vec<i64> = tm.split(':').filter_map(|x| x.parse().ok()).collect();
            if hm.len() != 2 || hm[0] > 23 || hm[1] > 59 {
                return Err(format!("expires '{text}': the time is HH:MM"));
            }
            hm[0] * 3600 + hm[1] * 60
        }
    };
    Ok((day * 86_400 + secs) * 1000)
}

/// A fresh random key, and the roster entry that holds only its salted hash. The key is shown once: give it to the person.
pub fn new_entry(name: &str, role: &str, iters: u32) -> Result<(String, Value), String> {
    use rand::RngCore;
    let name = clean(name);
    if name.is_empty() {
        return Err("give a name".into());
    }
    if !ROLES.contains(&role) {
        return Err(format!("role '{role}' is not one of {}", ROLES.join(", ")));
    }
    let mut rng = rand::rngs::OsRng;
    let (mut key, mut salt) = ([0u8; 20], [0u8; 16]);
    rng.fill_bytes(&mut key);
    rng.fill_bytes(&mut salt);
    let key = hex::encode(key);
    let entry = json!({"name": name, "role": role, "iterations": iters, "salt": hex::encode(salt), "key_hash": hex::encode(pbkdf2_key(&key, &salt, iters))});
    Ok((key, entry))
}

impl Roster {
    pub fn parse(text: &str) -> Result<Roster, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("the roster is not valid JSON: {e}"))?;
        let list = v.get("people").unwrap_or(&v).as_array().ok_or("the roster must be a list of people")?;
        let mut members: Vec<Member> = Vec::new();
        for (i, p) in list.iter().enumerate() {
            let name = clean(p["name"].as_str().unwrap_or(""));
            let role = p["role"].as_str().unwrap_or("").trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(format!("person {} has no name", i + 1));
            }
            if !ROLES.contains(&role.as_str()) {
                return Err(format!("{name}: role '{role}' is not one of {}", ROLES.join(", ")));
            }
            if members.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
                return Err(format!("{name} is listed twice"));
            }
            let expires_ms = match p["expires"].as_str() {
                Some(e) => Some(parse_expiry(e).map_err(|e| format!("{name}: {e}"))?),
                None => None,
            };
            let cred = match (p["key"].as_str(), p["key_hash"].as_str()) {
                (Some(key), None) => {
                    if key.len() < 8 {
                        return Err(format!("{name}: the key must be at least 8 characters"));
                    }
                    let hash = key_hash(key);
                    if members.iter().any(|m| matches!(&m.cred, Cred::Plain(h) if *h == hash)) {
                        return Err(format!("{name} has the same key as someone else: each person needs their own"));
                    }
                    Cred::Plain(hash)
                }
                (None, Some(h)) => {
                    let salt = hex::decode(p["salt"].as_str().unwrap_or("")).map_err(|_| format!("{name}: salt must be hex"))?;
                    let hash = hex::decode(h).map_err(|_| format!("{name}: key_hash must be hex"))?;
                    let iters = p["iterations"].as_u64().unwrap_or(200_000);
                    if salt.len() < 8 || hash.len() != 32 || !(1_000..=5_000_000).contains(&iters) {
                        return Err(format!("{name}: an entry needs a salt of at least 8 bytes, a 32-byte key_hash and iterations from 1000 to 5000000: make it with --new-roster-entry"));
                    }
                    Cred::Pbkdf2 { salt, iters: iters as u32, hash }
                }
                _ => return Err(format!("{name}: give either \"key\" or \"key_hash\" with \"salt\", not both and not neither")),
            };
            members.push(Member { name, role, cred, expires_ms });
        }
        Ok(Roster { members, proved: Mutex::new(HashMap::new()) })
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.find(key).is_some()
    }

    fn actor_of(m: &Member) -> Actor {
        if m.role == "agent" { Actor::Agent(m.name.clone()) } else { Actor::Member { name: m.name.clone(), role: m.role.clone() } }
    }

    /// Who this key belongs to, if it is known and has not expired.
    pub fn find(&self, key: &str) -> Option<Actor> {
        self.find_at(key, i64::MIN).ok().flatten()
    }

    /// Like `find`, but says when the key is known and has expired (`now_ms` as ms since the epoch).
    pub fn find_at(&self, key: &str, now_ms: i64) -> Result<Option<Actor>, String> {
        let digest = key_hash(key);
        let cached = self.proved.lock().unwrap().get(&digest).copied();
        let idx = match cached {
            Some(i) => Some(i),
            None => {
                let hit = self.members.iter().position(|m| match &m.cred {
                    Cred::Plain(h) => *h == digest,
                    Cred::Pbkdf2 { salt, iters, hash } => same(&pbkdf2_key(key, salt, *iters), hash),
                });
                if let Some(i) = hit {
                    self.proved.lock().unwrap().insert(digest, i);
                }
                hit
            }
        };
        let Some(m) = idx.map(|i| &self.members[i]) else { return Ok(None) };
        if m.expires_ms.is_some_and(|e| now_ms > e) {
            return Err(format!("the key for {} has expired", m.name));
        }
        Ok(Some(Self::actor_of(m)))
    }

    /// How many people's keys are written in the roster file in clear.
    pub fn plaintext_count(&self) -> usize {
        self.members.iter().filter(|m| matches!(m.cred, Cred::Plain(_))).count()
    }

    /// "Alex until 2026-12-31" lines for the startup report: whose key has an end.
    pub fn expiring(&self) -> Vec<(String, i64)> {
        self.members.iter().filter_map(|m| m.expires_ms.map(|e| (m.name.clone(), e))).collect()
    }

    /// "Alex (operator), Sam (lso)" for the startup line.
    pub fn summary(&self) -> String {
        self.members.iter().map(|m| format!("{} ({})", m.name, m.role)).collect::<Vec<_>>().join(", ")
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.members.iter().any(|m| m.role == role)
    }
}

/// How many entries of a roster text are written in clear (a helper for the tests and the startup warning).
pub fn governance_plain_count_for(text: &str) -> usize {
    Roster::parse(text).map(|r| r.plaintext_count()).unwrap_or(0)
}
