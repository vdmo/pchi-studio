//! The runtime's face on agent memory (`--memory FILE`): ask a question, write a note, list, verify, derive from the run.
//!
//! Who may do what is decided here, from who is really asking (`who`), not from what they claim: anyone who passes the token may **ask** (the memory says so when it does
//! not know) and **list**; an agent may **write** (always a draft); a person in the operator or editor role (or whoever holds the operator key) may also write
//! verified notes, **verify**, **supersede** and **derive** from the run. A guest, a laser safety officer and anyone unidentified on a locked show may only ask.
//! Verifying, superseding and deriving leave a signed receipt like any other decision.
//!
//! Memory never sits in the show's path: nothing here is called by a cue, a timer or a takeover, and if the memory file cannot be read the show does not notice.

use super::{authorized, unauthorized, who, App, Reply};
use crate::governance::Actor;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use pchi_memory::{Filter, Kind, New, RecallOpts, Status, Who};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Who the memory should treat this actor as, and whether they may write at all.
fn memory_who(a: &Actor) -> Option<Who> {
    match a {
        Actor::Person(n) => Some(Who::person(n.as_deref().unwrap_or("operator"))),
        Actor::Member { name, role } if role == "operator" || role == "editor" => Some(Who::person(name)),
        Actor::Member { name, role } if role == "agent" => Some(Who::agent(name)),
        Actor::Agent(n) => Some(Who::agent(n)),
        _ => None,
    }
}

fn off() -> Reply {
    (StatusCode::NOT_FOUND, Json(json!({"error": "memory is off: start the runtime with --memory FILE"})))
}

/// `GET /memory?q=…&kind=…&limit=…`: ask. The answer says `"answer": false` and why when memory does not know.
pub(crate) async fn get_memory(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let Some(question) = q.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "ask something: ?q=what went wrong with the haze cue"})));
    };
    let kind = match q.get("kind") {
        Some(k) => match Kind::parse(k) {
            Some(k) => Some(k),
            None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "kind is convention, venue, incident, decision, preference or task"}))),
        },
        None => None,
    };
    let opts = RecallOpts { limit: q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(5).clamp(1, 20), show: Some(a.show.show_id()), kind, reinforce: true };
    match mem.recall(question, &opts, now_ms()) {
        Ok(r) => (StatusCode::OK, Json(r.to_json())),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.0}))),
    }
}

/// `GET /memory/list?status=draft|verified|archived`
pub(crate) async fn get_memory_list(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let status = match q.get("status").map(String::as_str) {
        Some("draft") => Some(Status::Draft),
        Some("verified") => Some(Status::Verified),
        Some("archived") => Some(Status::Archived),
        _ => None,
    };
    match mem.list(&Filter { status, ..Default::default() }) {
        Ok(v) => (StatusCode::OK, Json(json!({"memories": v, "stats": mem.stats().ok()}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.0}))),
    }
}

/// `POST /memory` {"text", "kind", "key", "dept", "confidence", "verify"}: remember. An agent's note is a draft; `verify` is honoured only for a person.
pub(crate) async fn post_memory(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    let Some(w) = memory_who(&actor) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "this role may ask memory but not write to it"})));
    };
    let kind = body["kind"].as_str().map(|k| Kind::parse(k)).unwrap_or(Some(Kind::Convention));
    let Some(kind) = kind else { return (StatusCode::BAD_REQUEST, Json(json!({"error": "kind is convention, venue, incident, decision, preference or task"}))) };
    let new = New {
        kind,
        text: body["text"].as_str().unwrap_or("").to_string(),
        key: body["key"].as_str().map(String::from),
        show: a.show.show_id(),
        dept: body["dept"].as_str().unwrap_or("").to_string(),
        confidence: body["confidence"].as_f64().unwrap_or(0.5),
        verify: body["verify"].as_bool().unwrap_or(false),
        ..Default::default()
    };
    match mem.remember(&w, new, now_ms()) {
        Ok((r, fresh)) => (StatusCode::OK, Json(json!({"remembered": fresh, "id": r.id, "status": r.status.name(), "in_conflict_with": r.conflicts_with,
            "note": if r.status == Status::Draft { "stored as a draft: a person verifies it before it is relied on" } else { "stored verified" }}))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.0}))),
    }
}

/// `POST /memory/<id>/verify`: a person says it is right. Leaves a receipt.
pub(crate) async fn post_memory_verify(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    let Some(w) = memory_who(&actor).filter(|w| w.person) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "only a person in the operator or editor role may verify a memory"})));
    };
    match mem.verify(&w, &id, now_ms()) {
        Ok(r) => {
            a.show.receipt("memory.verify", "ALLOW", &actor, &format!("verified memory {}: {}", r.id, r.text.chars().take(120).collect::<String>()));
            (StatusCode::OK, Json(json!({"verified": true, "id": r.id})))
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e.0}))),
    }
}

/// `POST /memory/<id>/supersede` {"text"}: a person replaces a memory with a new, verified one. Leaves a receipt.
pub(crate) async fn post_memory_supersede(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    let Some(w) = memory_who(&actor).filter(|w| w.person) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "only a person in the operator or editor role may supersede a memory"})));
    };
    let old_kind = mem.get(&id).ok().flatten().map(|r| r.kind).unwrap_or_default();
    match mem.supersede(&w, &id, New { kind: old_kind, text: body["text"].as_str().unwrap_or("").to_string(), ..Default::default() }, now_ms()) {
        Ok(r) => {
            a.show.receipt("memory.supersede", "ALLOW", &actor, &format!("memory {id} replaced by {}: {}", r.id, r.text.chars().take(120).collect::<String>()));
            (StatusCode::OK, Json(json!({"superseded": id, "by": r.id})))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.0}))),
    }
}

/// `POST /memory/derive`: make drafts from this run's log (what failed, what ran late, interruptions, decisions). Doing it twice changes nothing.
pub(crate) async fn post_memory_derive(State(a): State<App>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    if !memory_who(&actor).is_some_and(|w| w.person) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "only a person in the operator or editor role may derive memory from the run"})));
    }
    match pchi_memory::derive::derive_into(&mem, &a.show.records(), &Who::agent("derive"), now_ms()) {
        Ok(d) => {
            a.show.receipt("memory.derive", "ALLOW", &actor, &format!("derived memory from the run: {} new draft(s), {} already known", d.new, d.already));
            (StatusCode::OK, Json(json!({"new": d.new, "already": d.already, "conflicts": d.conflicts, "next": "they are drafts: verify the ones worth keeping"})))
        }
        Err(e) => (StatusCode::CONFLICT, Json(json!({"error": e}))),
    }
}

fn now_ms() -> i64 {
    pchi_memory::now_ms()
}

/// `POST /memory/<id>/settle` {"over": "<id of the memory that is wrong>"}: two memories disagree; a person says this one is right. Leaves a receipt.
pub(crate) async fn post_memory_settle(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    let Some(w) = memory_who(&actor).filter(|w| w.person) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "only a person in the operator or editor role may settle a conflict between memories"})));
    };
    let over = body["over"].as_str().unwrap_or("");
    match mem.settle(&w, &id, over, now_ms()) {
        Ok(r) => {
            a.show.receipt("memory.settle", "ALLOW", &actor, &format!("memory {id} kept over {over}: {}", r.text.chars().take(120).collect::<String>()));
            (StatusCode::OK, Json(json!({"kept": id, "archived": over})))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.0}))),
    }
}

/// `POST /memory/<id>/archive`: a person says this is not worth remembering. It leaves the answers and stays in the audit trail. Leaves a receipt.
pub(crate) async fn post_memory_archive(State(a): State<App>, Path(id): Path<String>, q: Query<HashMap<String, String>>, h: HeaderMap) -> impl IntoResponse {
    if !authorized(&a.token, &q, &h) {
        return unauthorized();
    }
    let Some(mem) = a.show.memory() else { return off() };
    let actor = who(&a, &q, &h);
    let Some(w) = memory_who(&actor).filter(|w| w.person) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "only a person in the operator or editor role may archive a memory"})));
    };
    match mem.archive(&w, &id, now_ms()) {
        Ok(()) => {
            a.show.receipt("memory.archive", "ALLOW", &actor, &format!("archived memory {id}"));
            (StatusCode::OK, Json(json!({"archived": id})))
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e.0}))),
    }
}
