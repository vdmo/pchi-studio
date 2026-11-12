//! Helpers for building OSC messages for Strobe's playlist/transport commands.

use rosc::{OscMessage, OscType};

pub fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage { addr: addr.to_string(), args }
}

pub fn str_msg(addr: &str, s: &str) -> OscMessage {
    msg(addr, vec![OscType::String(s.to_string())])
}

pub fn int_msg(addr: &str, n: i32) -> OscMessage {
    msg(addr, vec![OscType::Int(n)])
}

pub fn bare(addr: &str) -> OscMessage {
    msg(addr, vec![])
}
