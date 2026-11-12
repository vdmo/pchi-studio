use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// A side effect requested by the chart, e.g. `scene.activate {scene: "portal"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
}

impl Action {
    pub fn new(kind: &str, payload: Value) -> Self {
        Self { kind: kind.into(), payload }
    }

    /// The PCHI event for this action, in the flat shape Studio and the
    /// adapters accept: `{"type": kind, ...payload fields}`.
    pub fn to_event(&self) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), json!(self.kind));
        if let Value::Object(p) = &self.payload {
            for (k, v) in p {
                if k != "type" {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        Value::Object(m)
    }
}
