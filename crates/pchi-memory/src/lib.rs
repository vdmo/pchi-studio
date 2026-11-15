//! Agent memory for PCHI shows.
//!
//! The idea, taken from how a stage manager works rather than how a chatbot does: remember **decisions and evidence, not conversations**. A memory here is
//! a short statement with a kind, a source, the hash of the evidence it came from, and a status. It can be wrong, so it carries how much it should be trusted
//! and how fast that fades.
//!
//! * **Typed, with a half-life.** A convention (how this department likes things) lasts weeks, a venue quirk longer, an incident a few weeks, a decision about
//!   a week, a preference days, a task hours. Forgetting is a curve, not a cut-off, and what is faint enough is archived by `sweep`.
//! * **Draft until a person verifies.** Whatever an agent writes, and whatever is derived from a run, is a *draft*: it can be recalled, weighted at 0.55 of a
//!   verified one, and shown as unverified. Only a person verifies, supersedes, pins or forgets. Agents never raise their own memory's standing.
//! * **Conflicts are surfaced, not merged.** Two memories under the same key that say different things are both kept and marked; recall will not choose
//!   between them when they are close, it says they disagree. A person settles it with `settle` (this one is right, that one is wrong; the wrong one is archived with its lineage) or replaces one with new words with `supersede`.
//! * **Recall can say it does not know.** It declines when nothing is remembered, nothing remembered is about the question, what is about it is too old or
//!   too uncertain, or two memories disagree. A wrong confident answer is worse than none in a control room.
//! * **Recalling is not evidence.** How often a memory has been recalled adds a small bounded bonus to ranking; it never changes how true it is.
//! * **Everything is audited.** Each write, recall, verification, supersession, expiry and deletion is a row in an append-only audit table.
//! * **Derived, so rebuildable.** `derive` makes drafts from a recording (failed sends, late cues, interruptions, decisions) with deterministic ids and the hash
//!   of the records they came from, so the same run gives the same memories and a memory whose evidence no longer matches can be told.
//!
//! Search is local: BM25 plus a hashed character-trigram vector, fused (see `search`). It is not a neural embedding, and `eval` is how to find out what it
//! gets wrong on your own questions.

pub mod derive;
pub mod eval;
pub mod search;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(format!("memory store: {e}"))
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(format!("memory record: {e}"))
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// What a memory is about. Each fades at its own rate.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// How this show or department likes things done.
    #[default]
    Convention,
    /// A fact about a venue, its network or its gear.
    Venue,
    /// Something that went wrong, and what it was.
    Incident,
    /// Something a person decided (an edit approved, a proposal rejected) and why.
    Decision,
    Preference,
    Task,
}

impl Kind {
    pub fn half_life_hours(self) -> f64 {
        match self {
            Kind::Convention => 35.0 * 24.0,
            Kind::Venue => 60.0 * 24.0,
            Kind::Incident => 21.0 * 24.0,
            Kind::Decision => 7.0 * 24.0,
            Kind::Preference => 4.0 * 24.0,
            Kind::Task => 23.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Convention => "convention",
            Kind::Venue => "venue",
            Kind::Incident => "incident",
            Kind::Decision => "decision",
            Kind::Preference => "preference",
            Kind::Task => "task",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        [Kind::Convention, Kind::Venue, Kind::Incident, Kind::Decision, Kind::Preference, Kind::Task].into_iter().find(|k| k.name() == s.trim().to_ascii_lowercase())
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Draft,
    Verified,
    Archived,
}

impl Status {
    /// How much a memory's standing counts toward recall.
    pub fn weight(self) -> f64 {
        match self {
            Status::Verified => 1.0,
            Status::Draft => 0.55,
            Status::Archived => 0.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Verified => "verified",
            Status::Archived => "archived",
        }
    }
}

/// Who is writing. Only a person may verify, supersede, pin or forget.
#[derive(Clone, Debug)]
pub struct Who {
    pub name: String,
    pub person: bool,
}

impl Who {
    pub fn person(name: &str) -> Who {
        Who { name: name.into(), person: true }
    }
    pub fn agent(name: &str) -> Who {
        Who { name: name.into(), person: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Record {
    pub id: String,
    pub kind: Kind,
    /// An optional topic: two memories with the same key and show that disagree are in conflict.
    pub key: Option<String>,
    pub text: String,
    pub status: Status,
    /// How sure whoever wrote it was (0 to 1). Asserted, not inferred.
    pub confidence: f64,
    /// `run`, `receipt`, `person` or `agent`.
    pub source_type: String,
    pub source_ref: String,
    /// Hashes of the records this came from (see `derive::check_evidence`).
    pub evidence: Vec<String>,
    pub show: String,
    pub dept: String,
    pub author: String,
    pub created_ms: i64,
    pub verified_ms: Option<i64>,
    pub verified_by: Option<String>,
    pub last_recalled_ms: Option<i64>,
    pub recalls: u32,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    pub pinned: bool,
    pub conflicts_with: Vec<String>,
}

/// A memory to be written.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct New {
    pub kind: Kind,
    pub text: String,
    pub key: Option<String>,
    pub show: String,
    pub dept: String,
    pub confidence: f64,
    pub source_type: String,
    pub source_ref: String,
    pub evidence: Vec<String>,
    /// Honoured only for a person: write it already verified.
    pub verify: bool,
}

impl Default for New {
    fn default() -> Self {
        New { kind: Kind::Convention, text: String::new(), key: None, show: String::new(), dept: String::new(), confidence: 0.5, source_type: String::new(), source_ref: String::new(), evidence: vec![], verify: false }
    }
}

impl New {
    pub fn new(kind: Kind, text: &str) -> New {
        New { kind, text: text.into(), ..Default::default() }
    }
}

pub fn make_id(kind: Kind, key: &Option<String>, text: &str, show: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("{}|{}|{}|{}", kind.name(), key.as_deref().unwrap_or(""), text.trim(), show));
    hex::encode(h.finalize())[..16].to_string()
}

/// How much of its standing a memory has kept: 1 when new, half after one half-life of its kind. Pinned ones do not fade.
pub fn decay(r: &Record, now: i64) -> f64 {
    if r.pinned {
        return 1.0;
    }
    let age_h = ((now - r.verified_ms.unwrap_or(r.created_ms)).max(0) as f64) / 3_600_000.0;
    0.5f64.powf(age_h / r.kind.half_life_hours())
}

#[derive(Clone, Debug)]
pub struct RecallOpts {
    pub limit: usize,
    pub show: Option<String>,
    pub kind: Option<Kind>,
    /// Count this recall toward the memories' reinforcement (default true).
    pub reinforce: bool,
}

impl Default for RecallOpts {
    fn default() -> Self {
        RecallOpts { limit: 5, show: None, kind: None, reinforce: true }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Hit {
    pub record: Record,
    pub activation: f64,
    pub alignment: f64,
    pub weight: f64,
    pub decay: f64,
    pub reinforcement: f64,
    /// `verified` or `draft`: what the reader may lean on.
    pub trust: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Recall {
    pub hits: Vec<Hit>,
    /// Why there is no answer, if there is none.
    pub abstained: Option<String>,
    /// What came closest when it abstained (not reinforced; for a person to look at).
    pub closest: Vec<Hit>,
}

impl Recall {
    pub fn to_json(&self) -> Value {
        let h = |x: &Hit| json!({"id": x.record.id, "kind": x.record.kind.name(), "text": x.record.text, "key": x.record.key, "trust": x.trust, "status": x.record.status.name(), "confidence": x.record.confidence,
            "source": x.record.source_type, "show": x.record.show, "dept": x.record.dept, "score": (x.activation * 1000.0).round() / 1000.0, "alignment": (x.alignment * 1000.0).round() / 1000.0,
            "decay": (x.decay * 1000.0).round() / 1000.0, "conflicts_with": x.record.conflicts_with, "created_ms": x.record.created_ms});
        json!({"answer": self.abstained.is_none(), "abstained": self.abstained, "memories": self.hits.iter().map(h).collect::<Vec<_>>(), "closest": self.closest.iter().map(h).collect::<Vec<_>>()})
    }
}

/// Below this share of the question's content (weighted by how rare each word is in memory), nothing remembered is about it. Chosen after the sample set showed 0.25 answering questions about things never written down; measure yours.
pub const MIN_ALIGNMENT: f64 = 0.45;
/// Below this, what is about it is too old or too uncertain to lean on.
pub const MIN_ACTIVATION: f64 = 0.12;
/// Two conflicting memories closer than this cannot be chosen between.
pub const AMBIGUOUS_MARGIN: f64 = 0.05;
/// Below this share of its standing, a memory is archived by `sweep`.
pub const EXPIRES_BELOW: f64 = 0.05;

pub struct Memory {
    conn: Mutex<Connection>,
}

#[derive(Default, Clone, Debug)]
pub struct Filter {
    pub status: Option<Status>,
    pub kind: Option<Kind>,
    pub show: Option<String>,
    pub limit: Option<usize>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory (id TEXT PRIMARY KEY, status TEXT NOT NULL, kind TEXT NOT NULL, key TEXT, show TEXT NOT NULL, data TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS memory_key ON memory(key);
CREATE TABLE IF NOT EXISTS audit (seq INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, action TEXT NOT NULL, mem_id TEXT, by_name TEXT NOT NULL, detail TEXT NOT NULL);
";

fn person_only(who: &Who, what: &str) -> Result<()> {
    if who.person {
        Ok(())
    } else {
        Err(Error(format!("only a person may {what}: an agent's memory stays a draft until a person verifies it")))
    }
}

impl Memory {
    pub fn open(path: &str) -> Result<Memory> {
        let c = Connection::open(path)?;
        c.execute_batch(SCHEMA)?;
        Ok(Memory { conn: Mutex::new(c) })
    }

    pub fn in_memory() -> Result<Memory> {
        let c = Connection::open_in_memory()?;
        c.execute_batch(SCHEMA)?;
        Ok(Memory { conn: Mutex::new(c) })
    }

    fn save(&self, r: &Record) -> Result<()> {
        self.conn.lock().unwrap().execute("INSERT OR REPLACE INTO memory (id, status, kind, key, show, data) VALUES (?1,?2,?3,?4,?5,?6)", params![r.id, r.status.name(), r.kind.name(), r.key, r.show, serde_json::to_string(r)?])?;
        Ok(())
    }

    fn audit_row(&self, now: i64, action: &str, id: Option<&str>, by: &str, detail: &str) {
        let _ = self.conn.lock().unwrap().execute("INSERT INTO audit (ts, action, mem_id, by_name, detail) VALUES (?1,?2,?3,?4,?5)", params![now, action, id, by, detail]);
    }

    pub fn get(&self, id: &str) -> Result<Option<Record>> {
        let c = self.conn.lock().unwrap();
        match c.query_row("SELECT data FROM memory WHERE id = ?1", params![id], |r| r.get::<_, String>(0)) {
            Ok(d) => Ok(Some(serde_json::from_str(&d)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list(&self, f: &Filter) -> Result<Vec<Record>> {
        let c = self.conn.lock().unwrap();
        let mut q = c.prepare("SELECT data FROM memory ORDER BY rowid")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for d in rows {
            let r: Record = serde_json::from_str(&d?)?;
            if f.status.is_some_and(|s| s != r.status) || f.kind.is_some_and(|k| k != r.kind) || f.show.as_ref().is_some_and(|s| *s != r.show) {
                continue;
            }
            out.push(r);
            if f.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
        Ok(out)
    }

    /// Write a memory. It is a **draft** unless a person asks for it verified. Writing what is already there changes nothing. A memory under a key where
    /// another, different one already stands is marked in conflict with it (both ways); nothing is merged.
    pub fn remember(&self, who: &Who, new: New, now: i64) -> Result<(Record, bool)> {
        let text = new.text.trim().to_string();
        if text.is_empty() || text.chars().count() > 2000 {
            return Err(Error("a memory is a sentence or a few, up to 2000 characters".into()));
        }
        let key = new.key.clone().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let id = make_id(new.kind, &key, &text, &new.show);
        if let Some(mut r) = self.get(&id)? {
            if r.status == Status::Archived {
                if r.superseded_by.is_some() {
                    return Err(Error(format!("that was superseded by {}: write the newer one, or ask a person to supersede again", r.superseded_by.clone().unwrap_or_default())));
                }
                r.status = Status::Draft;
                self.save(&r)?;
                self.audit_row(now, "revive", Some(&id), &who.name, "an archived memory was written again");
                return Ok((r, true));
            }
            return Ok((r, false));
        }
        let verified = new.verify && who.person;
        let mut rec = Record {
            id: id.clone(),
            kind: new.kind,
            key: key.clone(),
            text,
            status: if verified { Status::Verified } else { Status::Draft },
            confidence: new.confidence.clamp(0.0, 1.0),
            source_type: if new.source_type.is_empty() { if who.person { "person".into() } else { "agent".into() } } else { new.source_type.clone() },
            source_ref: new.source_ref.clone(),
            evidence: new.evidence.clone(),
            show: new.show.clone(),
            dept: new.dept.clone(),
            author: who.name.clone(),
            created_ms: now,
            verified_ms: if verified { Some(now) } else { None },
            verified_by: if verified { Some(who.name.clone()) } else { None },
            last_recalled_ms: None,
            recalls: 0,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            conflicts_with: vec![],
        };
        if let Some(k) = &key {
            for mut other in self.list(&Filter { show: Some(rec.show.clone()), ..Default::default() })? {
                if other.key.as_deref() == Some(k.as_str()) && other.status != Status::Archived && other.id != id && other.text != rec.text {
                    other.conflicts_with.push(id.clone());
                    rec.conflicts_with.push(other.id.clone());
                    self.save(&other)?;
                }
            }
        }
        self.save(&rec)?;
        self.audit_row(now, "remember", Some(&id), &who.name, &format!("{} ({}, {}){}", rec.kind.name(), rec.status.name(), rec.source_type, if rec.conflicts_with.is_empty() { String::new() } else { format!(", in conflict with {}", rec.conflicts_with.join(", ")) }));
        Ok((rec, true))
    }

    /// A person says this memory is right.
    pub fn verify(&self, who: &Who, id: &str, now: i64) -> Result<Record> {
        person_only(who, "verify a memory")?;
        let mut r = self.get(id)?.ok_or_else(|| Error(format!("no memory {id}")))?;
        if r.status == Status::Archived {
            return Err(Error("that memory is archived".into()));
        }
        r.status = Status::Verified;
        r.verified_ms = Some(now);
        r.verified_by = Some(who.name.clone());
        self.save(&r)?;
        self.audit_row(now, "verify", Some(id), &who.name, "");
        Ok(r)
    }

    /// A person replaces `old_id` with a new, verified memory. The old one is archived and points to it; the conflict is settled.
    pub fn supersede(&self, who: &Who, old_id: &str, mut new: New, now: i64) -> Result<Record> {
        person_only(who, "supersede a memory")?;
        let mut old = self.get(old_id)?.ok_or_else(|| Error(format!("no memory {old_id}")))?;
        new.verify = true;
        if new.key.is_none() {
            new.key = old.key.clone();
        }
        if new.show.is_empty() {
            new.show = old.show.clone();
        }
        let (mut rec, _) = self.remember(who, new, now)?;
        rec.supersedes = Some(old.id.clone());
        rec.conflicts_with.retain(|c| c != old_id);
        self.save(&rec)?;
        old.status = Status::Archived;
        old.superseded_by = Some(rec.id.clone());
        self.save(&old)?;
        self.audit_row(now, "supersede", Some(old_id), &who.name, &format!("replaced by {}", rec.id));
        Ok(rec)
    }

    /// A person says two memories that disagree are not equal: `keep` is right and `drop` is wrong. `drop` is archived and points to `keep`, and the conflict between them is cleared.
    pub fn settle(&self, who: &Who, keep_id: &str, drop_id: &str, now: i64) -> Result<Record> {
        person_only(who, "settle a conflict between memories")?;
        if keep_id == drop_id {
            return Err(Error("a memory cannot be settled against itself".into()));
        }
        let mut keep = self.get(keep_id)?.ok_or_else(|| Error(format!("no memory {keep_id}")))?;
        let mut drop = self.get(drop_id)?.ok_or_else(|| Error(format!("no memory {drop_id}")))?;
        if keep.status == Status::Archived {
            return Err(Error("the memory to keep is archived".into()));
        }
        keep.conflicts_with.retain(|c| c != drop_id);
        keep.status = Status::Verified;
        keep.verified_ms = Some(now);
        keep.verified_by = Some(who.name.clone());
        keep.supersedes = Some(drop_id.to_string());
        drop.status = Status::Archived;
        drop.superseded_by = Some(keep_id.to_string());
        drop.conflicts_with.clear();
        self.save(&keep)?;
        self.save(&drop)?;
        self.audit_row(now, "settle", Some(keep_id), &who.name, &format!("kept over {drop_id}"));
        Ok(keep)
    }

    pub fn archive(&self, who: &Who, id: &str, now: i64) -> Result<()> {
        person_only(who, "archive a memory")?;
        let mut r = self.get(id)?.ok_or_else(|| Error(format!("no memory {id}")))?;
        r.status = Status::Archived;
        self.save(&r)?;
        self.audit_row(now, "archive", Some(id), &who.name, "");
        Ok(())
    }

    /// Keep a memory from fading (a convention that must outlive the curve).
    pub fn pin(&self, who: &Who, id: &str, pinned: bool, now: i64) -> Result<()> {
        person_only(who, "pin a memory")?;
        let mut r = self.get(id)?.ok_or_else(|| Error(format!("no memory {id}")))?;
        r.pinned = pinned;
        self.save(&r)?;
        self.audit_row(now, if pinned { "pin" } else { "unpin" }, Some(id), &who.name, "");
        Ok(())
    }

    /// Delete a memory for good. Fails closed: it needs a person AND their explicit request.
    pub fn forget(&self, who: &Who, id: &str, user_requested: bool, now: i64) -> Result<()> {
        person_only(who, "delete a memory")?;
        if !user_requested {
            return Err(Error("not deleted: deleting is permanent and needs the person's explicit request (user_requested)".into()));
        }
        let n = self.conn.lock().unwrap().execute("DELETE FROM memory WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error(format!("no memory {id}")));
        }
        self.audit_row(now, "forget", Some(id), &who.name, "deleted at the person's request");
        Ok(())
    }

    /// Archive what has faded below `EXPIRES_BELOW` of its standing (pinned ones never fade). Returns the ids.
    pub fn sweep(&self, now: i64) -> Result<Vec<String>> {
        let mut gone = Vec::new();
        for mut r in self.list(&Filter::default())? {
            if r.status != Status::Archived && decay(&r, now) < EXPIRES_BELOW {
                r.status = Status::Archived;
                self.save(&r)?;
                self.audit_row(now, "expire", Some(&r.id), "memory", &format!("faded: a {} memory has a half-life of {:.0} h", r.kind.name(), r.kind.half_life_hours()));
                gone.push(r.id);
            }
        }
        Ok(gone)
    }

    /// Answer a question from memory, or say why not.
    pub fn recall(&self, query: &str, opts: &RecallOpts, now: i64) -> Result<Recall> {
        let pool: Vec<Record> = self.list(&Filter::default())?.into_iter().filter(|r| r.status != Status::Archived && opts.show.as_ref().map_or(true, |s| *s == r.show || r.show.is_empty()) && opts.kind.map_or(true, |k| k == r.kind)).collect();
        let none = |why: &str, closest: Vec<Hit>| Recall { hits: vec![], abstained: Some(why.to_string()), closest };
        if pool.is_empty() {
            return Ok(none("nothing is remembered yet", vec![]));
        }
        let docs: Vec<search::Doc> = pool.iter().map(|r| search::Doc::new(&format!("{} {} {} {}", r.text, r.key.clone().unwrap_or_default(), r.kind.name(), r.dept))).collect();
        let (_, _, hybrid) = search::rankings(&docs, query);
        let q = search::tokens(query);
        let mut hits: Vec<Hit> = hybrid
            .into_iter()
            .take(opts.limit.max(1) * 3 + 5)
            .map(|i| {
                let r = &pool[i];
                let alignment = search::coverage_idf(&docs, i, &q);
                let (weight, dec) = (r.status.weight(), decay(r, now));
                let reinforcement = (0.02 * (1.0 + r.recalls as f64).ln()).min(0.1);
                Hit { record: r.clone(), activation: alignment * weight * dec + if alignment >= MIN_ALIGNMENT { reinforcement * weight } else { 0.0 }, alignment, weight, decay: dec, reinforcement, trust: if r.status == Status::Verified { "verified" } else { "draft" } }
            })
            .collect();
        hits.sort_by(|a, b| b.activation.partial_cmp(&a.activation).unwrap_or(std::cmp::Ordering::Equal));
        let best_alignment = hits.iter().map(|h| h.alignment).fold(0.0, f64::max);
        let closest: Vec<Hit> = hits.iter().take(3).cloned().collect();
        if hits.is_empty() || best_alignment < MIN_ALIGNMENT {
            return Ok(none("nothing remembered is about that", closest));
        }
        hits.retain(|h| h.alignment >= MIN_ALIGNMENT);
        if hits[0].activation < MIN_ACTIVATION {
            return Ok(none("what is remembered about that is too old or too uncertain to lean on", closest));
        }
        if hits.len() > 1 {
            let (a, b) = (&hits[0], &hits[1]);
            let conflict = a.record.conflicts_with.contains(&b.record.id) || b.record.conflicts_with.contains(&a.record.id);
            if conflict && (a.activation - b.activation).abs() < AMBIGUOUS_MARGIN {
                return Ok(none(&format!("two memories disagree ({} and {}) and are too close to choose: a person must settle it (supersede one)", a.record.id, b.record.id), closest));
            }
        }
        hits.truncate(opts.limit.max(1));
        if opts.reinforce {
            for h in hits.iter_mut() {
                h.record.recalls += 1;
                h.record.last_recalled_ms = Some(now);
                self.save(&h.record)?;
            }
        }
        self.audit_row(now, "recall", None, "memory", &format!("{:?} -> {} hit(s)", query.chars().take(80).collect::<String>(), hits.len()));
        Ok(Recall { hits, abstained: None, closest: vec![] })
    }

    /// The audit trail, newest last.
    pub fn audit(&self, limit: usize) -> Result<Vec<Value>> {
        let c = self.conn.lock().unwrap();
        let mut q = c.prepare("SELECT seq, ts, action, mem_id, by_name, detail FROM audit ORDER BY seq DESC LIMIT ?1")?;
        let rows = q.query_map(params![limit as i64], |r| Ok(json!({"seq": r.get::<_, i64>(0)?, "ts": r.get::<_, i64>(1)?, "action": r.get::<_, String>(2)?, "id": r.get::<_, Option<String>>(3)?, "by": r.get::<_, String>(4)?, "detail": r.get::<_, String>(5)?})))?;
        let mut v: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
        v.reverse();
        Ok(v)
    }

    pub fn stats(&self) -> Result<Value> {
        let all = self.list(&Filter::default())?;
        let n = |s: Status| all.iter().filter(|r| r.status == s).count();
        Ok(json!({"total": all.len(), "verified": n(Status::Verified), "draft": n(Status::Draft), "archived": n(Status::Archived), "in_conflict": all.iter().filter(|r| r.status != Status::Archived && !r.conflicts_with.is_empty()).count()}))
    }

    /// Every memory and the audit trail, one JSON object a line.
    pub fn export_jsonl(&self) -> Result<String> {
        let mut out = String::new();
        for r in self.list(&Filter::default())? {
            out += &format!("{}\n", json!({"memory": r}));
        }
        for a in self.audit(100_000)? {
            out += &format!("{}\n", json!({"audit": a}));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;
    const T0: i64 = 1_800_000_000_000;

    fn nova() -> Who {
        Who::agent("nova")
    }
    fn alex() -> Who {
        Who::person("Alex")
    }

    fn with(kind: Kind, text: &str, key: Option<&str>) -> New {
        New { kind, text: text.into(), key: key.map(String::from), show: "gala".into(), ..Default::default() }
    }

    #[test]
    fn what_an_agent_writes_is_a_draft_and_only_a_person_can_raise_it() {
        let m = Memory::in_memory().unwrap();
        let mut n = with(Kind::Convention, "Lighting wants a 2 second pre-wait on every look.", None);
        n.verify = true; // an agent asking for verified is ignored
        let (r, fresh) = m.remember(&nova(), n.clone(), T0).unwrap();
        assert!(fresh && r.status == Status::Draft && r.author == "nova" && r.source_type == "agent");
        assert!(m.verify(&nova(), &r.id, T0).unwrap_err().0.contains("only a person"));
        assert!(m.supersede(&nova(), &r.id, n.clone(), T0).is_err() && m.pin(&nova(), &r.id, true, T0).is_err() && m.archive(&nova(), &r.id, T0).is_err());
        let v = m.verify(&alex(), &r.id, T0 + 1).unwrap();
        assert_eq!((v.status, v.verified_by.as_deref()), (Status::Verified, Some("Alex")));
        // writing the same thing again changes nothing
        let (again, fresh) = m.remember(&nova(), n, T0 + 2).unwrap();
        assert!(!fresh && again.status == Status::Verified);
        // a person may write it verified at once
        let (p, _) = m.remember(&alex(), New { verify: true, ..with(Kind::Venue, "The hall's Wi-Fi drops every night at ten.", None) }, T0).unwrap();
        assert_eq!(p.status, Status::Verified);
    }

    #[test]
    fn each_kind_fades_at_its_own_rate_and_pinned_ones_do_not() {
        let m = Memory::in_memory().unwrap();
        let (task, _) = m.remember(&alex(), with(Kind::Task, "Re-patch the haze machine before soundcheck.", None), T0).unwrap();
        let (conv, _) = m.remember(&alex(), with(Kind::Convention, "Keep laser cues on their own lane.", None), T0).unwrap();
        assert!((decay(&task, T0 + 23 * 3_600_000) - 0.5).abs() < 1e-9, "a task halves in 23 hours");
        assert!((decay(&conv, T0 + 35 * DAY) - 0.5).abs() < 1e-9, "a convention halves in 35 days");
        assert!(decay(&task, T0) > decay(&task, T0 + DAY));
        // a week on, the task has faded past the line and is swept; the convention stays
        let gone = m.sweep(T0 + 7 * DAY).unwrap();
        assert_eq!(gone, vec![task.id.clone()]);
        assert_eq!(m.get(&conv.id).unwrap().unwrap().status, Status::Draft);
        m.pin(&alex(), &conv.id, true, T0).unwrap();
        assert!(m.sweep(T0 + 3650 * DAY).unwrap().is_empty(), "a pinned memory never fades");
    }

    #[test]
    fn recall_finds_what_is_about_the_question_and_says_when_nothing_is() {
        let m = Memory::in_memory().unwrap();
        assert_eq!(m.recall("anything", &RecallOpts::default(), T0).unwrap().abstained.as_deref(), Some("nothing is remembered yet"));
        m.remember(&alex(), New { verify: true, ..with(Kind::Incident, "The haze cue in scene two fired 400 ms late because the desk was busy.", None) }, T0).unwrap();
        m.remember(&alex(), New { verify: true, ..with(Kind::Venue, "The loading dock door sticks in the cold.", None) }, T0).unwrap();
        let r = m.recall("what went wrong with the haze cue last time?", &RecallOpts::default(), T0 + DAY).unwrap();
        assert!(r.abstained.is_none() && r.hits[0].record.text.contains("haze"), "{:?}", r.to_json());
        assert_eq!(r.hits[0].trust, "verified");
        // a question the memory is not about is not answered with its nearest guess
        let r = m.recall("which projector does the finale use", &RecallOpts::default(), T0 + DAY).unwrap();
        assert_eq!(r.abstained.as_deref(), Some("nothing remembered is about that"), "{:?}", r.to_json());
        assert!(r.hits.is_empty());
    }

    #[test]
    fn a_verified_memory_outranks_a_draft_and_an_old_one_is_too_faint_to_use() {
        let m = Memory::in_memory().unwrap();
        m.remember(&nova(), with(Kind::Preference, "Sam prefers the stage left monitor mix quiet.", None), T0).unwrap();
        let r = m.recall("monitor mix", &RecallOpts::default(), T0).unwrap();
        assert_eq!(r.hits[0].trust, "draft");
        let draft_score = r.hits[0].activation;
        m.remember(&alex(), New { verify: true, ..with(Kind::Preference, "Sam wants the monitor mix loud for the finale.", None) }, T0).unwrap();
        let r = m.recall("monitor mix", &RecallOpts::default(), T0).unwrap();
        assert!(r.hits[0].record.text.contains("loud") && r.hits[0].activation > draft_score);
        // months later a preference (4-day half-life) is too faint to lean on, even though it matches
        let r = m.recall("monitor mix", &RecallOpts::default(), T0 + 60 * DAY).unwrap();
        assert!(r.abstained.as_deref().is_some_and(|a| a.contains("too old")), "{:?}", r.to_json());
    }

    #[test]
    fn two_memories_that_disagree_are_both_kept_and_recall_will_not_choose() {
        let m = Memory::in_memory().unwrap();
        let (a, _) = m.remember(&alex(), New { verify: true, ..with(Kind::Venue, "The house desk is on 10.0.0.20.", Some("house-desk-ip")) }, T0).unwrap();
        let (b, _) = m.remember(&alex(), New { verify: true, ..with(Kind::Venue, "The house desk is on 10.0.0.45.", Some("house-desk-ip")) }, T0).unwrap();
        assert_eq!(m.get(&a.id).unwrap().unwrap().conflicts_with, vec![b.id.clone()]);
        assert_eq!(b.conflicts_with, vec![a.id.clone()]);
        let r = m.recall("what is the house desk address", &RecallOpts::default(), T0).unwrap();
        assert!(r.abstained.as_deref().is_some_and(|x| x.contains("disagree")), "{:?}", r.to_json());
        assert_eq!(r.closest.len(), 2);
        // an agent cannot settle it; a person says which is right: the other is archived with its lineage, and recall answers
        assert!(m.settle(&nova(), &b.id, &a.id, T0).is_err());
        let kept = m.settle(&alex(), &b.id, &a.id, T0 + 1).unwrap();
        assert_eq!(kept.supersedes.as_deref(), Some(a.id.as_str()));
        assert!(kept.conflicts_with.is_empty());
        let old = m.get(&a.id).unwrap().unwrap();
        assert_eq!((old.status, old.superseded_by.as_deref()), (Status::Archived, Some(b.id.as_str())));
        let r = m.recall("what is the house desk address", &RecallOpts::default(), T0 + 2).unwrap();
        assert!(r.abstained.is_none() && r.hits.len() == 1 && r.hits[0].record.text.contains("10.0.0.45"), "{:?}", r.to_json());
        // writing the dead one again is refused: it was superseded
        assert!(m.remember(&nova(), with(Kind::Venue, "The house desk is on 10.0.0.20.", Some("house-desk-ip")), T0 + 3).unwrap_err().0.contains("superseded"));
        // replacing one memory with new words is `supersede`
        let c = m.supersede(&alex(), &b.id, with(Kind::Venue, "The house desk moved to 10.0.0.45 in March.", Some("house-desk-ip")), T0 + 4).unwrap();
        assert_eq!(m.get(&b.id).unwrap().unwrap().superseded_by.as_deref(), Some(c.id.as_str()));
    }

    #[test]
    fn being_recalled_often_does_not_make_a_memory_more_true() {
        let m = Memory::in_memory().unwrap();
        let (r, _) = m.remember(&nova(), with(Kind::Convention, "Lasers are armed last, by the LSO.", None), T0).unwrap();
        let first = m.recall("who arms the lasers", &RecallOpts::default(), T0).unwrap().hits[0].activation;
        for _ in 0..200 {
            m.recall("who arms the lasers", &RecallOpts::default(), T0).unwrap();
        }
        let got = m.get(&r.id).unwrap().unwrap();
        assert_eq!((got.status, got.recalls), (Status::Draft, 201));
        let last = m.recall("who arms the lasers", &RecallOpts { reinforce: false, ..Default::default() }, T0).unwrap().hits[0].clone();
        assert!(last.activation - first <= 0.1 * 0.55 + 1e-9, "the bonus is bounded: {first} -> {}", last.activation);
        assert_eq!(last.trust, "draft");
    }

    #[test]
    fn deleting_fails_closed_and_everything_is_audited() {
        let m = Memory::in_memory().unwrap();
        let (r, _) = m.remember(&nova(), with(Kind::Task, "Check the fog fluid.", None), T0).unwrap();
        assert!(m.forget(&nova(), &r.id, true, T0).is_err());
        assert!(m.forget(&alex(), &r.id, false, T0).unwrap_err().0.contains("explicit request"));
        assert!(m.get(&r.id).unwrap().is_some());
        m.forget(&alex(), &r.id, true, T0 + 1).unwrap();
        assert!(m.get(&r.id).unwrap().is_none());
        let actions: Vec<String> = m.audit(50).unwrap().iter().map(|a| a["action"].as_str().unwrap().to_string()).collect();
        assert_eq!(actions, vec!["remember", "forget"]);
        assert!(m.export_jsonl().unwrap().contains("\"audit\""));
    }
}
