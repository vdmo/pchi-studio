//! Bring a cue list from another tool into a PCHI show, so a real show can be tried without retyping it.
//!
//! Two sources:
//!
//! * **A cue sheet (CSV/TSV)**: the format every tool can export and every stage manager can type. One row a cue; see `docs/import.md`.
//! * **A QLab 4 workspace** (`.qlab4`, an XML property list): its cues, timed from their pre-waits and continue modes, grouped into one scene per GO.
//!
//! What comes in is a *timeline of cues*: when each one fires, what it is called, and, where PCHI has an equivalent, what it does (a Light cue becomes a lighting
//! look, and so on). What does not have an equivalent (an audio file to play, a video, an OSC message to some other tool) comes in as a cue with the original's
//! name and notes on the right department's lane, routed nowhere, so the sheet is complete and nothing is silently dropped; the report lists each such cue.
//! Nothing is ever sent for a cue that was not mapped. `--map TYPE=KIND` maps a QLab cue type to a PCHI kind of your choosing.
//!
//! What it does not try: QLab 5 workspaces, Chataigne and ossia files (not read: no sample of their format was available to check against), fades and
//! levels inside cues, and the *content* of media. The QLab reader follows the documented structure of a QLab 4 property list and was tested on workspaces written
//! by hand from it; a real workspace may use something it does not know, and then the report says what it skipped.

use pchi_showchart::ShowEngine;
use serde_json::{json, Value};
use std::collections::HashMap;

/// One cue, before it is placed in a scene.
#[derive(Clone, Debug)]
struct Item {
    /// The scene it belongs to (empty: the only one).
    scene: String,
    at_ms: u64,
    lane: String,
    dept: String,
    kind: String,
    payload: Value,
    label: String,
    num: Option<f64>,
    length_ms: Option<u64>,
}

/// The result of an import.
#[derive(Debug)]
pub struct Imported {
    /// The whole `.pchi` file, ready to run or open in Studio.
    pub file: Value,
    /// What came in, and what did not (one sentence each).
    pub notes: Vec<String>,
    pub cues: usize,
    pub scenes: usize,
}

fn slug(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let t = t.trim_matches('_').to_string();
    if t.is_empty() { "show".into() } else { t }
}

/// The department a PCHI kind belongs to, as a lane name.
fn dept_lane(kind: &str, given: &str) -> (String, String) {
    // a cue with no PCHI action must never be routed to a real device: it sits on the notes lane, department "other"
    if kind.starts_with("import.") || kind.starts_with("note.") {
        return ("other".into(), "notes".into());
    }
    let d = crate::rig::dept_of_kind(kind);
    let dept = if d == "other" { given.trim().to_ascii_lowercase() } else { d.to_string() };
    let dept = if dept.is_empty() { "other".to_string() } else { dept };
    (dept.clone(), if dept == "other" { "notes".into() } else { dept })
}

/// The action for a friendly name (`light`, `audio`, `laser`, `scene`) or a full kind (`lighting.look`), with `value` as its argument.
fn action_for(word: &str, value: &str) -> (String, Value) {
    let w = word.trim().to_ascii_lowercase();
    let (kind, key) = match w.as_str() {
        "light" | "lights" | "lighting" => ("lighting.look", "look"),
        "audio" | "sound" => ("audio.snapshot", "snapshot"),
        "laser" => ("laser.cue", "cue"),
        "scene" | "visual" | "video" => ("scene.activate", "scene"),
        _ if w.contains('.') => return (w, json!({"value": value})),
        _ => return (format!("note.{}", slug(&w).to_ascii_lowercase()), json!({"value": value})),
    };
    (kind.to_string(), json!({ key: value }))
}

/// `1:23.5`, `0:01:23`, `83.5`, `83500ms` -> milliseconds.
pub fn parse_time(text: &str) -> Result<u64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("empty time".into());
    }
    if let Some(ms) = t.strip_suffix("ms") {
        return ms.trim().parse::<f64>().map(|v| v.max(0.0).round() as u64).map_err(|_| format!("'{t}' is not a time"));
    }
    let mut total = 0.0f64;
    for part in t.split(':') {
        let v: f64 = part.trim().parse().map_err(|_| format!("'{t}' is not a time (write 1:23.5, 0:01:23 or 83.5)"))?;
        if v < 0.0 {
            return Err(format!("'{t}' is before the start"));
        }
        total = total * 60.0 + v;
    }
    Ok((total * 1000.0).round() as u64)
}

fn split_row(line: &str, delim: char) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

fn build(name: &str, items: Vec<Item>, mut notes: Vec<String>) -> Result<Imported, String> {
    if items.is_empty() {
        return Err("no cues were found".into());
    }
    let mut scene_names: Vec<String> = Vec::new();
    for it in &items {
        let s = if it.scene.is_empty() { "RUN".to_string() } else { it.scene.clone() };
        if !scene_names.contains(&s) {
            scene_names.push(s);
        }
    }
    // unique, safe ids
    let mut ids: Vec<String> = Vec::new();
    for s in &scene_names {
        let mut id = slug(s);
        while id == "PRESHOW" || ids.contains(&id) {
            id.push('_');
        }
        ids.push(id);
    }
    let mut states = vec![json!({"id": "PRESHOW"})];
    let mut transitions = Vec::new();
    let mut total = 0usize;
    for (i, s) in scene_names.iter().enumerate() {
        let mut lanes: Vec<(String, String, Vec<Value>)> = Vec::new();
        let mut longest = 0u64;
        let mut mine: Vec<&Item> = items.iter().filter(|it| (if it.scene.is_empty() { "RUN" } else { it.scene.as_str() }) == s).collect();
        mine.sort_by_key(|it| it.at_ms);
        for (n, it) in mine.iter().enumerate() {
            longest = longest.max(it.at_ms + it.length_ms.unwrap_or(0));
            let lane_i = match lanes.iter().position(|l| l.0 == it.lane) {
                Some(p) => p,
                None => {
                    lanes.push((it.lane.clone(), it.dept.clone(), Vec::new()));
                    lanes.len() - 1
                }
            };
            let mut cue = json!({"id": format!("{}_{}", ids[i], n + 1), "at_ms": it.at_ms, "label": it.label, "action": {"kind": it.kind, "payload": it.payload}});
            if let Some(num) = it.num {
                cue["num"] = json!(num);
            }
            if let Some(l) = it.length_ms {
                cue["length_ms"] = json!(l);
            }
            lanes[lane_i].2.push(cue);
            total += 1;
        }
        let lanes_json: Vec<Value> = lanes.into_iter().map(|(id, dept, cues)| json!({"id": id, "name": id, "dept": dept, "cues": cues})).collect();
        states.push(json!({"id": ids[i], "timeline": {"duration_ms": longest + 2_000, "lanes": lanes_json}}));
        transitions.push(json!({"source": if i == 0 { "PRESHOW".to_string() } else { ids[i - 1].clone() }, "target": ids[i], "event": "GO"}));
    }
    let id = slug(name).to_ascii_lowercase();
    let chart = json!({"id": id, "name": name, "version": "0.1.0", "initial": "PRESHOW", "states": states, "transitions": transitions});
    // it must be a show the runtime accepts
    let parsed: pchi_showchart::Showchart = serde_json::from_value(chart.clone()).map_err(|e| format!("the imported show is not valid: {e}"))?;
    if let Err(errs) = ShowEngine::new(parsed) {
        return Err(format!("the imported show is not valid: {}", errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")));
    }
    notes.push(format!("imported {total} cue(s) in {} scene(s). Press GO to enter the first; each later GO enters the next.", scene_names.len()));
    let file = json!({"version": "1.0", "project": id, "name": name, "edges": [], "timeline": [], "assets": [], "nodes": [], "showchart": chart});
    Ok(Imported { file, notes, cues: total, scenes: scene_names.len() })
}

/// A cue sheet: a header row, then one row a cue. Columns (any order, names case-insensitive): `time` (required), `action` or `kind` (required),
/// `value` or `target`, `label` or `name`, `number` or `cue`, `lane`, `length`, `scene`.
pub fn from_csv(text: &str, name: &str) -> Result<Imported, String> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let head = lines.next().ok_or("the file is empty")?;
    let delim = [',', '\t', ';'].into_iter().max_by_key(|d| head.matches(*d).count()).unwrap_or(',');
    let cols: Vec<String> = split_row(head, delim).into_iter().map(|c| c.to_ascii_lowercase()).collect();
    let find = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
    let (c_time, c_action) = (find(&["time", "at", "start"]), find(&["action", "kind", "type"]));
    let (Some(c_time), Some(c_action)) = (c_time, c_action) else {
        return Err(format!("the first row must name the columns, at least `time` and `action` (found: {})", cols.join(", ")));
    };
    let (c_value, c_label, c_num, c_lane, c_len, c_scene) = (find(&["value", "target", "argument"]), find(&["label", "name", "description"]), find(&["number", "cue", "q"]), find(&["lane", "dept", "department"]), find(&["length", "duration"]), find(&["scene", "section"]));
    let mut items = Vec::new();
    let mut notes = Vec::new();
    for (i, line) in lines.enumerate() {
        let row = split_row(line, delim);
        let at = |c: Option<usize>| c.and_then(|c| row.get(c)).map(|s| s.as_str()).unwrap_or("");
        let at_ms = parse_time(at(Some(c_time))).map_err(|e| format!("row {}: {e}", i + 2))?;
        let word = at(Some(c_action));
        if word.is_empty() {
            return Err(format!("row {}: no action", i + 2));
        }
        let value = at(c_value);
        let (kind, payload) = action_for(word, value);
        let (dept, mut lane) = dept_lane(&kind, at(c_lane));
        if !at(c_lane).is_empty() && !kind.starts_with("note.") {
            lane = slug(at(c_lane)).to_ascii_lowercase();
        }
        if kind.starts_with("note.") {
            notes.push(format!("row {}: '{word}' is not a PCHI action: kept as a note on the {lane} lane, nothing will be sent for it", i + 2));
        }
        let length_ms = if at(c_len).is_empty() { None } else { Some(parse_time(at(c_len)).map_err(|e| format!("row {}: length: {e}", i + 2))?) };
        items.push(Item { scene: at(c_scene).to_string(), at_ms, lane, dept, kind, payload, label: if at(c_label).is_empty() { value.to_string() } else { at(c_label).to_string() }, num: at(c_num).trim_start_matches(['Q', 'q']).parse().ok(), length_ms });
    }
    build(name, items, notes)
}

// ---- QLab 4 ----

fn plist_dict<'a, 'i>(node: roxmltree::Node<'a, 'i>) -> Vec<(String, roxmltree::Node<'a, 'i>)> {
    let kids: Vec<_> = node.children().filter(|c| c.is_element()).collect();
    kids.chunks(2).filter(|p| p.len() == 2 && p[0].tag_name().name() == "key").map(|p| (p[0].text().unwrap_or("").to_string(), p[1])).collect()
}

fn get<'a, 'i>(d: &[(String, roxmltree::Node<'a, 'i>)], key: &str) -> Option<roxmltree::Node<'a, 'i>> {
    d.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

fn text_of(n: Option<roxmltree::Node>) -> String {
    n.and_then(|n| n.text()).unwrap_or("").trim().to_string()
}

fn real_of(n: Option<roxmltree::Node>) -> f64 {
    text_of(n).parse().unwrap_or(0.0)
}

struct QCue {
    kind: String,
    number: String,
    name: String,
    notes: String,
    pre: f64,
    post: f64,
    dur: f64,
    cont: i64,
    children: Vec<QCue>,
}

fn qcues(array: roxmltree::Node) -> Vec<QCue> {
    array
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "dict")
        .map(|c| {
            let d = plist_dict(c);
            let name = text_of(get(&d, "name"));
            QCue {
                kind: text_of(get(&d, "type")),
                number: text_of(get(&d, "number")),
                name: if name.is_empty() { text_of(get(&d, "listName")) } else { name },
                notes: text_of(get(&d, "notes")),
                pre: real_of(get(&d, "preWait")),
                post: real_of(get(&d, "postWait")),
                dur: real_of(get(&d, "duration")),
                cont: text_of(get(&d, "continueMode")).parse().unwrap_or(0),
                children: get(&d, "cues").map(qcues).unwrap_or_default(),
            }
        })
        .collect()
}

/// A QLab 4 workspace. Each GO (a cue not reached by the one before's auto-continue or auto-follow) becomes a scene; cues reached by continue are placed after
/// their predecessor: auto-continue after its post-wait counted from when its action started (after its pre-wait), auto-follow after its action ended.
/// `map` sends a QLab cue type to a PCHI kind (`Audio` -> `audio.snapshot`); `Light` is `lighting.look` unless remapped.
pub fn from_qlab4(xml: &str, name: &str, map: &HashMap<String, String>) -> Result<Imported, String> {
    let doc = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| format!("not a QLab 4 workspace (it is not XML: {e}). QLab 5 workspaces are not read."))?;
    let root = doc.root_element();
    if root.tag_name().name() != "plist" {
        return Err("not a QLab 4 workspace: expected a property list".into());
    }
    let top = root.children().find(|c| c.is_element() && c.tag_name().name() == "dict").ok_or("not a QLab 4 workspace: the property list is empty")?;
    let d = plist_dict(top);
    let lists = get(&d, "cueLists").ok_or("not a QLab 4 workspace: it has no cue lists")?;
    let mut cues: Vec<QCue> = Vec::new();
    for l in qcues(lists) {
        // a cue list is itself a cue-shaped dict whose `cues` are the list
        cues.extend(l.children);
    }
    if cues.is_empty() {
        return Err("the workspace has no cues".into());
    }
    let mut items: Vec<Item> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut scene_n = 0usize;
    // `go` is the time (ms, in the current scene) the next cue is triggered at; None means it waits for a GO
    fn place(list: &[QCue], base_ms: f64, own_go: bool, map: &HashMap<String, String>, items: &mut Vec<Item>, notes: &mut Vec<String>, scene_n: &mut usize, scene: &mut String) {
        let mut go: Option<f64> = if own_go { None } else { Some(base_ms) };
        for c in list {
            let t = match go {
                Some(t) => t,
                None if !own_go => base_ms,
                None => {
                    *scene_n += 1;
                    *scene = format!("{}{}", if c.number.is_empty() { format!("Q{}", scene_n) } else { format!("Q{}", c.number) }, if c.name.is_empty() { String::new() } else { format!(" {}", c.name) });
                    0.0
                }
            };
            let start = t + c.pre * 1000.0;
            if c.kind.eq_ignore_ascii_case("group") {
                notes.push(format!("group '{}' was flattened: its children are timed from the group's start (continue and follow modes are respected; a child that has neither starts with the group; QLab's group modes are not interpreted)", c.name));
                place(&c.children, start, false, map, items, notes, scene_n, scene);
            } else if !matches!(c.kind.to_ascii_lowercase().as_str(), "memo" | "wait") || !c.name.is_empty() {
                let kind_key = c.kind.clone();
                let (kind, payload, mapped) = match map.get(&kind_key).or_else(|| map.get(&kind_key.to_ascii_lowercase())) {
                    Some(k) => (k.clone(), json!({"name": c.name, "number": c.number}), true),
                    None if c.kind.eq_ignore_ascii_case("light") => ("lighting.look".to_string(), json!({"look": c.name, "name": c.name, "number": c.number}), true),
                    None => (format!("import.{}", slug(&c.kind).to_ascii_lowercase()), json!({"name": c.name, "number": c.number, "notes": c.notes, "qlab_type": c.kind}), false),
                };
                // map targets with a known argument name get it too
                let mut payload = payload;
                if mapped {
                    if let Some(key) = match kind.as_str() { "audio.snapshot" => Some("snapshot"), "laser.cue" => Some("cue"), "scene.activate" => Some("scene"), "lighting.look" => Some("look"), _ => None } {
                        payload[key] = json!(c.name);
                    }
                }
                if !mapped {
                    notes.push(format!("cue {} '{}' is a QLab {} cue: kept on the sheet as a note, nothing will be sent (use --map {}=<kind> to give it an action)", c.number, c.name, c.kind, c.kind));
                }
                let (dept, lane) = dept_lane(&kind, &if mapped { String::new() } else { c.kind.to_ascii_lowercase() });
                items.push(Item { scene: scene.clone(), at_ms: start.max(0.0).round() as u64, lane, dept, kind, payload, label: if c.name.is_empty() { c.kind.clone() } else { c.name.clone() }, num: c.number.trim_start_matches(['Q', 'q']).parse().ok(), length_ms: if c.dur > 0.0 { Some((c.dur * 1000.0).round() as u64) } else { None } });
            }
            go = match c.cont {
                1 => Some(start + c.post * 1000.0),
                2 => Some(start + (c.dur + c.post) * 1000.0),
                _ => None,
            };
        }
    }
    let mut scene = String::new();
    place(&cues, 0.0, true, map, &mut items, &mut notes, &mut scene_n, &mut scene);
    notes.dedup();
    build(name, items, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = "number,time,scene,action,value,label,length\n1,0:00,Opening,light,warm,House to half,\n2,0:02.5,Opening,audio,sting,Sting,3\n3,0:05,Opening,Fireworks,big,Pyro (not ours),\n4,0:00,Finale,laser,finale1,Laser finale,\n";

    #[test]
    fn a_cue_sheet_becomes_a_show_the_runtime_accepts() {
        let r = from_csv(SHEET, "Gala").unwrap();
        assert_eq!((r.cues, r.scenes), (4, 2));
        let chart = &r.file["showchart"];
        assert_eq!(chart["initial"], "PRESHOW");
        let opening = chart["states"].as_array().unwrap().iter().find(|s| s["id"] == "Opening").unwrap();
        let lanes = opening["timeline"]["lanes"].as_array().unwrap();
        let lighting = lanes.iter().find(|l| l["id"] == "lighting").unwrap();
        assert_eq!(lighting["cues"][0]["action"], json!({"kind": "lighting.look", "payload": {"look": "warm"}}));
        let audio = lanes.iter().find(|l| l["id"] == "audio").unwrap();
        assert_eq!((audio["cues"][0]["at_ms"].clone(), audio["cues"][0]["length_ms"].clone()), (json!(2500), json!(3000)));
        // the cue with no PCHI action is kept as a note and said so
        assert!(r.notes.iter().any(|n| n.contains("Fireworks") && n.contains("nothing will be sent")), "{:?}", r.notes);
        assert!(lanes.iter().any(|l| l["id"] == "notes"));
        // two GOs: into Opening, then into Finale
        assert_eq!(chart["transitions"].as_array().unwrap().len(), 2);
        // and the runtime reads it back
        let (chart, devices) = crate::rig::load(&r.file.to_string()).unwrap();
        assert_eq!(chart.id.as_deref(), Some("gala"));
        assert!(devices.map_or(true, |d| d.is_empty()));
    }

    #[test]
    fn times_and_mistakes_are_read_or_refused_in_words() {
        assert_eq!(parse_time("1:23.5").unwrap(), 83_500);
        assert_eq!(parse_time("0:01:23").unwrap(), 83_000);
        assert_eq!(parse_time("83.5").unwrap(), 83_500);
        assert_eq!(parse_time("250ms").unwrap(), 250);
        assert!(parse_time("soon").unwrap_err().contains("not a time"));
        assert!(from_csv("a,b\n1,2\n", "x").unwrap_err().contains("`time` and `action`"));
        assert!(from_csv("time,action\nlater,light\n", "x").unwrap_err().contains("row 2"));
        assert!(from_csv("time,action\n", "x").unwrap_err().contains("no cues"));
        // tabs and semicolons, quotes
        let r = from_csv("time;action;label\n0:01;light;\"warm; low\"\n", "x").unwrap();
        assert_eq!(r.file["showchart"]["states"][1]["timeline"]["lanes"][0]["cues"][0]["label"], "warm; low");
    }

    const QLAB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>cueLists</key><array><dict><key>type</key><string>Cue List</string><key>cues</key><array>
<dict><key>type</key><string>Light</string><key>number</key><string>1</string><key>name</key><string>Warm wash</string><key>preWait</key><real>0</real><key>postWait</key><real>2</real><key>continueMode</key><integer>1</integer></dict>
<dict><key>type</key><string>Audio</string><key>number</key><string>2</string><key>name</key><string>Sting</string><key>preWait</key><real>0.5</real><key>duration</key><real>4</real><key>continueMode</key><integer>0</integer></dict>
<dict><key>type</key><string>Light</string><key>number</key><string>3</string><key>name</key><string>Fade out</string><key>preWait</key><real>0</real><key>continueMode</key><integer>0</integer></dict>
<dict><key>type</key><string>Group</string><key>number</key><string>4</string><key>name</key><string>Finale</string><key>cues</key><array>
<dict><key>type</key><string>Light</string><key>name</key><string>Full</string><key>duration</key><real>3</real><key>continueMode</key><integer>2</integer></dict>
<dict><key>type</key><string>Video</string><key>name</key><string>Fireworks clip</string><key>preWait</key><real>1</real></dict></array></dict>
</array></dict></array></dict></plist>"#;

    #[test]
    fn a_qlab_workspace_is_timed_from_its_waits_and_continue_modes() {
        let r = from_qlab4(QLAB, "Gala", &HashMap::new()).unwrap();
        assert_eq!(r.scenes, 3, "three GOs: cue 1 (with 2 chained to it), cue 3, group 4");
        let states = r.file["showchart"]["states"].as_array().unwrap();
        let cue_at = |scene: usize, lane: &str, i: usize| states[scene]["timeline"]["lanes"].as_array().unwrap().iter().find(|l| l["id"] == lane).unwrap()["cues"][i]["at_ms"].clone();
        // Q1 starts at 0; auto-continue after its 2 s post-wait; the audio cue then waits its 0.5 s pre-wait
        assert_eq!(cue_at(1, "lighting", 0), json!(0));
        assert_eq!(cue_at(1, "notes", 0), json!(2500));
        // the next GO (cue 3) is its own scene starting at zero
        assert_eq!(cue_at(2, "lighting", 0), json!(0));
        // the group's children are timed from its start; the first auto-follows (3 s long), then the video waits its 1 s pre-wait
        assert_eq!(cue_at(3, "lighting", 0), json!(0));
        assert_eq!(cue_at(3, "notes", 0), json!(4000));
        assert!(r.notes.iter().any(|n| n.contains("Audio") && n.contains("--map")), "{:?}", r.notes);
        assert!(r.notes.iter().any(|n| n.contains("group 'Finale' was flattened")), "{:?}", r.notes);
        // with a mapping, the audio cue has an action
        let mut map = HashMap::new();
        map.insert("Audio".to_string(), "audio.snapshot".to_string());
        let r = from_qlab4(QLAB, "Gala", &map).unwrap();
        let audio = r.file["showchart"]["states"][1]["timeline"]["lanes"].as_array().unwrap().iter().find(|l| l["id"] == "audio").unwrap()["cues"][0].clone();
        assert_eq!(audio["action"], json!({"kind": "audio.snapshot", "payload": {"name": "Sting", "number": "2", "snapshot": "Sting"}}));
    }

    #[test]
    fn what_is_not_a_qlab_4_workspace_is_refused_in_words() {
        assert!(from_qlab4("not xml at all", "x", &HashMap::new()).unwrap_err().contains("QLab 5 workspaces are not read"));
        assert!(from_qlab4("<plist><dict><key>a</key><string>b</string></dict></plist>", "x", &HashMap::new()).unwrap_err().contains("no cue lists"));
    }
}
