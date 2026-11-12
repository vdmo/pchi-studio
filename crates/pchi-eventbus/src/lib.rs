//! PCHI event bus: every Showchart action becomes a PCHI event published
//! here; adapters, Studio and Rune subscribe. Nothing talks to a renderer
//! directly.
//!
//! ```text
//! Showchart -> Action -> EventBus -> subscribers (adapters, Studio, Rune)
//! ```

pub mod scheduler;
pub use scheduler::run_show;

use pchi_showchart::{Action, Runtime};
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Value>,
}

impl EventBus {
    /// `capacity` is how many events a slow subscriber may lag behind before
    /// it starts missing the oldest ones (it is told how many it missed).
    pub fn new(capacity: usize) -> Self {
        Self { tx: broadcast::channel(capacity).0 }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }

    /// Publish an event. Returns how many subscribers received it
    /// (0 is fine: publishing never fails just because nobody listens).
    pub fn publish(&self, event: Value) -> usize {
        self.tx.send(event).unwrap_or(0)
    }

    pub fn publish_action(&self, action: &Action) -> usize {
        self.publish(action.to_event())
    }
}

/// Lets the showchart replay/run helpers publish straight onto the bus.
impl Runtime for EventBus {
    fn emit(&self, action: &Action) {
        self.publish_action(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pchi_showchart::{runtime::replay, ShowEngine, Showchart};
    use serde_json::json;

    #[tokio::test]
    async fn scene_activate_appears_on_the_bus() {
        let bus = EventBus::new(16);
        let mut sub = bus.subscribe();
        let chart = Showchart::from_json(include_str!("../../pchi-showchart/examples/portal.json")).unwrap();
        let mut engine = ShowEngine::new(chart).unwrap();
        replay(&mut engine, &["START", "PORTAL_OPEN"], &bus);
        assert_eq!(sub.recv().await.unwrap(), json!({"type": "scene.activate", "scene": "portal"}));
        assert_eq!(sub.recv().await.unwrap(), json!({"type": "renderer.activate", "renderer": "vork"}));
    }

    #[tokio::test]
    async fn every_subscriber_gets_every_event() {
        let bus = EventBus::new(16);
        let (mut a, mut b) = (bus.subscribe(), bus.subscribe());
        assert_eq!(bus.publish(json!({"type": "x"})), 2);
        assert_eq!(a.recv().await.unwrap(), b.recv().await.unwrap());
    }

    #[test]
    fn publishing_with_no_subscribers_is_not_an_error() {
        assert_eq!(EventBus::new(4).publish(json!({"type": "x"})), 0);
    }

    #[tokio::test]
    async fn a_slow_subscriber_is_told_it_lagged() {
        let bus = EventBus::new(2);
        let mut sub = bus.subscribe();
        for i in 0..5 {
            bus.publish(json!({"type": "e", "i": i}));
        }
        assert!(matches!(sub.recv().await, Err(broadcast::error::RecvError::Lagged(_))));
    }
}
