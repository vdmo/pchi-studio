//! The rig as the runtime sees it: which devices exist, which department each
//! serves, and where to reach them. Mirrors `pchi-sdk/src/rig.ts` (rigDept,
//! laneDevice); the tests below pin the two to the same table.

use pchi_showchart::Showchart;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub dept: String,
    pub host: Option<String>,
    pub port: Option<u16>,
}

impl Device {
    pub fn address(&self) -> Option<String> {
        match (&self.host, self.port) {
            (Some(h), Some(p)) if !h.trim().is_empty() => Some(format!("{}:{}", h.trim(), p)),
            _ => None,
        }
    }
}

/// The department a device type serves (same table as the SDK's `rigDept`).
pub fn dept_of_type(t: &str) -> Option<&'static str> {
    Some(match t {
        "vork" | "unreal" | "touchdesigner" | "notch" | "projector" | "led-wall" | "resolume" => "visual",
        "lighting" | "lighting-desk" | "grandma3" | "eos" => "lighting",
        "laser" | "beyond" => "laser",
        "audio-zone" | "audio-console" | "audio-agent" | "x32" => "audio",
        _ => return None,
    })
}

/// The department an action kind belongs to (same as the cue sheet's `deptFor`).
pub fn dept_of_kind(kind: &str) -> &'static str {
    if kind.starts_with("scene") || kind.starts_with("renderer") || kind.starts_with("visual") {
        "visual"
    } else if kind.starts_with("lighting") {
        "lighting"
    } else if kind.starts_with("laser") {
        "laser"
    } else if kind.starts_with("audio") {
        "audio"
    } else {
        "other"
    }
}

/// Read a show file. It may be a bare showchart, or a Studio `.pchi` file with
/// the showchart under `showchart` and the rig under `nodes`. The rig is `None`
/// for a bare showchart (nothing to route to).
pub fn load(text: &str) -> Result<(Showchart, Option<Vec<Device>>), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if let Some(sc) = v.get("showchart").filter(|s| s.is_object()) {
        let chart: Showchart = serde_json::from_value(sc.clone()).map_err(|e| format!("showchart: {e}"))?;
        let devices = v
            .get("nodes")
            .and_then(Value::as_array)
            .map(|ns| ns.iter().filter_map(device_of).collect::<Vec<_>>());
        return Ok((chart, devices));
    }
    let chart: Showchart = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok((chart, None))
}

fn device_of(n: &Value) -> Option<Device> {
    let kind = n.get("type")?.as_str()?.to_string();
    let dept = n
        .get("dept")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| dept_of_type(&kind).map(str::to_string))?;
    let params = n.get("params");
    let host = params.and_then(|p| p.get("host")).and_then(Value::as_str).map(str::to_string);
    let port = params.and_then(|p| p.get("port")).and_then(|p| p.as_u64().or_else(|| p.as_str().and_then(|s| s.trim().parse().ok()))).and_then(|p| u16::try_from(p).ok());
    let id = n.get("id")?.as_str()?.to_string();
    Some(Device { label: n.get("label").and_then(Value::as_str).unwrap_or(&id).to_string(), id, kind, dept, host, port })
}

/// The device that handles a cue: the lane's chosen one, else the only device of
/// its department. Several candidates and no choice is an error, never a guess.
pub fn resolve<'a>(devices: &'a [Device], dept: &str, device: Option<&str>) -> Result<&'a Device, String> {
    if let Some(id) = device {
        return devices.iter().find(|d| d.id == id).ok_or_else(|| format!("device '{id}' is not in the rig"));
    }
    let c: Vec<&Device> = devices.iter().filter(|d| d.dept == dept).collect();
    match c.len() {
        1 => Ok(c[0]),
        0 => Err(format!("no {dept} device in the rig")),
        n => Err(format!("{n} {dept} devices in the rig: choose one on the lane")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, kind: &str) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: dept_of_type(kind).unwrap().into(), host: Some("127.0.0.1".into()), port: Some(9000) }
    }

    #[test]
    fn types_map_to_the_same_departments_as_the_sdk() {
        assert_eq!(dept_of_type("laser"), Some("laser"));
        assert_eq!(dept_of_type("notch"), Some("visual"));
        assert_eq!(dept_of_type("audio-console"), Some("audio"));
        assert_eq!(dept_of_type("lighting-desk"), Some("lighting"));
        assert_eq!(dept_of_type("resolume"), Some("visual"));
        assert_eq!(dept_of_type("grandma3"), Some("lighting"));
        assert_eq!(dept_of_type("eos"), Some("lighting"));
        assert_eq!(dept_of_type("x32"), Some("audio"));
        assert_eq!(dept_of_type("beyond"), Some("laser"));
        assert_eq!(dept_of_type("ndi"), None);
        assert_eq!(dept_of_type("cue"), None);
        assert_eq!(dept_of_kind("lighting.look"), "lighting");
        assert_eq!(dept_of_kind("scene.activate"), "visual");
        assert_eq!(dept_of_kind("cue.trigger"), "other");
    }

    #[test]
    fn resolve_uses_the_choice_else_the_only_candidate_and_never_guesses() {
        let rig = vec![dev("l1", "laser"), dev("d1", "lighting-desk"), dev("d2", "lighting-desk")];
        assert_eq!(resolve(&rig, "laser", None).unwrap().id, "l1");
        assert!(resolve(&rig, "lighting", None).unwrap_err().contains("choose one"));
        assert_eq!(resolve(&rig, "lighting", Some("d2")).unwrap().id, "d2");
        assert!(resolve(&rig, "audio", None).unwrap_err().contains("no audio device"));
        assert!(resolve(&rig, "laser", Some("gone")).unwrap_err().contains("not in the rig"));
    }

    #[test]
    fn loads_a_studio_file_or_a_bare_showchart() {
        let bare = r#"{"initial":"A","states":[{"id":"A"}],"transitions":[]}"#;
        let (c, rig) = load(bare).unwrap();
        assert_eq!(c.initial, "A");
        assert!(rig.is_none());

        let studio = r#"{"version":"1.0","nodes":[
            {"id":"laser-1","type":"laser","label":"Main laser","params":{"host":"10.0.0.5","port":8000}},
            {"id":"cue-1","type":"cue","label":"Cue","params":{}},
            {"id":"x","type":"osc","dept":"lighting","label":"Desk","params":{"host":"h","port":"7700"}}],
            "showchart":{"initial":"A","states":[{"id":"A"}],"transitions":[]}}"#;
        let (c, rig) = load(studio).unwrap();
        assert_eq!(c.initial, "A");
        let rig = rig.unwrap();
        assert_eq!(rig.len(), 2, "the cue node serves no department");
        assert_eq!(rig[0].address().as_deref(), Some("10.0.0.5:8000"));
        assert_eq!(rig[1].dept, "lighting");
        assert_eq!(rig[1].port, Some(7700));
        assert!(load("{").is_err());
    }
}
