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

/// One person (or one named agent) and the key that identifies them.
pub struct Member {
    pub name: String,
    pub role: String,
    key_hash: String,
}

/// Who may sign in, from a roster file: `[{"name": "Alex", "role": "operator", "key": "..."}]`
/// (or `{"people": [...]}`). Keys are compared by hash and never sent anywhere; the file itself holds them, so protect it.
#[derive(Default)]
pub struct Roster {
    members: Vec<Member>,
}

fn key_hash(key: &str) -> String {
    primeswarm_pchi::governance::hash_state(&Value::String(key.to_string()))
}

impl Roster {
    pub fn parse(text: &str) -> Result<Roster, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("the roster is not valid JSON: {e}"))?;
        let list = v.get("people").unwrap_or(&v).as_array().ok_or("the roster must be a list of people")?;
        let mut members: Vec<Member> = Vec::new();
        for (i, p) in list.iter().enumerate() {
            let name = clean(p["name"].as_str().unwrap_or(""));
            let role = p["role"].as_str().unwrap_or("").trim().to_ascii_lowercase();
            let key = p["key"].as_str().unwrap_or("");
            if name.is_empty() {
                return Err(format!("person {} has no name", i + 1));
            }
            if !ROLES.contains(&role.as_str()) {
                return Err(format!("{name}: role '{role}' is not one of {}", ROLES.join(", ")));
            }
            if key.len() < 8 {
                return Err(format!("{name}: the key must be at least 8 characters"));
            }
            let hash = key_hash(key);
            if members.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
                return Err(format!("{name} is listed twice"));
            }
            if members.iter().any(|m| m.key_hash == hash) {
                return Err(format!("{name} has the same key as someone else: each person needs their own"));
            }
            members.push(Member { name, role, key_hash: hash });
        }
        Ok(Roster { members })
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

    /// Who this key belongs to: a member, or a named agent when the role is "agent".
    pub fn find(&self, key: &str) -> Option<Actor> {
        let h = key_hash(key);
        self.members.iter().find(|m| m.key_hash == h).map(|m| if m.role == "agent" { Actor::Agent(m.name.clone()) } else { Actor::Member { name: m.name.clone(), role: m.role.clone() } })
    }

    /// "Alex (operator), Sam (lso)" for the startup line.
    pub fn summary(&self) -> String {
        self.members.iter().map(|m| format!("{} ({})", m.name, m.role)).collect::<Vec<_>>().join(", ")
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.members.iter().any(|m| m.role == role)
    }
}
