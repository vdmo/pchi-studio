use rusqlite::{params, Connection};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    /// Insertion order; the log is append-only so this is the total order.
    pub id: i64,
    /// Milliseconds since the Unix epoch (supplied by the caller).
    pub timestamp: i64,
    pub source: String,
    pub target: Option<String>,
    pub event_type: String,
    pub payload: Value,
}

pub struct EventStore {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp  INTEGER NOT NULL,
    source     TEXT    NOT NULL,
    target     TEXT,
    event_type TEXT    NOT NULL,
    payload    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS events_type ON events(event_type);
";

impl EventStore {
    pub fn open(path: &str) -> Result<Self> {
        let s = Self { conn: Connection::open(path)? };
        s.conn.execute_batch(SCHEMA)?;
        Ok(s)
    }

    pub fn open_in_memory() -> Result<Self> {
        let s = Self { conn: Connection::open_in_memory()? };
        s.conn.execute_batch(SCHEMA)?;
        Ok(s)
    }

    pub fn append(&self, timestamp: i64, source: &str, target: Option<&str>, event_type: &str, payload: &Value) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO events (timestamp, source, target, event_type, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![timestamp, source, target, event_type, serde_json::to_string(payload)?],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Every record, oldest first.
    pub fn all(&self) -> Result<Vec<EventRecord>> {
        let mut q = self.conn.prepare("SELECT id, timestamp, source, target, event_type, payload FROM events ORDER BY id")?;
        let rows = q.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, timestamp, source, target, event_type, payload) = row?;
            out.push(EventRecord { id, timestamp, source, target, event_type, payload: serde_json::from_str(&payload)? });
        }
        Ok(out)
    }

    /// Records after `id` (exclusive), oldest first, at most `limit`: how a standby follows a log.
    pub fn after(&self, id: i64, limit: usize) -> Result<Vec<EventRecord>> {
        let mut q = self.conn.prepare("SELECT id, timestamp, source, target, event_type, payload FROM events WHERE id > ?1 ORDER BY id LIMIT ?2")?;
        let rows = q.query_map(params![id, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, timestamp, source, target, event_type, payload) = row?;
            out.push(EventRecord { id, timestamp, source, target, event_type, payload: serde_json::from_str(&payload)? });
        }
        Ok(out)
    }

    /// The timestamp of record `id`, if there is one (a standby checks it still follows the same log).
    pub fn timestamp_of(&self, id: i64) -> Result<Option<i64>> {
        match self.conn.query_row("SELECT timestamp FROM events WHERE id = ?1", params![id], |r| r.get(0)) {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The id of the newest record, or 0 for an empty log.
    pub fn last_id(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?)
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn append_and_read_back_in_order() {
        let s = EventStore::open_in_memory().unwrap();
        let a = s.append(10, "cli", None, "show.input", &json!({"event": "START"})).unwrap();
        let b = s.append(20, "show", Some("vork"), "scene.activate", &json!({"scene": "portal"})).unwrap();
        assert!(b > a);
        let all = s.all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].payload, json!({"event": "START"}));
        assert_eq!(all[1].target.as_deref(), Some("vork"));
        assert_eq!(s.count().unwrap(), 2);
    }

    #[test]
    fn file_backed_store_persists_across_opens() {
        let path = std::env::temp_dir().join(format!("pchi-store-{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        EventStore::open(p).unwrap().append(1, "x", None, "t", &json!(1)).unwrap();
        assert_eq!(EventStore::open(p).unwrap().count().unwrap(), 1);
        std::fs::remove_file(p).unwrap();
    }
}
