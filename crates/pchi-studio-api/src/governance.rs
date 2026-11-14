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
    Person(Option<String>),
    Agent(String),
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
    /// Hash of the show (and rig) this was proposed against: if the live show moves on, the proposal is superseded.
    pub base: String,
    pub chart: Showchart,
    pub devices: Option<Vec<rig::Device>>,
}

impl Proposal {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "actor": self.actor, "ts": self.ts, "status": self.status, "changes": self.changes, "notes": self.notes, "receipt": self.receipt})
    }
}

#[derive(Default)]
pub struct Gov {
    pub log: OnceLock<GovernanceLog>,
    pub proposals: Mutex<Vec<Proposal>>,
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
