use super::{cue::CueTranslator, playlist::PlaylistTranslator, scene::SceneTranslator};
use super::{TranslateError, Translator};
use crate::config::settings::Addresses;
use crate::pchi::events::PchiEvent;
use rosc::OscMessage;

pub struct Router {
    translators: Vec<Box<dyn Translator>>,
}

impl Router {
    pub fn new(addrs: &Addresses) -> Self {
        Self {
            translators: vec![
                Box::new(SceneTranslator { load: addrs.scene_load.clone() }),
                Box::new(CueTranslator { trigger: addrs.cue_trigger.clone() }),
                Box::new(PlaylistTranslator { addrs: addrs.clone() }),
            ],
        }
    }

    pub fn route(&self, e: &PchiEvent) -> Result<Vec<OscMessage>, TranslateError> {
        self.translators
            .iter()
            .find(|t| t.supports(&e.event_type))
            .ok_or_else(|| TranslateError::Unsupported(e.event_type.clone()))?
            .translate(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;
    use serde_json::json;

    fn router() -> Router {
        Router::new(&Addresses::default())
    }

    #[test]
    fn scene_activate_nested_payload() {
        let e = PchiEvent::new("scene.activate", json!({"scene": "nebula"}));
        let out = router().route(&e).unwrap();
        assert_eq!(out[0].addr, "/strobe/scene/load");
        assert_eq!(out[0].args, vec![OscType::String("nebula".into())]);
    }

    #[test]
    fn scene_activate_flat_form_from_studio() {
        let e = PchiEvent::parse(br#"{"type":"scene.activate","scene":"nebula","node":"scene-1"}"#).unwrap();
        assert_eq!(router().route(&e).unwrap()[0].addr, "/strobe/scene/load");
    }

    #[test]
    fn cue_playlist_and_jump() {
        let r = router();
        let cue = r.route(&PchiEvent::new("cue.trigger", json!({"cue": "portal-open"}))).unwrap();
        assert_eq!(cue[0].addr, "/strobe/cue/trigger");
        let play = r.route(&PchiEvent::new("playlist.play", json!({"playlist": "festival-main"}))).unwrap();
        assert_eq!(play[0].args, vec![OscType::String("festival-main".into())]);
        let jump = r.route(&PchiEvent::new("playlist.jump", json!({"cue": 12}))).unwrap();
        assert_eq!((jump[0].addr.as_str(), &jump[0].args), ("/strobe/playlist/jump", &vec![OscType::Int(12)]));
    }

    #[test]
    fn missing_argument_and_unknown_event_are_errors() {
        let r = router();
        assert_eq!(
            r.route(&PchiEvent::new("scene.activate", json!({}))).unwrap_err(),
            TranslateError::MissingArg { event: "scene.activate".into(), arg: "scene" }
        );
        assert_eq!(
            r.route(&PchiEvent::new("graph.update", json!({}))).unwrap_err(),
            TranslateError::Unsupported("graph.update".into())
        );
    }

    #[test]
    fn osc_encoding_roundtrips() {
        let m = router().route(&PchiEvent::new("playlist.jump", json!({"cue": 3}))).unwrap().remove(0);
        let bytes = crate::strobe::osc::encode(&m).unwrap();
        let (_, pkt) = rosc::decoder::decode_udp(&bytes).unwrap();
        assert_eq!(pkt, rosc::OscPacket::Message(m));
    }
}
