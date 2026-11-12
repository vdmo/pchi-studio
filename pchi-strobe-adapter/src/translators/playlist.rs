//! Playlist and transport events. Maps onto Strobe's `PlaylistClock`
//! commands (Next / Previous / JumpTo / Hold / ToggleBlackout) as described
//! in the integration spec.

use super::{require_int, require_str, TranslateError, Translator};
use crate::config::settings::Addresses;
use crate::pchi::events::PchiEvent;
use crate::strobe::commands::{bare, int_msg, str_msg};
use rosc::OscMessage;

pub struct PlaylistTranslator {
    pub addrs: Addresses,
}

impl Translator for PlaylistTranslator {
    fn supports(&self, t: &str) -> bool {
        matches!(
            t,
            "playlist.play"
                | "playlist.pause"
                | "playlist.stop"
                | "playlist.jump"
                | "playlist.next"
                | "playlist.previous"
                | "transport.hold"
                | "transport.blackout"
        )
    }

    fn translate(&self, e: &PchiEvent) -> Result<Vec<OscMessage>, TranslateError> {
        let a = &self.addrs;
        let m = match e.event_type.as_str() {
            "playlist.play" => str_msg(&a.playlist_play, require_str(e, "playlist")?),
            "playlist.jump" => int_msg(&a.playlist_jump, require_int(e, "cue")?),
            "playlist.pause" => bare(&a.playlist_pause),
            "playlist.stop" => bare(&a.playlist_stop),
            "playlist.next" => bare(&a.playlist_next),
            "playlist.previous" => bare(&a.playlist_previous),
            "transport.hold" => bare(&a.transport_hold),
            "transport.blackout" => bare(&a.transport_blackout),
            other => return Err(TranslateError::Unsupported(other.into())),
        };
        Ok(vec![m])
    }
}
