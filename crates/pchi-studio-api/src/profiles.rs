//! Device profiles: how a PCHI action becomes the OSC a popular tool actually understands.
//!
//! A rig device whose type is one of the five below (`resolume`, `grandma3`, `eos`, `x32`,
//! `beyond`) is sent that tool's own messages instead of the generic `/pchi/...` ones.
//!
//! What each mapping rests on (checked against the vendor's documentation unless noted):
//! * **grandMA3**: the command line is reached with address `/cmd`, type `s` (MA3 manual,
//!   "Remote Inputs OSC"; the manual's example is `Go+ Exec 402`).
//! * **ETC Eos**: `/eos/cue/<n>/fire` and `/eos/cue/<list>/<n>/fire` (ETC support article).
//! * **Pangolin BEYOND**: `/beyond/general/StartCue` (s), `StopCue` (s), `BlackOut`,
//!   `EnableLaserOutput`, `DisableLaserOutput` (Pangolin wiki, "OSC commands").
//! * **Resolume Arena**: the clip pattern `/composition/layers/<l>/clips/<c>/connect` and the column
//!   form `/composition/columns/<n>/connect` are both confirmed by community sources (a Workcad manual
//!   shows the exact column packet with integer 1) but not by a Resolume page; both use the integer 1. Any other address can be used by
//!   giving it in full (Resolume's own OSC Shortcut mode shows each control's address).
//! * **Behringer X32**: `/-action/goscene` (i) is from memory and NOT confirmed against a desk.
//!   (`/ch/NN/mix/on` and `/ch/NN/mix/fader` are confirmed but are not used here.)
//!
//! * **QLab**: `/cue/<number>/start` plus `/stop`, `/panic`, `/pause`, `/resume`. QLab's OSC dictionary
//!   confirms cue addressing by number (`/cue/{cue_number}/...`) and those workspace commands; the
//!   `start` entry itself was past what could be read, so it is long-standing knowledge to re-check.
//! * **ChamSys MagicQ**: `/pb/<playback>/go`, `/pb/<playback>/<cue>`, `/pb/<playback>/release` from
//!   ChamSys's OSC page as reported by search results (the page itself could not be fetched here).
//!   MagicQ PC only controls the first 10 playbacks.
//!
//! Anything not confirmed is meant to be tried against the real tool (or its software version)
//! before a show depends on it. Ports are never assumed: they are whatever is set in the tool.

use pchi_showchart::Action;
use rosc::{encoder, OscMessage, OscPacket, OscType};
use serde_json::Value;

/// The device types that have a profile.
pub const PROFILES: [&str; 7] = ["resolume", "grandma3", "eos", "x32", "beyond", "qlab", "magicq"];

/// The command words grandMA3 may be sent from a show file. Playback commands only: a show file
/// must never be able to delete, store or overwrite anything on a console.
const MA_ALLOWED: [&str; 9] = ["go+", "go-", "go", "goback", "goto", "pause", "off", "on", "flash"];

fn pack(addr: &str, args: Vec<OscType>) -> Result<Vec<u8>, String> {
    encoder::encode(&OscPacket::Message(OscMessage { addr: addr.to_string(), args })).map_err(|e| format!("{e:?}"))
}

/// The action's first payload value as text ("blue", 12, "2/3" all arrive as text).
fn value(action: &Action) -> Result<String, String> {
    let v = match &action.payload {
        Value::Object(p) => p.values().next().cloned(),
        _ => None,
    };
    let text = match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    if text.is_empty() {
        Err(format!("{} has no value: set one in the Cue sheet", action.kind))
    } else {
        Ok(text)
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}
/// "12" or "12.5"
fn number(s: &str) -> bool {
    let mut parts = s.splitn(2, '.');
    digits(parts.next().unwrap_or("")) && parts.next().map_or(true, digits)
}
fn safe_address(s: &str) -> bool {
    s.starts_with('/') && s.len() <= 200 && s.chars().all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c))
}

/// The messages for `action` on a device of type `device_type`; `None` when that type has no profile
/// (the generic `/pchi/...` message is used then). Errors say what to fix.
pub fn encode(device_type: &str, action: &Action) -> Option<Result<Vec<Vec<u8>>, String>> {
    if !PROFILES.contains(&device_type) {
        return None;
    }
    Some(match device_type {
        "resolume" => resolume(action),
        "grandma3" => grandma3(action),
        "eos" => eos(action),
        "x32" => x32(action),
        "qlab" => qlab(action),
        "magicq" => magicq(action),
        _ => beyond(action),
    })
}

fn unmapped(tool: &str, action: &Action) -> Result<Vec<Vec<u8>>, String> {
    Err(format!("{tool} has no mapping for \"{}\"", action.kind))
}

fn resolume(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    if a.kind != "scene.activate" {
        return unmapped("Resolume", a);
    }
    let v = value(a)?;
    let addr = if v.starts_with('/') {
        if !safe_address(&v) {
            return Err(format!("\"{v}\" is not a usable OSC address"));
        }
        v
    } else if digits(&v) {
        format!("/composition/columns/{v}/connect")
    } else if let Some((l, c)) = v.split_once('/').filter(|(l, c)| digits(l) && digits(c)) {
        format!("/composition/layers/{l}/clips/{c}/connect")
    } else {
        return Err(format!("\"{v}\": use a column number (3), layer/clip (2/5), or an address from Resolume's OSC Shortcut mode (/composition/...)"));
    };
    Ok(vec![pack(&addr, vec![OscType::Int(1)])?])
}

fn grandma3(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    if a.kind != "lighting.look" {
        return unmapped("grandMA3", a);
    }
    let v = value(a)?;
    let cmd = if digits(&v) {
        format!("Go+ Exec {v}")
    } else {
        let first = v.split_whitespace().next().unwrap_or("").to_lowercase();
        if v.len() > 200 || v.contains(';') || v.contains('\n') || !MA_ALLOWED.contains(&first.as_str()) {
            return Err(format!("\"{v}\": only playback commands may be sent to grandMA3 ({}), one at a time", MA_ALLOWED.join(", ")));
        }
        v
    };
    Ok(vec![pack("/cmd", vec![OscType::String(cmd)])?])
}

fn eos(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    if a.kind != "lighting.look" {
        return unmapped("ETC Eos", a);
    }
    let v = value(a)?;
    let addr = if number(&v) {
        format!("/eos/cue/{v}/fire")
    } else if let Some((l, c)) = v.split_once('/').filter(|(l, c)| digits(l) && number(c)) {
        format!("/eos/cue/{l}/{c}/fire")
    } else {
        return Err(format!("\"{v}\": use a cue number (2 or 2.5), or cuelist/cue (3/9)"));
    };
    Ok(vec![pack(&addr, vec![])?])
}

fn x32(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    if a.kind != "audio.snapshot" && a.kind != "audio.scene" {
        return unmapped("X32", a);
    }
    let v = value(a)?;
    match v.parse::<i32>() {
        Ok(n) if digits(&v) && n <= 99 => Ok(vec![pack("/-action/goscene", vec![OscType::Int(n)])?]),
        _ => Err(format!("\"{v}\": use the scene number on the desk (0 to 99)")),
    }
}

/// QLab cue numbers are free text ("12", "12.5", "A1"): letters, digits and . _ - only.
fn qlab_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn qlab(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    // QLab can run audio or video, so either kind may be sent to it
    if !(a.kind.starts_with("audio.") || a.kind == "scene.activate" || a.kind == "renderer.activate") {
        return unmapped("QLab", a);
    }
    let v = value(a)?;
    match v.to_lowercase().as_str() {
        w @ ("stop" | "panic" | "pause" | "resume") => Ok(vec![pack(&format!("/{w}"), vec![])?]),
        _ if qlab_id(&v) => Ok(vec![pack(&format!("/cue/{v}/start"), vec![])?]),
        _ => Err(format!("\"{v}\": use a QLab cue number (12, 12.5, A1) or stop, panic, pause, resume")),
    }
}

fn magicq(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    if a.kind != "lighting.look" {
        return unmapped("MagicQ", a);
    }
    let v = value(a)?;
    let addr = if digits(&v) {
        format!("/pb/{v}/go")
    } else if let Some((p, c)) = v.split_once('/').filter(|(p, c)| digits(p) && digits(c)) {
        format!("/pb/{p}/{c}")
    } else if let Some(p) = v.to_lowercase().strip_prefix("release ").map(str::trim).filter(|p| digits(p)) {
        format!("/pb/{p}/release")
    } else {
        return Err(format!("\"{v}\": use a playback number (3 = go), playback/cue (3/5 = jump to cue 5), or \"release 3\""));
    };
    Ok(vec![pack(&addr, vec![])?])
}

fn beyond(a: &Action) -> Result<Vec<Vec<u8>>, String> {
    match a.kind.as_str() {
        "laser.cue" | "laser.stop" => {
            let v = value(a)?;
            if v.len() > 64 || v.chars().any(|c| c.is_control()) {
                return Err("the cue name is too long or has control characters".into());
            }
            let addr = if a.kind == "laser.cue" { "/beyond/general/StartCue" } else { "/beyond/general/StopCue" };
            Ok(vec![pack(addr, vec![OscType::String(v)])?])
        }
        "laser.blackout" => Ok(vec![pack("/beyond/general/BlackOut", vec![])?]),
        _ => unmapped("BEYOND", a),
    }
}

/// A harmless message that makes this kind of tool answer, so the runtime can tell it is listening.
/// Only where the vendor's own material shows a reply: the X32 answers `/info`. Others return `None`:
/// for those the link shows only what the tool sends on its own (see `links`).
pub fn probe(device_type: &str) -> Option<Vec<u8>> {
    match device_type {
        "x32" => pack("/info", vec![]).ok(),
        _ => None,
    }
}

/// What a hard stop sends to a device of this type, so the output actually goes dark.
/// Empty when no safe message is known (the tool's own stop must be used).
pub fn stop_messages(device_type: &str) -> Vec<Vec<u8>> {
    if device_type == "beyond" {
        ["/beyond/general/BlackOut", "/beyond/general/DisableLaserOutput"].iter().filter_map(|a| pack(a, vec![]).ok()).collect()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(t: &str, kind: &str, payload: Value) -> Result<(String, Vec<OscType>), String> {
        let r = encode(t, &Action::new(kind, payload)).expect("has a profile")?;
        assert_eq!(r.len(), 1);
        match rosc::decoder::decode_udp(&r[0]).unwrap().1 {
            OscPacket::Message(m) => Ok((m.addr, m.args)),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn only_the_listed_tools_have_profiles() {
        for t in PROFILES {
            assert!(encode(t, &Action::new("x", json!({}))).is_some());
        }
        assert!(encode("laser", &Action::new("laser.cue", json!({"cue": "a"}))).is_none());
        assert!(encode("notch", &Action::new("scene.activate", json!({"scene": "a"}))).is_none());
    }

    #[test]
    fn resolume_columns_clips_and_learned_addresses() {
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "3"})).unwrap(), ("/composition/columns/3/connect".into(), vec![OscType::Int(1)]));
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "2/5"})).unwrap(), ("/composition/layers/2/clips/5/connect".into(), vec![OscType::Int(1)]));
        assert_eq!(one("resolume", "scene.activate", json!({"scene": "/composition/layers/1/clips/1/connect"})).unwrap().0, "/composition/layers/1/clips/1/connect");
        assert!(one("resolume", "scene.activate", json!({"scene": "tunnel"})).unwrap_err().contains("column number"));
        assert!(one("resolume", "scene.activate", json!({"scene": "/bad address"})).is_err());
        assert!(one("resolume", "lighting.look", json!({"look": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn grandma3_sends_playback_commands_only() {
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "402"})).unwrap(), ("/cmd".into(), vec![OscType::String("Go+ Exec 402".into())]));
        assert_eq!(one("grandma3", "lighting.look", json!({"look": 12})).unwrap().1, vec![OscType::String("Go+ Exec 12".into())]);
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "Goto Cue 5"})).unwrap().1, vec![OscType::String("Goto Cue 5".into())]);
        assert_eq!(one("grandma3", "lighting.look", json!({"look": "go+ sequence 1"})).unwrap().1, vec![OscType::String("go+ sequence 1".into())]);
        for bad in ["Delete Cue 1", "Store Cue 1", "Clear", "Go+ Exec 1; Delete Cue 1", "Go+ Exec 1\nDelete Cue 1", "blue"] {
            assert!(one("grandma3", "lighting.look", json!({"look": bad})).unwrap_err().contains("only playback commands"), "{bad}");
        }
    }

    #[test]
    fn eos_fires_cues_with_or_without_a_cuelist() {
        assert_eq!(one("eos", "lighting.look", json!({"look": "2"})).unwrap(), ("/eos/cue/2/fire".into(), vec![]));
        assert_eq!(one("eos", "lighting.look", json!({"look": "2.5"})).unwrap().0, "/eos/cue/2.5/fire");
        assert_eq!(one("eos", "lighting.look", json!({"look": "3/9"})).unwrap().0, "/eos/cue/3/9/fire");
        assert!(one("eos", "lighting.look", json!({"look": "warm"})).unwrap_err().contains("cue number"));
    }

    #[test]
    fn x32_recalls_a_scene_by_number() {
        assert_eq!(one("x32", "audio.snapshot", json!({"snapshot": "12"})).unwrap(), ("/-action/goscene".into(), vec![OscType::Int(12)]));
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "100"})).is_err());
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "intro"})).is_err());
        assert!(one("x32", "audio.snapshot", json!({"snapshot": "-1"})).is_err());
    }

    #[test]
    fn qlab_starts_cues_by_number_and_takes_the_workspace_commands() {
        assert_eq!(one("qlab", "audio.snapshot", json!({"snapshot": "12"})).unwrap(), ("/cue/12/start".into(), vec![]));
        assert_eq!(one("qlab", "audio.scene", json!({"scene": "12.5"})).unwrap().0, "/cue/12.5/start");
        assert_eq!(one("qlab", "scene.activate", json!({"scene": "A1"})).unwrap().0, "/cue/A1/start");
        for w in ["stop", "panic", "pause", "resume"] {
            assert_eq!(one("qlab", "audio.snapshot", json!({"snapshot": w.to_uppercase()})).unwrap(), (format!("/{w}"), vec![]));
        }
        assert!(one("qlab", "audio.snapshot", json!({"snapshot": "bad cue/../x"})).is_err());
        assert!(one("qlab", "lighting.look", json!({"look": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn magicq_goes_jumps_and_releases_playbacks() {
        assert_eq!(one("magicq", "lighting.look", json!({"look": "3"})).unwrap(), ("/pb/3/go".into(), vec![]));
        assert_eq!(one("magicq", "lighting.look", json!({"look": "3/5"})).unwrap().0, "/pb/3/5");
        assert_eq!(one("magicq", "lighting.look", json!({"look": "Release 3"})).unwrap().0, "/pb/3/release");
        assert!(one("magicq", "lighting.look", json!({"look": "warm"})).unwrap_err().contains("playback number"));
        assert!(one("magicq", "lighting.look", json!({"look": "release x"})).is_err());
        assert!(one("magicq", "audio.snapshot", json!({"snapshot": "1"})).unwrap_err().contains("no mapping"));
    }

    #[test]
    fn beyond_starts_and_stops_cues_and_blacks_out() {
        assert_eq!(one("beyond", "laser.cue", json!({"cue": "sweep"})).unwrap(), ("/beyond/general/StartCue".into(), vec![OscType::String("sweep".into())]));
        assert_eq!(one("beyond", "laser.stop", json!({"cue": "sweep"})).unwrap().0, "/beyond/general/StopCue");
        assert_eq!(one("beyond", "laser.blackout", json!({})).unwrap(), ("/beyond/general/BlackOut".into(), vec![]));
        assert!(one("beyond", "laser.cue", json!({"cue": ""})).unwrap_err().contains("no value"));
        assert!(one("beyond", "audio.snapshot", json!({"snapshot": "1"})).unwrap_err().contains("no mapping"));
        let stop = stop_messages("beyond");
        assert_eq!(stop.len(), 2);
        assert!(stop_messages("laser").is_empty());
    }
}
