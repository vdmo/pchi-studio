//! Node registration manifest and capability profile (`render.vj`).

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NodeManifest {
    pub node_id: String,
    pub name: &'static str,
    pub node_type: &'static str,
    pub capabilities: Vec<&'static str>,
}

pub const CAPABILITIES: &[&str] = &[
    "render.vj",
    "scene.activate",
    "cue.trigger",
    "playlist.play",
    "playlist.pause",
    "playlist.stop",
    "playlist.jump",
    "playlist.next",
    "playlist.previous",
    "transport.control",
    "osc.control",
    "telemetry.publish",
];

pub fn manifest(node_id: &str) -> NodeManifest {
    NodeManifest {
        node_id: node_id.to_string(),
        name: "Strobe VJ Engine",
        node_type: "renderer",
        capabilities: CAPABILITIES.to_vec(),
    }
}
