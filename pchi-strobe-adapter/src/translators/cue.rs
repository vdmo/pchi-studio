use super::{require_str, TranslateError, Translator};
use crate::pchi::events::PchiEvent;
use crate::strobe::commands::str_msg;
use rosc::OscMessage;

pub struct CueTranslator {
    pub trigger: String,
}

impl Translator for CueTranslator {
    fn supports(&self, t: &str) -> bool {
        t == "cue.trigger"
    }
    fn translate(&self, e: &PchiEvent) -> Result<Vec<OscMessage>, TranslateError> {
        Ok(vec![str_msg(&self.trigger, require_str(e, "cue")?)])
    }
}
