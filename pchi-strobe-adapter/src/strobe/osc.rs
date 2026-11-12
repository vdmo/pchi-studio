//! OSC transport to Strobe.

use rosc::{encoder, OscMessage, OscPacket};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

pub fn encode(m: &OscMessage) -> anyhow::Result<Vec<u8>> {
    Ok(encoder::encode(&OscPacket::Message(m.clone()))?)
}

pub struct OscSender {
    socket: UdpSocket,
    target: SocketAddr,
    dry_run: bool,
}

impl OscSender {
    pub async fn new(target: &str, dry_run: bool) -> anyhow::Result<Self> {
        Ok(Self { socket: UdpSocket::bind("0.0.0.0:0").await?, target: target.parse()?, dry_run })
    }

    pub async fn send(&self, m: &OscMessage) -> anyhow::Result<()> {
        if self.dry_run {
            tracing::info!("[dry-run] OSC {} {:?}", m.addr, m.args);
            return Ok(());
        }
        self.socket.send_to(&encode(m)?, self.target).await?;
        tracing::debug!("OSC -> {} {} {:?}", self.target, m.addr, m.args);
        Ok(())
    }
}
