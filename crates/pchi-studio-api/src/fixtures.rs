//! Fixture profiles and the patch: what a DMX fixture's channels mean, so a cue can say "wash 1 blue at 40%" instead of channel numbers.
//!
//! A **fixture type** is a small JSON file: its modes, and for each mode which channel carries which attribute (dimmer, red,
//! pan, strobe...), with optional 16-bit pairs and named values. A few generic ones are built in (`crates/pchi-studio-api/fixtures/`);
//! more come from a folder given with `--fixtures`. A file with a built-in's `id` replaces it.
//!
//! The **patch** says which fixtures sit where on a DMX device. It is one line of text in the device's `patch` parameter:
//!
//! ```text
//! wash1=rgbw@1, wash2=rgbw@5, spot=rgb-dimmer:4ch@20, walls=wash1+wash2
//! ```
//!
//! Each entry is `name=type[:mode]@address` (a fixture) or `name=fixture+fixture` (a group). Two fixtures may not share a channel.
//!
//! What this does not do: it knows nothing of a fixture's real behaviour. A fixture type is only as good as the manual it was
//! written from, and a wrong file is a wrong cue. Prove each type on the real fixture with `pchi-gearcheck`.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

const BUILT_IN: [&str; 4] = [include_str!("../fixtures/dimmer.json"), include_str!("../fixtures/rgb.json"), include_str!("../fixtures/rgbw.json"), include_str!("../fixtures/rgb-dimmer.json")];

#[derive(Clone, Debug)]
pub struct Attr {
    /// 1-based, within the fixture.
    pub channel: usize,
    /// The channel carrying the low byte of a 16-bit attribute (pan, tilt), if any.
    pub fine: Option<usize>,
    /// Named values: `strobe=fast`. Each is a DMX value 0 to 255.
    pub values: BTreeMap<String, u8>,
}

#[derive(Clone, Debug)]
pub struct Mode {
    pub name: String,
    pub channels: usize,
    pub attrs: BTreeMap<String, Attr>,
}

#[derive(Debug)]
pub struct FixtureType {
    pub id: String,
    pub name: String,
    pub modes: Vec<Mode>,
    pub notes: String,
    pub built_in: bool,
}

#[derive(Deserialize)]
struct RawFixture {
    id: String,
    name: String,
    modes: Vec<RawMode>,
    #[serde(default)]
    notes: String,
}

#[derive(Deserialize)]
struct RawMode {
    name: String,
    channels: usize,
    attributes: BTreeMap<String, RawAttr>,
}

#[derive(Deserialize)]
struct RawAttr {
    channel: usize,
    #[serde(default)]
    fine: Option<usize>,
    #[serde(default)]
    values: BTreeMap<String, u32>,
}

impl FixtureType {
    pub fn from_json(text: &str, built_in: bool) -> Result<FixtureType, String> {
        let raw: RawFixture = serde_json::from_str(text).map_err(|e| format!("not a fixture file: {e}"))?;
        let id = raw.id.trim().to_string();
        if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("id '{id}' must be 1 to 32 characters: a to z, 0 to 9 and -"));
        }
        if raw.modes.is_empty() {
            return Err(format!("{id}: no modes"));
        }
        let mut modes: Vec<Mode> = Vec::new();
        for m in &raw.modes {
            let at = |e: String| format!("{id} mode '{}': {e}", m.name);
            if m.name.trim().is_empty() || m.name.contains([':', '@', ',', ';', ' ']) {
                return Err(format!("{id}: a mode needs a name without spaces or : @ , ;"));
            }
            if modes.iter().any(|x| x.name.eq_ignore_ascii_case(&m.name)) {
                return Err(at("two modes have this name".into()));
            }
            if m.channels == 0 || m.channels > 512 {
                return Err(at(format!("channels must be 1 to 512, not {}", m.channels)));
            }
            let mut used: HashMap<usize, String> = HashMap::new();
            let mut attrs = BTreeMap::new();
            for (name, a) in &m.attributes {
                let key = name.to_ascii_lowercase();
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                    return Err(at(format!("attribute '{name}' must be a to z, 0 to 9 and _")));
                }
                for (label, ch) in std::iter::once(("channel", Some(a.channel))).chain(std::iter::once(("fine", a.fine))) {
                    let Some(ch) = ch else { continue };
                    if ch == 0 || ch > m.channels {
                        return Err(at(format!("{key} {label} {ch} is outside the fixture's {} channels", m.channels)));
                    }
                    if let Some(other) = used.insert(ch, key.clone()) {
                        return Err(at(format!("channel {ch} is used by both {other} and {key}")));
                    }
                }
                let mut values = BTreeMap::new();
                for (vn, v) in &a.values {
                    let v = u8::try_from(*v).map_err(|_| at(format!("{key} value '{vn}' is {v}: DMX values go from 0 to 255")))?;
                    values.insert(vn.to_ascii_lowercase(), v);
                }
                attrs.insert(key, Attr { channel: a.channel, fine: a.fine, values });
            }
            if attrs.is_empty() {
                return Err(at("no attributes".into()));
            }
            modes.push(Mode { name: m.name.clone(), channels: m.channels, attrs });
        }
        Ok(FixtureType { id, name: raw.name, modes, notes: raw.notes, built_in })
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<FixtureType>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<FixtureType>>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut m = HashMap::new();
        for t in BUILT_IN {
            let f = FixtureType::from_json(t, true).expect("a built-in fixture file is valid (a test checks them all)");
            m.insert(f.id.clone(), Arc::new(f));
        }
        RwLock::new(m)
    })
}

pub fn get(id: &str) -> Option<Arc<FixtureType>> {
    registry().read().unwrap().get(&id.to_ascii_lowercase()).cloned()
}

pub fn list() -> Vec<Arc<FixtureType>> {
    let mut v: Vec<_> = registry().read().unwrap().values().cloned().collect();
    v.sort_by(|a, b| b.built_in.cmp(&a.built_in).then_with(|| a.id.cmp(&b.id)));
    v
}

/// Add the fixture files (`*.json`, one type or a list) in `dir`. All or nothing: one bad file names itself and changes nothing.
pub fn load_dir(dir: &Path) -> Result<Vec<String>, String> {
    let mut files: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    let mut loaded = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: not valid JSON: {e}", f.display()))?;
        let items = if let serde_json::Value::Array(a) = v { a } else { vec![v] };
        for item in items {
            loaded.push(FixtureType::from_json(&item.to_string(), false).map_err(|e| format!("{}: {e}", f.display()))?);
        }
    }
    let mut reg = registry().write().unwrap();
    let mut lines = Vec::new();
    for f in loaded {
        let replaced = reg.get(&f.id).map(|o| o.built_in);
        lines.push(format!("{} ({}, {} mode(s)){}", f.name, f.id, f.modes.len(), match replaced { Some(true) => ", replaces the built-in", Some(false) => ", replaces an earlier one", None => "" }));
        reg.insert(f.id.clone(), Arc::new(f));
    }
    Ok(lines)
}

pub fn summary() -> serde_json::Value {
    serde_json::Value::Array(
        list()
            .iter()
            .map(|f| {
                serde_json::json!({"id": f.id, "name": f.name, "built_in": f.built_in, "notes": f.notes,
                    "modes": f.modes.iter().map(|m| serde_json::json!({"name": m.name, "channels": m.channels, "attributes": m.attrs.keys().collect::<Vec<_>>()})).collect::<Vec<_>>()})
            })
            .collect(),
    )
}

/// A fixture placed on a DMX device.
#[derive(Debug)]
pub struct Placed {
    pub name: String,
    pub ftype: Arc<FixtureType>,
    pub mode: usize,
    /// 1-based DMX address of its first channel.
    pub address: usize,
}

#[derive(Default, Debug)]
pub struct Patch {
    pub fixtures: Vec<Placed>,
    pub groups: Vec<(String, Vec<String>)>,
}

fn name_ok(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && n.len() <= 32 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Patch {
    /// Read a device's `patch` text. Errors name the entry and say what to fix.
    pub fn parse(text: &str) -> Result<Patch, String> {
        let mut p = Patch::default();
        let mut taken: Vec<Option<usize>> = vec![None; 513]; // channel -> index of the fixture using it
        let mut group_entries: Vec<(String, Vec<String>)> = Vec::new();
        for entry in text.split(|c| c == ',' || c == ';' || c == '\n').map(str::trim).filter(|e| !e.is_empty()) {
            let (name, rest) = entry.split_once('=').ok_or_else(|| format!("'{entry}': write name=type@address (for example wash1=rgbw@1) or name=a+b for a group"))?;
            let (name, rest) = (name.trim(), rest.trim());
            if !name_ok(name) || name.eq_ignore_ascii_case("blackout") {
                return Err(format!("'{name}' is not a usable fixture name: start with a letter, then letters, digits, - and _"));
            }
            if p.fixtures.iter().any(|f| f.name.eq_ignore_ascii_case(name)) || group_entries.iter().any(|g| g.0.eq_ignore_ascii_case(name)) {
                return Err(format!("'{name}' is patched twice"));
            }
            if let Some((ty, addr)) = rest.split_once('@') {
                let (tid, mode_name) = match ty.split_once(':') {
                    Some((t, m)) => (t.trim(), Some(m.trim())),
                    None => (ty.trim(), None),
                };
                let ft = get(tid).ok_or_else(|| format!("{name}: there is no fixture type '{tid}' (known: {}). Add a file for it with --fixtures", list().iter().map(|f| f.id.clone()).collect::<Vec<_>>().join(", ")))?;
                let mode = match mode_name {
                    None if ft.modes.len() == 1 => 0,
                    None => return Err(format!("{name}: {tid} has several modes ({}): say which, as {tid}:<mode>@{}", ft.modes.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join(", "), addr.trim())),
                    Some(m) => ft.modes.iter().position(|x| x.name.eq_ignore_ascii_case(m)).ok_or_else(|| format!("{name}: {tid} has no mode '{m}' (it has {})", ft.modes.iter().map(|m| m.name.clone()).collect::<Vec<_>>().join(", ")))?,
                };
                let address: usize = addr.trim().parse().map_err(|_| format!("{name}: '{}' is not a DMX address (1 to 512)", addr.trim()))?;
                let n = ft.modes[mode].channels;
                if address == 0 || address + n - 1 > 512 {
                    return Err(format!("{name}: {tid} needs {n} channel(s), so its address must be 1 to {}", 513 - n));
                }
                let idx = p.fixtures.len();
                for ch in address..address + n {
                    if let Some(o) = taken[ch] {
                        let o = &p.fixtures[o];
                        return Err(format!("{name} ({tid} at {address}) overlaps {} ({} at {}): channel {ch} is used twice", o.name, o.ftype.id, o.address));
                    }
                    taken[ch] = Some(idx);
                }
                p.fixtures.push(Placed { name: name.to_string(), ftype: ft, mode, address });
            } else {
                let members: Vec<String> = rest.split('+').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
                if members.is_empty() {
                    return Err(format!("{name}: a group lists fixtures, like {name}=wash1+wash2"));
                }
                group_entries.push((name.to_string(), members));
            }
        }
        for (g, members) in &group_entries {
            for m in members {
                if !p.fixtures.iter().any(|f| f.name.eq_ignore_ascii_case(m)) {
                    return Err(format!("group {g}: '{m}' is not a patched fixture"));
                }
            }
        }
        p.groups = group_entries;
        if p.fixtures.is_empty() {
            return Err("the patch has no fixtures".into());
        }
        Ok(p)
    }

    /// The fixtures a name refers to: one fixture, or a group's members.
    fn members(&self, name: &str) -> Option<Vec<&Placed>> {
        if let Some(f) = self.fixtures.iter().find(|f| f.name.eq_ignore_ascii_case(name)) {
            return Some(vec![f]);
        }
        let (_, ms) = self.groups.iter().find(|g| g.0.eq_ignore_ascii_case(name))?;
        Some(ms.iter().filter_map(|m| self.fixtures.iter().find(|f| f.name.eq_ignore_ascii_case(m))).collect())
    }

    /// `target.attr=value` as channel levels (channel numbers from 0). Without `attr`: a color word means `color`, a level means `dimmer`.
    pub fn resolve(&self, target: &str, attr: Option<&str>, value: &str) -> Result<Vec<(usize, u8)>, String> {
        let fixtures = self.members(target).ok_or_else(|| format!("'{target}' is not a patched fixture or group (patched: {})", self.names().join(", ")))?;
        let v = value.trim();
        let attr_name: String = match attr {
            Some(a) => a.to_ascii_lowercase(),
            None => if is_level(v) { "dimmer".into() } else { "color".into() },
        };
        let mut out = Vec::new();
        for f in fixtures {
            let mode = &f.ftype.modes[f.mode];
            let base = f.address - 1;
            if attr_name == "color" && !mode.attrs.contains_key("color") {
                out.extend(color(f, mode, v)?);
                continue;
            }
            let a = mode.attrs.get(&attr_name).ok_or_else(|| {
                let has: Vec<&str> = mode.attrs.keys().map(String::as_str).collect();
                let hint = if attr_name == "dimmer" { " (set a color instead, such as blue)" } else { "" };
                format!("{} ({}) has no '{attr_name}'{hint}: it has {}", f.name, f.ftype.id, has.join(", "))
            })?;
            if let Some(named) = a.values.get(&v.to_ascii_lowercase()) {
                out.push((base + a.channel - 1, *named));
                if let Some(fine) = a.fine {
                    out.push((base + fine - 1, 0));
                }
                continue;
            }
            let lv = level16(v).map_err(|e| format!("{}.{attr_name}: {e}{}", f.name, if a.values.is_empty() { String::new() } else { format!(" (or one of: {})", a.values.keys().cloned().collect::<Vec<_>>().join(", ")) }))?;
            match a.fine {
                Some(fine) => {
                    out.push((base + a.channel - 1, (lv >> 8) as u8));
                    out.push((base + fine - 1, (lv & 0xFF) as u8));
                }
                None => out.push((base + a.channel - 1, (lv as f64 / 257.0).round() as u8)),
            }
        }
        Ok(out)
    }

    pub fn names(&self) -> Vec<String> {
        self.fixtures.iter().map(|f| f.name.clone()).chain(self.groups.iter().map(|g| g.0.clone())).collect()
    }
}

fn is_level(v: &str) -> bool {
    let v = v.trim().to_ascii_lowercase();
    v.ends_with('%') || v.parse::<f64>().is_ok() || v == "full" || v == "out" || v == "off"
}

/// A level as 16 bits (0 to 65535): "50%", "128" (0 to 255), "full", "out".
fn level16(v: &str) -> Result<u32, String> {
    let s = v.trim();
    if let Some(p) = s.strip_suffix('%') {
        let n: f64 = p.trim().parse().map_err(|_| format!("'{s}' is not a level"))?;
        if !(0.0..=100.0).contains(&n) {
            return Err(format!("'{s}': a percentage goes from 0 to 100"));
        }
        return Ok((n * 65535.0 / 100.0).round() as u32);
    }
    match s.to_ascii_lowercase().as_str() {
        "full" => return Ok(65535),
        "out" | "off" => return Ok(0),
        _ => {}
    }
    let n: u32 = s.parse().map_err(|_| format!("'{s}' is not a level: use a percentage, 0 to 255, full or out"))?;
    if n > 255 {
        return Err(format!("'{s}': a level goes from 0 to 255"));
    }
    Ok(n * 257)
}

/// Named colors as red, green, blue (and white / amber when a fixture has them).
const COLORS: [(&str, [u8; 3]); 14] = [
    ("red", [255, 0, 0]),
    ("green", [0, 255, 0]),
    ("blue", [0, 0, 255]),
    ("white", [255, 255, 255]),
    ("warm", [255, 147, 41]),
    ("cool", [201, 226, 255]),
    ("amber", [255, 191, 0]),
    ("orange", [255, 100, 0]),
    ("yellow", [255, 255, 0]),
    ("cyan", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("pink", [255, 105, 180]),
    ("purple", [128, 0, 255]),
    ("off", [0, 0, 0]),
];

fn color(f: &Placed, mode: &Mode, word: &str) -> Result<Vec<(usize, u8)>, String> {
    let w = word.trim().to_ascii_lowercase();
    let [r, g, b] = COLORS.iter().find(|c| c.0 == w).map(|c| c.1).ok_or_else(|| format!("'{word}' is not a color I know ({})", COLORS.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")))?;
    let base = f.address - 1;
    let ch = |name: &str| mode.attrs.get(name).map(|a| base + a.channel - 1);
    let (Some(rc), Some(gc), Some(bc)) = (ch("red"), ch("green"), ch("blue")) else {
        return Err(format!("{} ({}) has no red, green and blue, so it cannot show a color: set its own attributes ({})", f.name, f.ftype.id, mode.attrs.keys().cloned().collect::<Vec<_>>().join(", ")));
    };
    let mut out = Vec::new();
    // a fixture with its own white or amber emitter uses it for those colors, and RGB for the rest
    match (w.as_str(), ch("white"), ch("amber")) {
        ("white", Some(wc), _) => out.extend([(rc, 0), (gc, 0), (bc, 0), (wc, 255)]),
        ("amber", _, Some(ac)) => out.extend([(rc, 0), (gc, 0), (bc, 0), (ac, 255)]),
        _ => {
            out.extend([(rc, r), (gc, g), (bc, b)]);
            for extra in [ch("white"), ch("amber")].into_iter().flatten() {
                out.push((extra, 0));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVER: &str = r#"{"id":"mover-x","name":"Mover X","modes":[
        {"name":"short","channels":4,"attributes":{"pan":{"channel":1,"fine":2},"dimmer":{"channel":3},"strobe":{"channel":4,"values":{"off":0,"slow":40,"fast":200}}}},
        {"name":"long","channels":6,"attributes":{"pan":{"channel":1,"fine":2},"tilt":{"channel":3,"fine":4},"dimmer":{"channel":5},"strobe":{"channel":6}}}]}"#;

    fn with_mover(f: impl FnOnce()) {
        let dir = std::env::temp_dir().join(format!("pchi-fixtures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mover.json"), MOVER).unwrap();
        load_dir(&dir).unwrap();
        f();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_built_in_fixtures_are_valid() {
        for id in ["dimmer", "rgb", "rgbw", "rgb-dimmer"] {
            let f = get(id).unwrap_or_else(|| panic!("{id}"));
            assert!(f.built_in && !f.modes.is_empty());
        }
        assert_eq!(get("RGBW").unwrap().modes[0].channels, 4, "ids are not case sensitive");
    }

    #[test]
    fn a_fixture_file_is_checked() {
        let bad = |t: &str| FixtureType::from_json(t, false).err().unwrap_or_default();
        let with = |attrs: &str| format!(r#"{{"id":"t","name":"T","modes":[{{"name":"m","channels":4,"attributes":{{{attrs}}}}}]}}"#);
        assert!(FixtureType::from_json(&with(r#""dimmer":{"channel":1}"#), false).is_ok());
        assert!(bad(&with(r#""dimmer":{"channel":5}"#)).contains("outside the fixture's 4 channels"));
        assert!(bad(&with(r#""dimmer":{"channel":1},"red":{"channel":1}"#)).contains("used by both"));
        assert!(bad(&with(r#""pan":{"channel":1,"fine":1}"#)).contains("used by both"));
        assert!(bad(&with(r#""dimmer":{"channel":1,"values":{"x":300}}"#)).contains("0 to 255"));
        assert!(bad(&with(r#""Bad Name":{"channel":1}"#)).contains("must be a to z"));
        assert!(bad(&with("")).contains("no attributes"));
        assert!(bad(r#"{"id":"Bad Id","name":"T","modes":[]}"#).contains("id 'Bad Id'"));
        assert!(bad(r#"{"id":"t","name":"T","modes":[]}"#).contains("no modes"));
        assert!(bad("{").contains("not a fixture file"));
    }

    #[test]
    fn a_patch_places_fixtures_and_refuses_overlaps() {
        let p = Patch::parse("wash1=rgbw@1, wash2=rgbw@5; spot=rgb-dimmer@20\n walls=wash1+wash2").unwrap();
        assert_eq!(p.fixtures.iter().map(|f| (f.name.as_str(), f.address)).collect::<Vec<_>>(), [("wash1", 1), ("wash2", 5), ("spot", 20)]);
        assert_eq!(p.names(), ["wash1", "wash2", "spot", "walls"]);
        let e = |t: &str| Patch::parse(t).err().unwrap_or_default();
        assert!(e("a=rgbw@1, b=rgb@4").contains("b (rgb at 4) overlaps a (rgbw at 1): channel 4 is used twice"));
        assert!(e("a=rgbw@510").contains("must be 1 to 509"));
        assert!(e("a=rgbw@0").contains("must be 1 to 509"));
        assert!(e("a=nosuch@1").contains("no fixture type 'nosuch'"));
        assert!(e("a=rgbw").contains("group a: 'rgbw' is not a patched fixture"), "an entry with no @ is a group: {}", e("a=rgbw"));
        assert!(e("a=rgbw@x").contains("not a DMX address"));
        assert!(e("a=rgbw@1, a=rgb@9").contains("patched twice"));
        assert!(e("1a=rgb@1").contains("not a usable fixture name"));
        assert!(e("blackout=rgb@1").contains("not a usable fixture name"));
        assert!(e("a=rgb@1, g=a+zz").contains("'zz' is not a patched fixture"));
        assert!(e("a=rgb:wide@1").contains("no mode 'wide'"));
        assert!(e("").contains("no fixtures"));
    }

    #[test]
    fn a_color_or_a_level_is_worked_out_per_fixture() {
        let p = Patch::parse("par=rgbw@1, led=rgb-dimmer@10, plain=rgb@20, lamp=dimmer@30, all=par+led").unwrap();
        let get = |v: Vec<(usize, u8)>, ch: usize| v.iter().find(|(c, _)| *c == ch - 1).map(|x| x.1);
        // blue on an RGBW par sets red, green, blue and clears white
        let blue = p.resolve("par", None, "blue").unwrap();
        assert_eq!((get(blue.clone(), 1), get(blue.clone(), 2), get(blue.clone(), 3), get(blue, 4)), (Some(0), Some(0), Some(255), Some(0)));
        // white uses the white emitter when there is one, and all three colors when there is not
        let w = p.resolve("par", None, "white").unwrap();
        assert_eq!((get(w.clone(), 1), get(w.clone(), 4)), (Some(0), Some(255)));
        let w2 = p.resolve("plain", None, "white").unwrap();
        assert_eq!((get(w2.clone(), 20), get(w2.clone(), 21), get(w2, 22)), (Some(255), Some(255), Some(255)));
        // a bare level is the dimmer; a fixture with none says so
        let d = p.resolve("led", None, "50%").unwrap();
        assert_eq!(get(d, 10), Some(128));
        assert!(p.resolve("plain", None, "50%").unwrap_err().contains("set a color instead"));
        assert!(p.resolve("par", Some("dimmer"), "50%").unwrap_err().contains("has no 'dimmer'"));
        assert_eq!(get(p.resolve("lamp", None, "full").unwrap(), 30), Some(255));
        assert_eq!(get(p.resolve("lamp", None, "out").unwrap(), 30), Some(0));
        // a group is every member
        let g = p.resolve("all", None, "red").unwrap();
        assert_eq!((get(g.clone(), 1), get(g.clone(), 11), get(g, 12)), (Some(255), Some(255), Some(0)));
        // explicit attributes
        assert_eq!(get(p.resolve("par", Some("green"), "100%").unwrap(), 2), Some(255));
        // mistakes say what is there
        assert!(p.resolve("nope", None, "blue").unwrap_err().contains("patched: par, led, plain, lamp, all"));
        assert!(p.resolve("par", None, "mauve").unwrap_err().contains("not a color I know"));
        assert!(p.resolve("par", Some("dimmer"), "300").is_err());
        assert!(p.resolve("lamp", None, "blue").unwrap_err().contains("cannot show a color"));
    }

    #[test]
    fn sixteen_bit_attributes_and_named_values_work() {
        with_mover(|| {
            let p = Patch::parse("m1=mover-x:long@1, m2=mover-x:short@20").unwrap();
            let get = |v: &Vec<(usize, u8)>, ch: usize| v.iter().find(|(c, _)| *c == ch - 1).map(|x| x.1);
            // pan 50% is 32768: coarse 128, fine 0; 25% is 16384
            let pan = p.resolve("m1", Some("pan"), "50%").unwrap();
            assert_eq!((get(&pan, 1), get(&pan, 2)), (Some(128), Some(0)));
            let pan = p.resolve("m1", Some("pan"), "75%").unwrap();
            assert_eq!((get(&pan, 1), get(&pan, 2)), (Some(191), Some(255)), "49151 = 191 * 256 + 255");
            let pan = p.resolve("m1", Some("pan"), "full").unwrap();
            assert_eq!((get(&pan, 1), get(&pan, 2)), (Some(255), Some(255)));
            // an 8-bit number is a coarse value with no fine
            let pan = p.resolve("m1", Some("tilt"), "100").unwrap();
            assert_eq!((get(&pan, 3), get(&pan, 4)), (Some(100), Some(100)), "100 of 255 spread over 16 bits keeps the fine byte in step");
            // named values
            let s = p.resolve("m2", Some("strobe"), "fast").unwrap();
            assert_eq!(get(&s, 23), Some(200));
            assert!(p.resolve("m2", Some("strobe"), "flicker").unwrap_err().contains("or one of: fast, off, slow"));
            // the mode picks the channel map: the same attribute is on a different channel
            assert_eq!(get(&p.resolve("m1", None, "100%").unwrap(), 5), Some(255));
            assert_eq!(get(&p.resolve("m2", None, "100%").unwrap(), 22), Some(255));
            assert!(Patch::parse("m1=mover-x@1").err().unwrap().contains("has several modes (short, long)"));
        });
    }
}
