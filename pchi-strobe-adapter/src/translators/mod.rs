//! PCHI event -> OSC translation.

pub mod cue;
pub mod playlist;
pub mod router;
pub mod scene;

use crate::pchi::events::PchiEvent;
use rosc::OscMessage;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TranslateError {
    #[error("event '{event}' is missing required argument '{arg}'")]
    MissingArg { event: String, arg: &'static str },
    #[error("no translator supports event '{0}'")]
    Unsupported(String),
}

pub trait Translator: Send + Sync {
    fn supports(&self, event_type: &str) -> bool;
    fn translate(&self, event: &PchiEvent) -> Result<Vec<OscMessage>, TranslateError>;
}

pub(crate) fn require_str<'a>(e: &'a PchiEvent, arg: &'static str) -> Result<&'a str, TranslateError> {
    e.str_arg(arg)
        .ok_or_else(|| TranslateError::MissingArg { event: e.event_type.clone(), arg })
}

pub(crate) fn require_int(e: &PchiEvent, arg: &'static str) -> Result<i32, TranslateError> {
    e.int_arg(arg)
        .ok_or_else(|| TranslateError::MissingArg { event: e.event_type.clone(), arg })
}
