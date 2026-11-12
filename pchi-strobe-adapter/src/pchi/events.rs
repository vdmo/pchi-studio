//! Inbound PCHI events.
//!
//! Accepts both shapes seen in the project: arguments nested under
//! `payload` (`{"type":"scene.activate","payload":{"scene":"nebula"}}`) and
//! flat, as PCHI Studio emits (`{"type":"scene.activate","scene":"nebula"}`).

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct PchiEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl PchiEvent {
    pub fn new(event_type: &str, payload: Value) -> Self {
        Self { event_type: event_type.into(), payload, rest: Map::new() }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    fn arg(&self, key: &str) -> Option<&Value> {
        self.payload.get(key).or_else(|| self.rest.get(key))
    }

    pub fn str_arg(&self, key: &str) -> Option<&str> {
        self.arg(key).and_then(Value::as_str)
    }

    pub fn int_arg(&self, key: &str) -> Option<i32> {
        self.arg(key).and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok())
    }
}
