//! PCHI Conductor - Central hub for scene state management

use crate::governance::GovernanceLog;
use crate::{ConductorError, RuleEngine, SceneState, TransportLayer};
use pchi_schema::{MessageType, PCHIMessage, Payload};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// PCHI Conductor - Central state management hub
pub struct PCHIConductor {
    /// Current scene state
    state: Arc<RwLock<SceneState>>,

    /// Rule engine for Only-Lang rules
    rule_engine: RuleEngine,

    /// Signed, hash-chained log of governance receipts (every rule firing)
    governance: Arc<GovernanceLog>,

    /// Transport layer for sending/receiving messages
    transport: TransportLayer,

    /// Equilibrium threshold
    equilibrium_threshold: f64,
}

impl PCHIConductor {
    /// Create a new PCHI Conductor with the bundled default rule set and
    /// governance log/signing key at their default paths
    /// (`pchi_governance_log.jsonl`, `pchi_governance_key.hex` in the
    /// working directory).
    pub fn new(equilibrium_threshold: f64) -> Self {
        Self::with_governance_paths(
            equilibrium_threshold,
            "pchi_governance_key.hex",
            "pchi_governance_log.jsonl",
        )
        .expect("opening the default governance log/signing key")
    }

    /// Create a conductor with the bundled default rules but explicit
    /// governance file paths — used by tests and by anyone who wants the
    /// signed log somewhere other than the working directory.
    pub fn with_governance_paths(
        equilibrium_threshold: f64,
        signing_key_path: &str,
        governance_log_path: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            state: Arc::new(RwLock::new(SceneState::new())),
            rule_engine: RuleEngine::new(),
            governance: Arc::new(GovernanceLog::open(signing_key_path, governance_log_path)?),
            transport: TransportLayer::new(),
            equilibrium_threshold,
        })
    }

    /// Create a conductor loading rules from a `.only-pchi` file rather
    /// than the bundled default set.
    pub fn with_rules_file(
        equilibrium_threshold: f64,
        rules_path: &str,
        signing_key_path: &str,
        governance_log_path: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            state: Arc::new(RwLock::new(SceneState::new())),
            rule_engine: RuleEngine::load_from_file(rules_path)?,
            governance: Arc::new(GovernanceLog::open(signing_key_path, governance_log_path)?),
            transport: TransportLayer::new(),
            equilibrium_threshold,
        })
    }

    /// Process an incoming PCHI message
    pub async fn process_message(&self, message: PCHIMessage) -> Result<(), ConductorError> {
        // Validate the message
        message.validate()?;

        // Check equilibrium
        if !message.is_equilibrium() {
            warn!("Message not in equilibrium: residual = {}", message.pir_invariants.residual);
        }

        // Update state based on message type
        let mut state = self.state.write().await;
        state
            .show
            .note_seen(&message.source_id, chrono::Utc::now());
        let mut show_state_announcement: Option<Vec<u8>> = None;
        let mut transition_events: Vec<PCHIMessage> = Vec::new();
        // (reason, serialized `rejected` message) when a rule refused an enter.
        let mut rejection: Option<(String, Vec<u8>)> = None;

        match message.message_type {
            MessageType::SceneUpdate => {
                if let Payload::SceneUpdate(data) = message.payload {
                    let obj_count = data.objects.len();
                    for obj in &data.objects {
                        state.update_object(obj.id.clone(), obj.transform.position);
                    }
                    info!("Updated scene with {} objects", obj_count);
                }
            }
            MessageType::ControlParameter => {
                if let Payload::ControlParameter(data) = message.payload {
                    let key = format!("{}.{}", data.target_id, data.parameter);
                    if let Some(value) = data.value.as_f64() {
                        state.set_control_parameter(key.clone(), value);
                        info!("Set control parameter: {} = {}", key, value);
                    }
                }
            }
            MessageType::MusicalContext => {
                if let Payload::MusicalContext(data) = message.payload {
                    let context = crate::state::MusicalContext {
                        bpm: data.bpm,
                        beat: data.beat,
                        kick: data.kick,
                        snare: data.snare,
                        section: data.section,
                    };
                    state.update_musical_context(context);
                    info!("Updated musical context: BPM = {}", data.bpm);
                }
            }
            MessageType::ArtistTracking => {
                if let Payload::ArtistTracking(data) = message.payload {
                    let artist_id = data.artist_id.clone();
                    let tracking = crate::state::ArtistTracking {
                        artist_id: data.artist_id,
                        position: data.transform.position,
                        velocity: data.velocity,
                    };
                    state.update_artist_tracking(tracking);
                    info!("Updated artist tracking: {}", artist_id);
                }
            }
            MessageType::Heartbeat => {
                info!("Received heartbeat from {}", message.source_id);
            }
            MessageType::GovernanceEvent => {
                // A governance decision made *elsewhere* (another
                // conductor, a DGV-fronted tool) reported onto this stage.
                // The conductor doesn't re-derive or re-sign it — that
                // would let a forwarder claim authorship of someone else's
                // decision — it just makes the event visible; a receiving
                // dashboard/log can independently verify the embedded
                // receipt_id against the origin's own governance export.
                if let Payload::GovernanceEvent(data) = &message.payload {
                    info!(
                        "GovernanceEvent from {}: {} on {} -> {} (receipt {})",
                        message.source_id, data.action, data.tool, data.gate_state, data.receipt_id
                    );
                }
            }
            MessageType::ShowState => {
                if let Payload::ShowState(data) = &message.payload {
                    let now = chrono::Utc::now();

                    // A state change is judged by the transition rules (those
                    // mentioning `show.entering`) BEFORE it is applied. A DENY
                    // leaves the show where it was.
                    if let Some(target) = state.show.transition_target(data) {
                        state.show.set_entering(Some(target.clone()));
                        let verdict = self.rule_engine.evaluate_transition(&state, &self.governance);
                        state.show.set_entering(None);
                        let (events, deny) = verdict?;
                        transition_events = events;
                        if let Some(reason) = deny {
                            warn!("Transition to {} refused: {}", target, reason);
                            state.show.record_denial(now, reason.clone());
                            let refusal = PCHIMessage::new(
                                MessageType::ShowState,
                                "pchi-conductor".to_string(),
                                Payload::ShowState(pchi_schema::ShowStateData::Rejected {
                                    state: target,
                                    reason: reason.clone(),
                                    current: state.show.current().map(str::to_string),
                                }),
                            );
                            rejection = Some((reason, serde_json::to_vec(&refusal).unwrap_or_default()));
                        }
                    }

                    if rejection.is_none() {
                        let applied = state.show.apply(data, &message.source_id, now);
                        info!("ShowState from {}: {:?} -> {:?}", message.source_id, data, applied);
                        if applied == crate::show::Applied::Changed {
                            show_state_announcement = serde_json::to_vec(&message).ok();
                        }
                    }
                }
            }
        }

        // Heartbeats carry no scene-state change — they exist purely so a
        // bridge's liveness is visible, and set nothing in `state`. Before
        // this, every message unconditionally re-evaluated all rules
        // against whatever `state` already held, so a real, still-unfixed
        // violation from an earlier message (e.g. a curl value left over
        // threshold) got re-signed into a brand-new receipt on every
        // subsequent heartbeat — every 5s by default in the bundled
        // bridges — forever, for as long as that condition sat unaddressed.
        // Found live: one real DENY produced 5 identical signed receipts
        // (same receipt_id, since nothing about the content had changed)
        // over 20 seconds of an otherwise-idle bridge. Not fixed by
        // de-duplicating receipts after the fact — fixed at the source:
        // a message that changes nothing has nothing new to evaluate.
        if matches!(message.message_type, MessageType::Heartbeat) {
            return Ok(());
        }

        // Show-state messages change nothing a rule can see, so they are
        // not evaluated (same reasoning as heartbeats above). They ARE
        // announced to WebSocket listeners when they changed something, so
        // bridges can drive their tools from the shared state. Unchanged
        // repeats are not announced, which is what keeps two bridges from
        // echoing a transition back and forth.
        if matches!(message.message_type, MessageType::ShowState) {
            drop(state);
            for event in transition_events {
                if let Ok(bytes) = serde_json::to_vec(&event) {
                    if let Err(e) = self.transport.broadcast_websocket(&bytes).await {
                        error!("Failed to broadcast governance event: {}", e);
                    }
                }
            }
            if let Some(bytes) = show_state_announcement {
                if let Err(e) = self.transport.broadcast_websocket(&bytes).await {
                    error!("Failed to broadcast show state: {}", e);
                }
            }
            if let Some((reason, bytes)) = rejection {
                if let Err(e) = self.transport.broadcast_websocket(&bytes).await {
                    error!("Failed to broadcast transition refusal: {}", e);
                }
                return Err(ConductorError::RuleError(reason));
            }
            return Ok(());
        }

        // Update equilibrium status
        state.update_equilibrium_status(self.equilibrium_threshold);

        // Apply rules. A prior version of this method swallowed rule
        // errors (`if let Err(e) = ...; error!(...); Ok(())` — always
        // returned Ok regardless of a DENY), so a caller had no way to
        // learn a denial actually happened. Every fired rule is signed
        // into the governance log before this returns, denial or not.
        let (events, deny_reason) = self.rule_engine.evaluate(&state, &self.governance)?;
        if let Some(reason) = &deny_reason {
            state.show.record_denial(chrono::Utc::now(), reason.clone());
        }
        drop(state);

        // Broadcast every event — including a DENY's — before turning a
        // denial into the error this function returns. evaluate() used to
        // return Err directly on a deny, which discarded events before
        // they were ever broadcast; see evaluate()'s doc comment.
        for event in events {
            if let Ok(bytes) = serde_json::to_vec(&event) {
                if let Err(e) = self.transport.broadcast_websocket(&bytes).await {
                    error!("Failed to broadcast governance event: {}", e);
                }
            }
        }

        if let Some(msg) = deny_reason {
            return Err(ConductorError::RuleError(msg));
        }

        Ok(())
    }

    /// Get current scene state
    pub async fn get_state(&self) -> SceneState {
        self.state.read().await.clone()
    }

    /// Get Arc reference to scene state (for web server)
    pub fn get_state_arc(&self) -> Arc<RwLock<SceneState>> {
        Arc::clone(&self.state)
    }

    /// Get Arc reference to the governance log (for web server export route)
    pub fn get_governance_arc(&self) -> Arc<GovernanceLog> {
        Arc::clone(&self.governance)
    }

    /// Get equilibrium status
    pub async fn get_equilibrium_status(&self) -> crate::state::EquilibriumStatus {
        self.state.read().await.equilibrium_status.clone()
    }

    /// Start the transport layer
    pub async fn start_transport(&mut self, bind_addr: &str) -> Result<(), ConductorError> {
        self.transport.start(bind_addr).await
    }

    /// Accept WebSocket connections on the transport layer's dedicated port
    /// (8889) and register each with `TransportLayer` so
    /// `broadcast_websocket` — the mechanism `process_message` uses to push
    /// governance events out in real time — actually has clients to send
    /// to. `start_transport` only opens the listening socket; until this
    /// ran, nothing ever called `accept_websocket`, so `ws_clients` was
    /// permanently empty and every broadcast silently went nowhere. Same
    /// bug shape as the UDP receive loop above, found the same way: by
    /// actually testing the path end-to-end instead of assuming a bound
    /// socket means a served one.
    pub async fn run_ws_accept_loop(self: Arc<Self>) {
        loop {
            match self.transport.accept_websocket().await {
                Ok((id, stream)) => {
                    self.transport.add_ws_client(id, stream).await;
                }
                Err(e) => {
                    error!("WebSocket accept loop stopping: {}", e);
                    return;
                }
            }
        }
    }

    /// Receive PCHI messages over UDP and feed them into `process_message`
    /// in a loop until the socket errors or is stopped. `start_transport`
    /// only opens the sockets — until this ran, nothing anything sent to
    /// `bind_addr` (the Resolume/TouchDesigner/Ableton bridges' actual
    /// send target) was ever validated, governed, or applied. Requires
    /// `Arc<Self>` so it can be `tokio::spawn`ed alongside the web server.
    ///
    /// Wire format: JSON (`serde_json`), matching what `web_server.rs`'s
    /// WebSocket path already uses — `pchi.fbs` describes a FlatBuffers
    /// schema, but nothing in this crate actually serializes to it at
    /// runtime, so claiming binary framing here would be another
    /// documented-but-not-built gap.
    pub async fn run_udp_receive_loop(self: Arc<Self>) {
        let mut buf = vec![0u8; 65536];
        loop {
            let (len, addr) = match self.transport.receive(&mut buf).await {
                Ok(v) => v,
                Err(e) => {
                    error!("UDP receive loop stopping: {}", e);
                    return;
                }
            };
            match serde_json::from_slice::<PCHIMessage>(&buf[..len]) {
                Ok(message) => {
                    if let Err(e) = self.process_message(message).await {
                        warn!("process_message error from {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    warn!("Dropping unparseable UDP packet from {}: {}", addr, e);
                }
            }
        }
    }

    /// Stop the transport layer
    pub async fn stop_transport(&mut self) -> Result<(), ConductorError> {
        self.transport.stop().await
    }
}

impl Default for PCHIConductor {
    fn default() -> Self {
        Self::new(1e-12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pchi_schema::ControlParameterData;

    async fn test_conductor() -> (PCHIConductor, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("pchi-conductor-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = PCHIConductor::with_governance_paths(
            1e-12,
            dir.join("key.hex").to_str().unwrap(),
            dir.join("log.jsonl").to_str().unwrap(),
        )
        .unwrap();
        (c, dir)
    }

    #[tokio::test]
    async fn denying_rule_surfaces_as_an_error_not_swallowed() {
        let (conductor, dir) = test_conductor().await;
        let msg = PCHIMessage::new(
            MessageType::ControlParameter,
            "test".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "tentacle_system".to_string(),
                parameter: "total_curl".to_string(),
                value: serde_json::json!(10.0),
            }),
        );
        let result = conductor.process_message(msg).await;
        assert!(result.is_err(), "a DENY-worthy state must not be swallowed as Ok");

        let export = conductor.get_governance_arc().export();
        assert!(export["count"].as_u64().unwrap() >= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn governance_event_message_type_does_not_error() {
        let (conductor, dir) = test_conductor().await;
        let msg = PCHIMessage::new(
            MessageType::GovernanceEvent,
            "other-conductor".to_string(),
            Payload::GovernanceEvent(pchi_schema::GovernanceEventData {
                agent_id: "a1".to_string(),
                tool: "strobe".to_string(),
                action: "strobe_intensity_limit".to_string(),
                gate_state: "DENY".to_string(),
                receipt_id: "rcpt_deadbeef".to_string(),
            }),
        );
        assert!(conductor.process_message(msg).await.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn governance_events_actually_reach_a_connected_ws_client() {
        // Regression test for the bug this fixes: broadcast_websocket had a
        // real client registry and real send logic, but nothing ever called
        // accept_websocket to populate it — every broadcast silently went
        // nowhere, undetected because no test exercised a real WS
        // connection end-to-end. bind_addr must end in exactly ":8888" —
        // start_transport derives the WS port via bind_addr.replace(
        // ":8888", ":8889"), a plain substring replace with no other
        // validation.
        let (mut conductor, dir) = test_conductor().await;
        conductor.start_transport("127.0.0.1:8888").await.unwrap();
        let conductor = Arc::new(conductor);
        let ws_task = tokio::spawn(conductor.clone().run_ws_accept_loop());

        // Give the accept loop a moment to actually be polling accept().
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let (ws_stream, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:8889")
            .await
            .expect("client should connect to the now-accepting WS server");
        let (_, mut read) = futures_util::StreamExt::split(ws_stream);

        // Give the server a moment to register the new client before the
        // triggering message is processed — otherwise the broadcast could
        // race the registration.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let msg = PCHIMessage::new(
            MessageType::ControlParameter,
            "test".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "tentacle_system".to_string(),
                parameter: "total_curl".to_string(),
                value: serde_json::json!(10.0),
            }),
        );
        // Expected to Err (a real DENY) — the point here is what the WS
        // client receives, not this call's return value.
        let _ = conductor.process_message(msg).await;

        let received = tokio::time::timeout(std::time::Duration::from_secs(5), futures_util::StreamExt::next(&mut read))
            .await
            .expect("timed out waiting for a governance event over the WS connection")
            .expect("stream ended without a message")
            .expect("WS read error");

        let text = received.into_text().expect("expected a text frame");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("expected valid PCHIMessage JSON");
        assert_eq!(parsed["type"], "governance_event");
        assert_eq!(parsed["payload"]["data"]["gate_state"], "DENY");

        ws_task.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn heartbeat_does_not_re_sign_a_lingering_violation() {
        let (conductor, dir) = test_conductor().await;

        // Leave a real, still-unfixed violation in scene state.
        let violating = PCHIMessage::new(
            MessageType::ControlParameter,
            "test".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "tentacle_system".to_string(),
                parameter: "total_curl".to_string(),
                value: serde_json::json!(10.0),
            }),
        );
        assert!(conductor.process_message(violating).await.is_err());
        let count_after_violation = conductor.get_governance_arc().export()["count"].as_u64().unwrap();
        assert_eq!(count_after_violation, 1, "the real violation should produce exactly one receipt");

        // Five heartbeats, same unaddressed condition still sitting in state.
        for _ in 0..5 {
            let heartbeat = PCHIMessage::new(
                MessageType::Heartbeat,
                "test_bridge".to_string(),
                Payload::ControlParameter(ControlParameterData {
                    target_id: "test_bridge".to_string(),
                    parameter: "heartbeat".to_string(),
                    value: serde_json::json!(1.0),
                }),
            );
            assert!(conductor.process_message(heartbeat).await.is_ok());
        }

        let count_after_heartbeats = conductor.get_governance_arc().export()["count"].as_u64().unwrap();
        assert_eq!(
            count_after_heartbeats, count_after_violation,
            "heartbeats must not produce new receipts for a condition that hasn't changed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn with_rules_file_actually_uses_the_custom_rules_not_the_bundled_default() {
        let dir = std::env::temp_dir().join(format!("pchi-rulesfile-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // A threshold (2.0) stricter than the bundled default's (4.0) —
        // chosen so a value that the bundled rules would silently allow
        // only denies if this file is genuinely the one in effect.
        let rules_path = dir.join("strict.only-pchi");
        std::fs::write(
            &rules_path,
            "harmony(1e-12)\n\nif tentacle_system.total_curl > 2.0 then\n    deny(\"too strict for the bundled default to agree with\")\nend\n",
        )
        .unwrap();

        let conductor = PCHIConductor::with_rules_file(
            1e-12,
            rules_path.to_str().unwrap(),
            dir.join("key.hex").to_str().unwrap(),
            dir.join("log.jsonl").to_str().unwrap(),
        )
        .unwrap();

        let msg = PCHIMessage::new(
            MessageType::ControlParameter,
            "test".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "tentacle_system".to_string(),
                parameter: "total_curl".to_string(),
                value: serde_json::json!(3.0),
            }),
        );
        let result = conductor.process_message(msg).await;
        assert!(
            result.is_err(),
            "3.0 is under the bundled default's 4.0 threshold but over this file's 2.0 — \
             an error here proves the custom file is the one actually loaded"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn show_msg(source: &str, data: pchi_schema::ShowStateData) -> PCHIMessage {
        PCHIMessage::new(
            MessageType::ShowState,
            source.to_string(),
            Payload::ShowState(data),
        )
    }

    #[tokio::test]
    async fn show_state_messages_build_the_shared_picture_without_touching_governance() {
        use pchi_schema::{ReadinessStatus, ShowStateData};
        let (conductor, dir) = test_conductor().await;

        // A real violation first, so we can prove show-state messages don't
        // re-sign it the way an un-gated message would.
        let violating = PCHIMessage::new(
            MessageType::ControlParameter,
            "test".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "tentacle_system".to_string(),
                parameter: "total_curl".to_string(),
                value: serde_json::json!(10.0),
            }),
        );
        assert!(conductor.process_message(violating).await.is_err());
        let receipts = conductor.get_governance_arc().export()["count"].as_u64().unwrap();

        for data in [
            ShowStateData::Plan {
                show: Some("FW27".into()),
                states: vec!["PRESHOW".into(), "LOOK_1".into(), "FINALE".into()],
            },
            ShowStateData::Enter { state: "PRESHOW".into(), next_state: None },
            ShowStateData::Readiness {
                department: "visuals".into(),
                status: ReadinessStatus::Ready,
                detail: None,
            },
        ] {
            assert!(conductor.process_message(show_msg("strobe_bridge", data)).await.is_ok());
        }

        let snap = conductor.get_state().await.show.snapshot(chrono::Utc::now());
        assert_eq!(snap.current.as_deref(), Some("PRESHOW"));
        assert_eq!(snap.next.as_deref(), Some("LOOK_1"));
        assert_eq!(snap.departments.len(), 1);
        // Confidence reflects the earlier DENY: 100% readiness - 10% penalty.
        assert_eq!(snap.confidence.unwrap().score, 90);
        assert_eq!(
            conductor.get_governance_arc().export()["count"].as_u64().unwrap(),
            receipts,
            "show-state messages must not trigger rule evaluation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    const GATE_RULES: &str = "harmony(1e-12)\n\n\
        if show.entering == \"FINALE\" and readiness.lasers != \"ready\" then\n\
        \x20   deny(\"FINALE requires lasers ready\")\nend\n";

    fn gated_conductor() -> (PCHIConductor, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("pchi-gate-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rules = dir.join("gate.only-pchi");
        std::fs::write(&rules, GATE_RULES).unwrap();
        let c = PCHIConductor::with_rules_file(
            1e-12,
            rules.to_str().unwrap(),
            dir.join("key.hex").to_str().unwrap(),
            dir.join("log.jsonl").to_str().unwrap(),
        )
        .unwrap();
        (c, dir)
    }

    fn readiness(dept: &str, status: pchi_schema::ReadinessStatus) -> pchi_schema::ShowStateData {
        pchi_schema::ShowStateData::Readiness { department: dept.into(), status, detail: None }
    }

    fn enter(state: &str) -> pchi_schema::ShowStateData {
        pchi_schema::ShowStateData::Enter { state: state.into(), next_state: None }
    }

    #[tokio::test]
    async fn a_transition_rule_refuses_entering_a_state_until_a_department_is_ready() {
        use pchi_schema::ReadinessStatus::*;
        let (c, dir) = gated_conductor();
        assert!(c.process_message(show_msg("s", enter("PRESHOW"))).await.is_ok());

        // Lasers have never reported: "unknown" must block, not pass.
        let refused = c.process_message(show_msg("s", enter("FINALE"))).await;
        assert!(refused.is_err(), "silence is not readiness");
        let snap = c.get_state().await.show.snapshot(chrono::Utc::now());
        assert_eq!(snap.current.as_deref(), Some("PRESHOW"), "the show did not move");
        assert_eq!(snap.history.len(), 1);
        assert_eq!(snap.recent_denials.len(), 1, "a refusal is visible to Mission Control");

        // Not ready is still a no.
        c.process_message(show_msg("l", readiness("lasers", NotReady))).await.unwrap();
        assert!(c.process_message(show_msg("s", enter("FINALE"))).await.is_err());

        // Other states are untouched by the rule.
        assert!(c.process_message(show_msg("s", enter("LOOK_1"))).await.is_ok());

        // Ready: allowed.
        c.process_message(show_msg("l", readiness("lasers", Ready))).await.unwrap();
        assert!(c.process_message(show_msg("s", enter("FINALE"))).await.is_ok());
        let snap = c.get_state().await.show.snapshot(chrono::Utc::now());
        assert_eq!(snap.current.as_deref(), Some("FINALE"));

        // Each refusal is a signed, verifiable receipt.
        let export = c.get_governance_arc().export();
        assert_eq!(export["count"].as_u64().unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn transition_rules_never_fire_on_ordinary_messages() {
        use pchi_schema::ReadinessStatus::*;
        let dir = std::env::temp_dir().join(format!("pchi-gate-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // An `or`: with lasers unknown this condition is TRUE for any
        // evaluation, so it only stays quiet if transition rules are really
        // kept out of ordinary passes.
        let rules = dir.join("r.only-pchi");
        std::fs::write(
            &rules,
            "harmony(1e-12)\n\nif show.entering == \"X\" or readiness.lasers != \"ready\" then\n    deny(\"would fire\")\nend\n",
        )
        .unwrap();
        let c = PCHIConductor::with_rules_file(
            1e-12,
            rules.to_str().unwrap(),
            dir.join("key.hex").to_str().unwrap(),
            dir.join("log.jsonl").to_str().unwrap(),
        )
        .unwrap();
        let ctl = PCHIMessage::new(
            MessageType::ControlParameter,
            "t".to_string(),
            Payload::ControlParameter(ControlParameterData {
                target_id: "x".to_string(),
                parameter: "y".to_string(),
                value: serde_json::json!(0.1),
            }),
        );
        assert!(c.process_message(ctl).await.is_ok());
        c.process_message(show_msg("l", readiness("lasers", Ready))).await.unwrap();
        assert_eq!(c.get_governance_arc().export()["count"].as_u64().unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn re_entering_the_current_state_is_not_gated() {
        let (c, dir) = gated_conductor();
        // Get into FINALE legitimately first.
        c.process_message(show_msg("l", readiness("lasers", pchi_schema::ReadinessStatus::Ready)))
            .await
            .unwrap();
        c.process_message(show_msg("s", enter("FINALE"))).await.unwrap();
        // Lasers drop out; a bridge repeating the same enter must not be
        // refused (nothing is transitioning).
        c.process_message(show_msg("l", readiness("lasers", pchi_schema::ReadinessStatus::NotReady)))
            .await
            .unwrap();
        assert!(c.process_message(show_msg("s", enter("FINALE"))).await.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn blank_show_state_fields_are_rejected_at_validation() {
        use pchi_schema::ShowStateData;
        let (conductor, dir) = test_conductor().await;
        let bad = show_msg("x", ShowStateData::Enter { state: "  ".into(), next_state: None });
        assert!(conductor.process_message(bad).await.is_err());
        let bad = show_msg("x", ShowStateData::Plan { show: None, states: vec![] });
        assert!(conductor.process_message(bad).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn show_state_wire_format_is_stable() {
        // This is the exact JSON the bridges send; pin it.
        let raw = r#"{"version":"2.0.0","type":"show_state","source_id":"b","timestamp":1.0,
            "pir_invariants":{"equilibrium_check":{"residual":0.0,"precision":1e-12,"sign_sequence":[]},
            "coherence_gap":0.0,"curvature_signature":"0.0, 0.0, 0.0","residual":0.0},
            "payload":{"payloadType":"show_state","data":{"kind":"readiness","department":"visuals","status":"not_ready","detail":"waiting for Strobe"}}}"#;
        let m: PCHIMessage = serde_json::from_str(raw).expect("wire JSON must parse");
        match m.payload {
            Payload::ShowState(pchi_schema::ShowStateData::Readiness { department, status, detail }) => {
                assert_eq!(department, "visuals");
                assert_eq!(status, pchi_schema::ReadinessStatus::NotReady);
                assert_eq!(detail.as_deref(), Some("waiting for Strobe"));
            }
            other => panic!("wrong payload: {other:?}"),
        }
    }

    #[tokio::test]
    async fn with_rules_file_rejects_a_missing_path_instead_of_silently_falling_back() {
        let dir = std::env::temp_dir().join(format!("pchi-rulesfile-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let result = PCHIConductor::with_rules_file(
            1e-12,
            dir.join("does-not-exist.only-pchi").to_str().unwrap(),
            dir.join("key.hex").to_str().unwrap(),
            dir.join("log.jsonl").to_str().unwrap(),
        );
        assert!(result.is_err(), "a missing rules file must be a startup error, not a silent fallback");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
