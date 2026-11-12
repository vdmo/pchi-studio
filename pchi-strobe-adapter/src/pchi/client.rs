//! Publishing to the PCHI conductor (raw JSON over UDP, like the other bridges).

use crate::strobe::telemetry::Telemetry;
use pchi_schema::{ControlParameterData, MessageType, PCHIMessage, Payload};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

pub struct ConductorClient {
    socket: UdpSocket,
    target: SocketAddr,
}

impl ConductorClient {
    pub async fn new(target: &str) -> anyhow::Result<Self> {
        Ok(Self { socket: UdpSocket::bind("127.0.0.1:0").await?, target: target.parse()? })
    }

    /// One `control_parameter` message per telemetry field.
    pub fn telemetry_messages(t: &Telemetry) -> Vec<PCHIMessage> {
        t.fields()
            .into_iter()
            .map(|(parameter, value)| {
                PCHIMessage::new(
                    MessageType::ControlParameter,
                    t.node_id.clone(),
                    Payload::ControlParameter(ControlParameterData {
                        target_id: t.node_id.clone(),
                        parameter: parameter.to_string(),
                        value,
                    }),
                )
            })
            .collect()
    }

    pub async fn publish(&self, t: &Telemetry) -> anyhow::Result<()> {
        for m in Self::telemetry_messages(t) {
            self.socket.send_to(&serde_json::to_vec(&m)?, self.target).await?;
        }
        Ok(())
    }
}
