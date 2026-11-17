//! A show library kept on the server (`--library DIR`): people save shows there, plan them (a date, a venue, a status, notes), and open
//! them from any machine that can reach the runtime.
//!
//! One folder per show: `meta.json` (who owns it, who may see it, when it is planned for) and one file per saved version (`v0001.pchi`,
//! ...). A save never overwrites a version: it adds the next one, so an earlier one can always be opened. Writes go to a temporary file
//! and are renamed into place, and `meta.json` is written last, so a crash leaves the library on the last complete save.
//!
//! This module knows nothing about people or roles: the HTTP layer decides who may do what. It only keeps files tidy and refuses a save
//! made on top of an out-of-date version (two people editing the same show).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How many versions of one show are kept (the oldest are removed when a newer one is saved).
pub const KEEP_VERSIONS: u32 = 50;
/// The biggest show file the library takes.
pub const MAX_SHOW_BYTES: usize = 8 * 1024 * 1024;

pub const STATUSES: [&str; 4] = ["draft", "planned", "ready", "done"];
pub const VISIBILITY: [&str; 2] = ["private", "team"];

#[derive(Debug)]
pub enum LibErr {
    NotFound,
    /// Someone saved a newer version first: what is there now (`version`, `updated_by`, `updated`).
    Conflict(Value),
    Bad(String),
    Io(String),
}

pub struct Library {
    dir: PathBuf,
    /// One write at a time: a save reads the version, checks it and writes the next.
    lock: Mutex<()>,
}

/// An id is lower-case letters, digits and `-` only, so it can never reach outside the library folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && !id.starts_with('-') && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s: String = out.trim_matches('-').chars().take(40).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "show".into()
    } else {
        s
    }
}

fn clean_line(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

fn clean_notes(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(2000).collect::<String>().trim().to_string()
}

/// A planned date or date-and-time: digits, `-`, `:` and `T` only, such as `2026-12-05` or `2026-12-05T19:30` (or empty).
fn valid_when(s: &str) -> bool {
    s.is_empty() || (s.len() >= 10 && s.len() <= 16 && s.as_bytes()[0].is_ascii_digit() && s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'T')))
}

/// Apply the fields a person may set. Only the ones present change; anything else in `fields` is ignored.
fn apply_fields(meta: &mut Value, fields: &Value) -> Result<(), LibErr> {
    if let Some(v) = fields.get("name") {
        let n = clean_line(v.as_str().unwrap_or(""), 80);
        if n.is_empty() {
            return Err(LibErr::Bad("a show needs a name".into()));
        }
        meta["name"] = json!(n);
    }
    if let Some(v) = fields.get("visibility") {
        let s = v.as_str().unwrap_or("");
        if !VISIBILITY.contains(&s) {
            return Err(LibErr::Bad(format!("visibility is one of {}", VISIBILITY.join(", "))));
        }
        meta["visibility"] = json!(s);
    }
    if let Some(v) = fields.get("status") {
        let s = v.as_str().unwrap_or("");
        if !STATUSES.contains(&s) {
            return Err(LibErr::Bad(format!("status is one of {}", STATUSES.join(", "))));
        }
        meta["status"] = json!(s);
    }
    if let Some(v) = fields.get("planned_for") {
        let s = v.as_str().unwrap_or("").trim();
        if !valid_when(s) {
            return Err(LibErr::Bad("planned_for is a date such as 2026-12-05, or 2026-12-05T19:30".into()));
        }
        meta["planned_for"] = json!(s);
    }
    if let Some(v) = fields.get("venue") {
        meta["venue"] = json!(clean_line(v.as_str().unwrap_or(""), 100));
    }
    if let Some(v) = fields.get("notes") {
        meta["notes"] = json!(clean_notes(v.as_str().unwrap_or("")));
    }
    Ok(())
}

/// How many scenes a show file has, for the list. A file that does not say gives 0.
fn scene_count(text: &str) -> usize {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v["showchart"]["states"].as_array().map(Vec::len)).unwrap_or(0)
}

/// A show file is kept on a shared server, so a device password written into it (an OBS node's `password`) is not kept: it is taken out and the
/// devices it was taken from are returned, for the person to be told. Keep such a secret on the machine that talks to the device (`PCHI_OBS_PASSWORD`).
pub fn strip_secrets(text: &str) -> (String, Vec<String>) {
    let Ok(mut v) = serde_json::from_str::<Value>(text) else { return (text.to_string(), Vec::new()) };
    let mut gone = Vec::new();
    if let Some(nodes) = v.get_mut("nodes").and_then(Value::as_array_mut) {
        for n in nodes {
            let id = n["id"].as_str().unwrap_or("?").to_string();
            if let Some(p) = n.get_mut("params").and_then(Value::as_object_mut) {
                if p.get("password").is_some_and(|x| x.as_str().map_or(true, |s| !s.is_empty())) {
                    p.remove("password");
                    gone.push(id);
                }
            }
        }
    }
    if gone.is_empty() {
        (text.to_string(), gone)
    } else {
        (serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()), gone)
    }
}

fn write_atomic(path: &Path, body: &str) -> Result<(), LibErr> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| LibErr::Io(format!("{}: {e}", path.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| LibErr::Io(format!("{}: {e}", path.display())))
}

impl Library {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| format!("library folder {}: {e}", dir.display()))?;
        Ok(Self { dir, lock: Mutex::new(()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn show_dir(&self, id: &str) -> Result<PathBuf, LibErr> {
        if !valid_id(id) {
            return Err(LibErr::NotFound);
        }
        let p = self.dir.join(id);
        if p.is_dir() {
            Ok(p)
        } else {
            Err(LibErr::NotFound)
        }
    }

    fn read_meta(&self, id: &str) -> Result<Value, LibErr> {
        let p = self.show_dir(id)?.join("meta.json");
        let text = std::fs::read_to_string(&p).map_err(|_| LibErr::NotFound)?;
        serde_json::from_str(&text).map_err(|e| LibErr::Io(format!("{}: {e}", p.display())))
    }

    fn version_path(dir: &Path, v: u32) -> PathBuf {
        dir.join(format!("v{v:04}.pchi"))
    }

    /// Every show's meta, newest change first. A folder that does not read is left out, not fatal.
    pub fn list(&self) -> Vec<Value> {
        let mut out: Vec<Value> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if !valid_id(&name) {
                    return None;
                }
                self.read_meta(&name).ok()
            })
            .collect();
        out.sort_by_key(|m| std::cmp::Reverse(m["updated"].as_i64().unwrap_or(0)));
        out
    }

    pub fn meta(&self, id: &str) -> Result<Value, LibErr> {
        self.read_meta(id)
    }

    /// The meta and the text of one version (the latest when `version` is None).
    pub fn get(&self, id: &str, version: Option<u32>) -> Result<(Value, String), LibErr> {
        let meta = self.read_meta(id)?;
        let latest = meta["version"].as_u64().unwrap_or(1) as u32;
        let v = version.unwrap_or(latest);
        let text = std::fs::read_to_string(Self::version_path(&self.show_dir(id)?, v)).map_err(|_| LibErr::NotFound)?;
        Ok((meta, text))
    }

    /// The versions that can still be opened, oldest first, each with who saved it and when (read from the file times: the meta only holds the latest).
    pub fn versions(&self, id: &str) -> Result<Vec<u32>, LibErr> {
        let dir = self.show_dir(id)?;
        let mut v: Vec<u32> = std::fs::read_dir(&dir)
            .map_err(|e| LibErr::Io(e.to_string()))?
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().strip_prefix('v').and_then(|s| s.strip_suffix(".pchi")).and_then(|s| s.parse().ok()))
            .collect();
        v.sort_unstable();
        Ok(v)
    }

    /// A new show: version 1. `fields` carries name and (optionally) planned_for, venue, status, notes, visibility. New shows are private unless said.
    pub fn create(&self, owner: &str, now: i64, fields: &Value, text: &str) -> Result<Value, LibErr> {
        if text.len() > MAX_SHOW_BYTES {
            return Err(LibErr::Bad(format!("a show may be at most {} MB", MAX_SHOW_BYTES / 1024 / 1024)));
        }
        let _g = self.lock.lock().unwrap();
        let mut meta = json!({"name": "", "owner": owner, "visibility": "private", "status": "draft", "planned_for": "", "venue": "", "notes": "",
            "created": now, "updated": now, "updated_by": owner, "version": 1, "scenes": scene_count(text), "bytes": text.len()});
        apply_fields(&mut meta, fields)?;
        if meta["name"].as_str().unwrap_or("").is_empty() {
            return Err(LibErr::Bad("a show needs a name".into()));
        }
        use rand::RngCore;
        let mut r = [0u8; 3];
        let mut dir = None;
        for _ in 0..8 {
            rand::rngs::OsRng.fill_bytes(&mut r);
            let id = format!("{}-{}", slug(meta["name"].as_str().unwrap_or("")), hex::encode(r));
            if !self.dir.join(&id).exists() {
                dir = Some(id);
                break;
            }
        }
        let id = dir.ok_or_else(|| LibErr::Io("could not make a free id".into()))?;
        let path = self.dir.join(&id);
        std::fs::create_dir(&path).map_err(|e| LibErr::Io(format!("{}: {e}", path.display())))?;
        let made = write_atomic(&Self::version_path(&path, 1), text).and_then(|_| {
            meta["id"] = json!(id);
            write_atomic(&path.join("meta.json"), &serde_json::to_string_pretty(&meta).unwrap_or_default())
        });
        if let Err(e) = made {
            let _ = std::fs::remove_dir_all(&path); // do not leave half a show behind
            return Err(e);
        }
        Ok(meta)
    }

    /// Change a show. With `text`, that is a new version, and `base_version` (the version the person opened) must still be the latest: if someone
    /// saved since, nothing is written and `Conflict` says who. Without `text`, only the details (name, date, status, notes, visibility) change.
    pub fn save(&self, id: &str, by: &str, now: i64, fields: &Value, text: Option<&str>, base_version: Option<u32>) -> Result<Value, LibErr> {
        let _g = self.lock.lock().unwrap();
        let dir = self.show_dir(id)?;
        let mut meta = self.read_meta(id)?;
        let current = meta["version"].as_u64().unwrap_or(1) as u32;
        apply_fields(&mut meta, fields)?;
        if let Some(t) = text {
            if t.len() > MAX_SHOW_BYTES {
                return Err(LibErr::Bad(format!("a show may be at most {} MB", MAX_SHOW_BYTES / 1024 / 1024)));
            }
            let Some(base) = base_version else {
                return Err(LibErr::Bad("base_version is required with a new version: the version you opened".into()));
            };
            if base != current {
                return Err(LibErr::Conflict(json!({"version": current, "updated_by": meta["updated_by"], "updated": meta["updated"]})));
            }
            let next = current + 1;
            write_atomic(&Self::version_path(&dir, next), t)?;
            meta["version"] = json!(next);
            meta["scenes"] = json!(scene_count(t));
            meta["bytes"] = json!(t.len());
            if next > KEEP_VERSIONS {
                let _ = std::fs::remove_file(Self::version_path(&dir, next - KEEP_VERSIONS));
            }
        }
        meta["updated"] = json!(now);
        meta["updated_by"] = json!(by);
        write_atomic(&dir.join("meta.json"), &serde_json::to_string_pretty(&meta).unwrap_or_default())?;
        Ok(meta)
    }

    /// A private copy of the latest version for `owner` (to build on a show someone else shared).
    pub fn copy(&self, id: &str, owner: &str, now: i64, name: Option<&str>) -> Result<Value, LibErr> {
        let (meta, text) = self.get(id, None)?;
        let name = name.map(str::to_string).unwrap_or_else(|| format!("{} (copy)", meta["name"].as_str().unwrap_or("show")));
        self.create(owner, now, &json!({"name": name, "venue": meta["venue"], "notes": meta["notes"], "planned_for": meta["planned_for"]}), &text)
    }

    /// Take a show out of the library. It is moved to `_archive`, not deleted, so it can be put back by hand.
    pub fn archive(&self, id: &str, now: i64) -> Result<(), LibErr> {
        let _g = self.lock.lock().unwrap();
        let dir = self.show_dir(id)?;
        let arch = self.dir.join("_archive");
        std::fs::create_dir_all(&arch).map_err(|e| LibErr::Io(e.to_string()))?;
        std::fs::rename(&dir, arch.join(format!("{id}-{now}"))).map_err(|e| LibErr::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("pchi-lib-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    const SHOW: &str = r#"{"showchart":{"states":[{"id":"A"},{"id":"B"}]}}"#;

    #[test]
    fn create_list_get_and_versions() {
        let dir = temp("a");
        let lib = Library::open(&dir).unwrap();
        let m = lib.create("Alex", 100, &json!({"name": "Spring Gala!", "planned_for": "2026-12-05", "venue": "Hall 3"}), SHOW).unwrap();
        let id = m["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("spring-gala-") && valid_id(&id), "{id}");
        assert_eq!(m["owner"], "Alex");
        assert_eq!(m["scenes"], 2);
        assert_eq!(m["visibility"], "private");
        let m2 = lib.save(&id, "Eddie", 200, &json!({"status": "planned"}), Some(SHOW), Some(1)).unwrap();
        assert_eq!(m2["version"], 2);
        assert_eq!(m2["updated_by"], "Eddie");
        assert_eq!(m2["owner"], "Alex", "saving does not change the owner");
        assert_eq!(lib.versions(&id).unwrap(), vec![1, 2]);
        assert_eq!(lib.get(&id, Some(1)).unwrap().1, SHOW);
        assert_eq!(lib.list().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_save_is_refused_and_says_who() {
        let dir = temp("b");
        let lib = Library::open(&dir).unwrap();
        let id = lib.create("Alex", 1, &json!({"name": "S"}), SHOW).unwrap()["id"].as_str().unwrap().to_string();
        lib.save(&id, "Alex", 2, &json!({}), Some(SHOW), Some(1)).unwrap(); // now at v2
        match lib.save(&id, "Eddie", 3, &json!({}), Some("{}"), Some(1)) {
            Err(LibErr::Conflict(v)) => {
                assert_eq!(v["version"], 2);
                assert_eq!(v["updated_by"], "Alex");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(lib.get(&id, None).unwrap().1, SHOW, "nothing was overwritten");
        assert!(matches!(lib.save(&id, "Eddie", 3, &json!({}), Some("{}"), None), Err(LibErr::Bad(_))), "a save must say which version it was built on");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn passwords_are_not_kept() {
        let t = r#"{"nodes":[{"id":"obs-1","type":"obs","params":{"host":"h","password":"hunter2"}},{"id":"x","type":"osc","params":{"password":""}},{"id":"y","type":"osc"}],"showchart":{}}"#;
        let (out, gone) = strip_secrets(t);
        assert_eq!(gone, vec!["obs-1"]);
        assert!(!out.contains("hunter2") && out.contains("\"host\""), "{out}");
        let (same, none) = strip_secrets(SHOW);
        assert!(none.is_empty() && same == SHOW, "a show with no password is kept byte for byte");
    }

    #[test]
    fn ids_cannot_reach_outside_the_library() {
        let dir = temp("c");
        let lib = Library::open(&dir).unwrap();
        for bad in ["..", "../x", "a/b", "A", "", "_archive", "-x"] {
            assert!(matches!(lib.get(bad, None), Err(LibErr::NotFound)), "{bad}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn details_are_checked() {
        let dir = temp("d");
        let lib = Library::open(&dir).unwrap();
        assert!(matches!(lib.create("A", 1, &json!({"name": "  "}), SHOW), Err(LibErr::Bad(_))));
        assert!(matches!(lib.create("A", 1, &json!({"name": "x", "planned_for": "tomorrow"}), SHOW), Err(LibErr::Bad(_))));
        assert!(matches!(lib.create("A", 1, &json!({"name": "x", "status": "party"}), SHOW), Err(LibErr::Bad(_))));
        assert!(matches!(lib.create("A", 1, &json!({"name": "x", "visibility": "public"}), SHOW), Err(LibErr::Bad(_))));
        assert!(lib.list().is_empty(), "a refused show leaves nothing behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_versions_are_pruned_and_archive_moves_not_deletes() {
        let dir = temp("e");
        let lib = Library::open(&dir).unwrap();
        let id = lib.create("A", 1, &json!({"name": "S"}), SHOW).unwrap()["id"].as_str().unwrap().to_string();
        for v in 1..=KEEP_VERSIONS + 3 {
            lib.save(&id, "A", 2, &json!({}), Some(SHOW), Some(v)).unwrap();
        }
        let vs = lib.versions(&id).unwrap();
        assert_eq!(vs.len() as u32, KEEP_VERSIONS);
        assert_eq!(*vs.last().unwrap(), KEEP_VERSIONS + 4);
        lib.archive(&id, 9).unwrap();
        assert!(lib.list().is_empty());
        assert!(dir.join("_archive").read_dir().unwrap().count() == 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn copy_is_private_and_owned_by_the_copier() {
        let dir = temp("f");
        let lib = Library::open(&dir).unwrap();
        let id = lib.create("Alex", 1, &json!({"name": "Gala", "visibility": "team"}), SHOW).unwrap()["id"].as_str().unwrap().to_string();
        let c = lib.copy(&id, "Eddie", 2, None).unwrap();
        assert_eq!(c["owner"], "Eddie");
        assert_eq!(c["visibility"], "private");
        assert_eq!(c["name"], "Gala (copy)");
        assert_ne!(c["id"], json!(id));
        let _ = std::fs::remove_dir_all(dir);
    }
}
