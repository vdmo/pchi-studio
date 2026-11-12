//! PCHI Showchart — a show is a state machine.
//!
//! The engine is pure: `step(chart, state, event)` returns the next state and
//! the [`Action`]s to emit. It never calls a renderer. Actions become PCHI
//! events, adapters (Strobe, Vork, Ghost...) turn those into tool commands.
//!
//! ```text
//! State -> Action -> PCHI event -> Adapter -> Renderer
//! ```

pub mod action;
pub mod engine;
pub mod guard;
pub mod runtime;
pub mod showchart;
pub mod simulate;
pub mod state;
pub mod transition;

pub use action::Action;
pub use engine::{step, ShowEngine, Step, TransitionResult};
pub use guard::Context;
pub use runtime::{RecordingRuntime, Runtime};
pub use showchart::{Showchart, ValidationError};
pub use state::State;
pub use transition::{timer_event, Transition};
