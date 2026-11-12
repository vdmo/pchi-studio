//! Strobe telemetry: the renderer-neutral PCHI telemetry contract.
//!
//! Inbound OSC is expected as `<prefix>/scene|playlist|fps|frame|status`
//! (unverified against Strobe; configurable prefix).

use rosc::{OscMessage, OscType};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Telemetry {
    pub node_id: String,
    pub scene: String,
    pub playlist: String,
    pub fps: f64,
    pub frame: i64,
    pub status: String,
}

impl Telemetry {
    pub fn new(node_id: &str) -> Self {
        Self {
            node_id: node_id.into(),
            scene: String::new(),
            playlist: String::new(),
            fps: 0.0,
            frame: 0,
            status: "unknown".into(),
        }
    }

    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("scene", json!(self.scene)),
            ("playlist", json!(self.playlist)),
            ("fps", json!(self.fps)),
            ("frame", json!(self.frame)),
            ("status", json!(self.status)),
        ]
    }

    /// Apply one inbound OSC message. Returns true if it was a telemetry field.
    pub fn apply(&mut self, prefix: &str, m: &OscMessage) -> bool {
        let Some(field) = m.addr.strip_prefix(prefix).and_then(|s| s.strip_prefix('/')) else {
            return false;
        };
        let arg = m.args.first();
        match (field, arg) {
            ("scene", Some(OscType::String(s))) => self.scene = s.clone(),
            ("playlist", Some(OscType::String(s))) => self.playlist = s.clone(),
            ("status", Some(OscType::String(s))) => self.status = s.clone(),
            ("fps", Some(OscType::Float(f))) => self.fps = *f as f64,
            ("fps", Some(OscType::Double(f))) => self.fps = *f,
            ("fps", Some(OscType::Int(n))) => self.fps = *n as f64,
            ("frame", Some(OscType::Int(n))) => self.frame = *n as i64,
            ("frame", Some(OscType::Long(n))) => self.frame = *n,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_known_fields_and_ignores_others() {
        let mut t = Telemetry::new("strobe-main");
        let m = |a: &str, v: OscType| OscMessage { addr: a.into(), args: vec![v] };
        assert!(t.apply("/strobe/telemetry", &m("/strobe/telemetry/scene", OscType::String("nebula".into()))));
        assert!(t.apply("/strobe/telemetry", &m("/strobe/telemetry/fps", OscType::Float(60.0))));
        assert!(!t.apply("/strobe/telemetry", &m("/other/scene", OscType::String("x".into()))));
        assert!(!t.apply("/strobe/telemetry", &m("/strobe/telemetry/fps", OscType::String("bad".into()))));
        assert_eq!((t.scene.as_str(), t.fps), ("nebula", 60.0));
    }
}
