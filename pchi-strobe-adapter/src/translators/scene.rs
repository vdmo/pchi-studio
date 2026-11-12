use super::{require_str, TranslateError, Translator};
use crate::pchi::events::PchiEvent;
use crate::strobe::commands::str_msg;
use rosc::OscMessage;

pub struct SceneTranslator {
    pub load: String,
}

impl Translator for SceneTranslator {
    fn supports(&self, t: &str) -> bool {
        t == "scene.activate"
    }
    fn translate(&self, e: &PchiEvent) -> Result<Vec<OscMessage>, TranslateError> {
        Ok(vec![str_msg(&self.load, require_str(e, "scene")?)])
    }
}
