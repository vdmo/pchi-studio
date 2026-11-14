//! The doors check: everything that can be checked before the show, in one list.
//!
//! Each line is "ok", "warn" (look at it) or "fail" (this will not work). It checks the show against
//! the rig AND against each tool's profile: a cue whose value the tool cannot use is a "fail" here,
//! not a surprise at 21:07. It never blocks anything; it tells the operator the truth.

use crate::output::Output;
use crate::profiles;
use crate::rig::{dept_of_kind, resolve};
use pchi_showchart::{Action, Showchart};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct Check {
    /// "ok" | "warn" | "fail"
    pub level: &'static str,
    /// What was checked: "output", "device", "cue", "scene", "laser", "ready".
    pub area: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
}

fn c(level: &'static str, area: &'static str, text: impl Into<String>) -> Check {
    Check { level, area, text: text.into(), device: None, scene: None }
}

/// All checks for `chart` against `output` (None: the show was loaded without a rig).
/// `readiness` is what each department or device has said, `state` the scene that is running.
pub fn run(chart: &Showchart, output: Option<&Output>, readiness: &BTreeMap<String, Value>, state: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let Some(o) = output else {
        out.push(c("fail", "output", "No rig was loaded: the runtime has nothing to send cues to. Save the show from Studio (with its rig) and start the runtime on that file."));
        return out;
    };
    if o.sending() {
        out.push(c("ok", "output", "Sending is ON: cues really go to the devices."));
    } else {
        out.push(c("warn", "output", "Dry run: nothing is sent. Start the runtime with --send to go live."));
    }

    // ── devices ──
    for d in o.devices() {
        let mut dev = |level: &'static str, area: &'static str, text: String| {
            let mut x = c(level, area, text);
            x.device = Some(d.id.clone());
            out.push(x);
        };
        match d.address() {
            None => dev("fail", "device", format!("{} has no host and port: set them in the Rig tab.", d.label)),
            Some(a) if d.host.as_deref() == Some("127.0.0.1") || d.host.as_deref() == Some("localhost") => {
                dev("warn", "device", format!("{} is at {a}, which is the computer running the runtime. Right only if the tool runs there too.", d.label))
            }
            Some(a) => dev("ok", "device", format!("{} is at {a}.", d.label)),
        }
        if let Some(l) = o.link_state(d) {
            let state_word = l["state"].as_str().unwrap_or("none");
            let age = l["heard_ms_ago"].as_i64().map(|ms| format!(" (last heard {} s ago)", ms / 1000)).unwrap_or_default();
            match state_word {
                "linked" => dev("ok", "device", format!("{} answers: linked{age}.", d.label)),
                "lost" => dev("fail", "device", format!("{} stopped answering{age}.", d.label)),
                "silent" if l["probed"] == true => dev("fail", "device", format!("{} has not answered the runtime at all yet.", d.label)),
                "silent" => dev("warn", "device", format!("{} is expected to send feedback to the listen port and has not yet.", d.label)),
                "heard" => dev("ok", "device", format!("{} has sent feedback{age}.", d.label)),
                _ => dev("warn", "device", format!("{} gives no feedback: only its own READY or a test can show it is there.", d.label)),
            }
        }
        // what the tool's messages rest on: only the vendor's own documentation is a firm basis
        if let Some(p) = profiles::get(d.profile_key()) {
            let weak: Vec<&profiles::Rule> = p.rules.iter().filter(|r| r.basis != profiles::Basis::Vendor).collect();
            if !weak.is_empty() {
                let mut words: Vec<&str> = weak.iter().map(|r| r.basis.word()).collect();
                words.sort();
                words.dedup();
                dev("warn", "device", format!("{}: {} of {} message rule(s) in the {} profile rest on {} rather than the vendor's documentation: prove them with pchi-gearcheck before relying on them.", d.label, weak.len(), p.rules.len(), p.name, words.join(" and ")));
            }
        }
        if d.dept == "laser" {
            if o.is_armed(&d.id) {
                dev("warn", "laser", format!("{} is ARMED: cues will reach it.", d.label));
            } else {
                dev("ok", "laser", format!("{} is disarmed (arm it on the Live tab when the area is clear).", d.label));
            }
        }
        match readiness.get(&d.id).or_else(|| readiness.get(&d.dept)) {
            Some(w) if w["ready"] == true => dev("ok", "ready", format!("{} reports READY.", d.label)),
            Some(_) => dev("warn", "ready", format!("{} is on hold.", d.label)),
            None => dev("warn", "ready", format!("{} has not reported READY.", d.label)),
        }
    }

    // ── every cue and every scene action against the device and profile that will receive it ──
    let mut cues = 0usize;
    let mut bad = 0usize;
    for s in &chart.states {
        let mut push = |level: &'static str, text: String, dev: Option<String>| {
            let mut x = c(level, "cue", text);
            x.scene = Some(s.id.clone());
            x.device = dev;
            out.push(x);
        };
        let check_action = |action: &Action, dept: Option<&str>, device: Option<&str>, what: String, bad: &mut usize, cues: &mut usize| -> Option<(&'static str, String, Option<String>)> {
            let dept = dept.filter(|x| !x.is_empty()).unwrap_or_else(|| dept_of_kind(&action.kind));
            if dept == "other" {
                return None;
            }
            *cues += 1;
            let dev = match resolve(o.devices(), dept, device) {
                Ok(d) => d,
                Err(e) => {
                    *bad += 1;
                    return Some(("fail", format!("{what}: {e}."), None));
                }
            };
            // a blank value is never what anyone wanted
            if action.payload.as_object().is_some_and(|p| !p.is_empty() && p.values().all(|v| matches!(v, Value::String(x) if x.trim().is_empty()))) {
                *bad += 1;
                return Some(("fail", format!("{what}: the value is blank (set it in the Cue sheet)."), Some(dev.id.clone())));
            }
            if dev.is_dmx() {
                return match crate::dmx::describe(action) {
                    Err(e) => {
                        *bad += 1;
                        Some(("fail", format!("{what} to {}: {e}.", dev.label), Some(dev.id.clone())))
                    }
                    Ok(_) => None,
                };
            }
            match profiles::encode(dev.profile_key(), action) {
                Some(Err(e)) => {
                    *bad += 1;
                    Some(("fail", format!("{what} to {}: {e}.", dev.label), Some(dev.id.clone())))
                }
                _ => None,
            }
        };
        for (when, list) in [("on entering", &s.enter_actions), ("on leaving", &s.exit_actions)] {
            for a in list {
                if let Some((lvl, t, dv)) = check_action(a, None, None, format!("{} {when}: {}", s.id, a.kind), &mut bad, &mut cues) {
                    push(lvl, t, dv);
                }
            }
        }
        for l in s.timeline.iter().flat_map(|t| &t.lanes) {
            for q in &l.cues {
                let name = q.num.map(|n| format!("Q{n}")).unwrap_or_else(|| q.label.clone().unwrap_or_else(|| q.action.kind.clone()));
                let what = format!("{} {} at {:.1}s ({name})", s.id, if l.name.is_empty() { &l.id } else { &l.name }, q.at_ms as f64 / 1000.0);
                if let Some((lvl, t, dv)) = check_action(&q.action, l.dept.as_deref(), l.device.as_deref(), what, &mut bad, &mut cues) {
                    push(lvl, t, dv);
                }
            }
        }
    }
    if cues == 0 {
        out.push(c("warn", "scene", "The show has no cues that go to a device."));
    } else if bad == 0 {
        out.push(c("ok", "cue", format!("All {cues} cues can be sent: each has a device and a value its tool understands.")));
    }
    let here = chart.states.iter().find(|s| s.id == state);
    if let Some(s) = here {
        let mut x = c("ok", "scene", format!("The show is in {}.", s.id));
        x.scene = Some(s.id.clone());
        out.push(x);
    }
    out
}

/// "go" when nothing failed.
pub fn summary(checks: &[Check]) -> Value {
    let n = |lvl: &str| checks.iter().filter(|x| x.level == lvl).count();
    serde_json::json!({"go": n("fail") == 0, "fail": n("fail"), "warn": n("warn"), "ok": n("ok")})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{dept_of_type, Device};
    use serde_json::json;

    fn dev(id: &str, kind: &str, host: &str, port: u16) -> Device {
        Device { id: id.into(), label: id.into(), kind: kind.into(), dept: dept_of_type(kind).unwrap().into(), host: Some(host.into()), port: Some(port), listen_port: None, profile: None, universe: None }
    }
    fn chart(cue_value: &str) -> Showchart {
        Showchart::from_json(&format!(
            r#"{{"initial":"A","states":[{{"id":"A","timeline":{{"lanes":[
              {{"id":"audio","name":"Audio","dept":"audio","cues":[{{"id":"c1","at_ms":1000,"num":1,"action":{{"kind":"audio.snapshot","payload":{{"snapshot":"{cue_value}"}}}}}}]}},
              {{"id":"lights","name":"Lights","dept":"lighting","cues":[{{"id":"c2","at_ms":2000,"action":{{"kind":"lighting.look","payload":{{"look":"402"}}}}}}]}}]}}}}],"transitions":[]}}"#
        ))
        .unwrap()
    }
    fn levels(v: &[Check], area: &str) -> Vec<&'static str> {
        v.iter().filter(|x| x.area == area).map(|x| x.level).collect()
    }

    #[test]
    fn a_show_whose_cues_all_fit_their_tools_passes() {
        // (a dry-run output: link probes only run inside a live show)
        let o = Output::new(vec![dev("x1", "x32", "192.168.1.20", 10023), dev("m1", "grandma3", "192.168.1.30", 8000)], false);
        let checks = run(&chart("12"), Some(&o), &BTreeMap::new(), "A");
        assert!(checks.iter().any(|x| x.text.starts_with("All 2 cues can be sent")), "{checks:#?}");
        assert_eq!(summary(&checks)["fail"], 0);
        assert_eq!(levels(&checks, "output"), ["warn"]);
        // live and not yet heard from: an X32 that should answer a probe and has not is a failure, said plainly
        let live = Output::new(vec![dev("x1", "x32", "192.168.1.20", 10023), dev("m1", "grandma3", "192.168.1.30", 8000)], true);
        let l = run(&chart("12"), Some(&live), &BTreeMap::new(), "A");
        assert_eq!(levels(&l, "output"), ["ok"]);
        assert!(l.iter().any(|x| x.level == "fail" && x.text.contains("has not answered the runtime at all yet")));
    }

    #[test]
    fn a_value_the_tool_cannot_use_is_a_failure_with_the_reason() {
        let o = Output::new(vec![dev("x1", "x32", "192.168.1.20", 10023), dev("m1", "grandma3", "192.168.1.30", 8000)], false);
        let checks = run(&chart("intro"), Some(&o), &BTreeMap::new(), "A");
        let f: Vec<_> = checks.iter().filter(|x| x.level == "fail").collect();
        assert_eq!(f.len(), 1, "{checks:#?}");
        assert!(f[0].text.contains("scene number") && f[0].text.contains("A Audio at 1.0s (Q1)"), "{}", f[0].text);
        assert_eq!(f[0].device.as_deref(), Some("x1"));
        assert_eq!(summary(&checks)["go"], false);
    }

    #[test]
    fn missing_devices_blank_values_and_no_rig_are_failures() {
        // lighting has no device
        let o = Output::new(vec![dev("x1", "x32", "192.168.1.20", 10023)], false);
        let checks = run(&chart("12"), Some(&o), &BTreeMap::new(), "A");
        assert!(checks.iter().any(|x| x.level == "fail" && x.text.contains("no lighting device")), "{checks:#?}");
        assert!(levels(&checks, "output") == ["warn"], "a dry run is a warning");
        // blank
        let blank = run(&chart(""), Some(&Output::new(vec![dev("x1", "x32", "h", 1), dev("m1", "grandma3", "h", 2)], true)), &BTreeMap::new(), "A");
        assert!(blank.iter().any(|x| x.level == "fail" && x.text.contains("value is blank")));
        // no rig at all
        let none = run(&chart("12"), None, &BTreeMap::new(), "A");
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].level, "fail");
    }

    #[test]
    fn devices_are_checked_for_address_link_readiness_and_lasers() {
        let mut no_addr = dev("x1", "x32", "", 0);
        no_addr.host = None;
        no_addr.port = None;
        let o = Output::new(vec![no_addr, dev("l1", "beyond", "127.0.0.1", 12000)], true);
        let mut ready = BTreeMap::new();
        ready.insert("laser".to_string(), json!({"ready": false}));
        let checks = run(&chart("12"), Some(&o), &ready, "A");
        assert!(checks.iter().any(|x| x.level == "fail" && x.text.contains("no host and port")));
        assert!(checks.iter().any(|x| x.level == "warn" && x.text.contains("computer running the runtime")));
        assert!(checks.iter().any(|x| x.area == "laser" && x.level == "ok" && x.text.contains("disarmed")));
        o.set_armed("l1", true);
        let armed = run(&chart("12"), Some(&o), &ready, "A");
        assert!(armed.iter().any(|x| x.area == "laser" && x.level == "warn" && x.text.contains("ARMED")));
        assert!(armed.iter().any(|x| x.area == "ready" && x.level == "warn" && x.text.contains("on hold")));
        assert!(armed.iter().any(|x| x.area == "ready" && x.text.contains("has not reported READY")));
    }
}
