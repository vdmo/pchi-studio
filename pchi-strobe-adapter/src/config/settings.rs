//! Adapter settings, including the OSC address map.
//!
//! The default addresses follow the PCHI-Strobe design spec. They have NOT
//! been checked against Strobe's `strobe-io` crate; every address is
//! overridable here so they can be corrected without touching translators.

use clap::Parser;

#[derive(Debug, Clone)]
pub struct Addresses {
    pub scene_load: String,
    pub cue_trigger: String,
    pub playlist_play: String,
    pub playlist_pause: String,
    pub playlist_stop: String,
    pub playlist_jump: String,
    pub playlist_next: String,
    pub playlist_previous: String,
    pub transport_hold: String,
    pub transport_blackout: String,
    /// Prefix for inbound telemetry: `<prefix>/scene`, `/playlist`, `/fps`, `/frame`, `/status`.
    pub telemetry_prefix: String,
}

impl Default for Addresses {
    fn default() -> Self {
        Self {
            scene_load: "/strobe/scene/load".into(),
            cue_trigger: "/strobe/cue/trigger".into(),
            playlist_play: "/strobe/playlist/play".into(),
            playlist_pause: "/strobe/playlist/pause".into(),
            playlist_stop: "/strobe/playlist/stop".into(),
            playlist_jump: "/strobe/playlist/jump".into(),
            playlist_next: "/strobe/playlist/next".into(),
            playlist_previous: "/strobe/playlist/previous".into(),
            transport_hold: "/strobe/transport/hold".into(),
            transport_blackout: "/strobe/transport/blackout".into(),
            telemetry_prefix: "/strobe/telemetry".into(),
        }
    }
}

/// PCHI-Strobe adapter: PCHI events in (UDP JSON) -> OSC out to Strobe;
/// Strobe telemetry in (OSC) -> PCHI conductor.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Settings {
    /// PCHI node id this adapter registers as.
    #[arg(long, env = "PCHI_NODE_ID", default_value = "strobe-main")]
    pub node_id: String,

    /// UDP address where PCHI events (JSON) are received.
    #[arg(long, env = "PCHI_STROBE_LISTEN", default_value = "127.0.0.1:8890")]
    pub listen: String,

    /// Strobe's OSC input.
    #[arg(long, env = "STROBE_OSC", default_value = "127.0.0.1:9000")]
    pub strobe: String,

    /// UDP address where Strobe telemetry OSC is received.
    #[arg(long, env = "STROBE_TELEMETRY_LISTEN", default_value = "127.0.0.1:9100")]
    pub telemetry_listen: String,

    /// PCHI conductor UDP ingest (telemetry is published here).
    #[arg(long, env = "PCHI_CONDUCTOR", default_value = "127.0.0.1:8888")]
    pub conductor: String,

    /// Log the OSC that would be sent instead of sending it.
    #[arg(long)]
    pub dry_run: bool,

    /// Print the node registration manifest as JSON and exit.
    #[arg(long)]
    pub print_manifest: bool,

    /// Telemetry publish interval in milliseconds.
    #[arg(long, default_value_t = 1000)]
    pub telemetry_interval_ms: u64,
}
