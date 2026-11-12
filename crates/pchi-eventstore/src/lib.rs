//! SQLite event log for PCHI shows.
//!
//! `recorder` writes a run, `replay` re-runs it from the file alone and
//! checks it reproduces the recording.

pub mod recorder;
pub mod replay;
pub mod store;

pub use replay::{replay_latest, ReplayError, ReplayReport, ReplayStep};
pub use store::{EventRecord, EventStore, StoreError};
