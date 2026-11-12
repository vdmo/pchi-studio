use clap::Parser;
use pchi_strobe_adapter::config::settings::{Addresses, Settings};
use pchi_strobe_adapter::pchi::client::ConductorClient;
use pchi_strobe_adapter::pchi::events::PchiEvent;
use pchi_strobe_adapter::registry::capabilities::manifest;
use pchi_strobe_adapter::strobe::{osc::OscSender, telemetry::Telemetry};
use pchi_strobe_adapter::translators::router::Router;
use std::time::Duration;
use tokio::net::UdpSocket;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let s = Settings::parse();

    if s.print_manifest {
        println!("{}", serde_json::to_string_pretty(&manifest(&s.node_id))?);
        return Ok(());
    }

    let addrs = Addresses::default();
    let router = Router::new(&addrs);
    let sender = OscSender::new(&s.strobe, s.dry_run).await?;
    let conductor = ConductorClient::new(&s.conductor).await?;
    let events = UdpSocket::bind(&s.listen).await?;
    let tele_in = UdpSocket::bind(&s.telemetry_listen).await?;
    let mut telemetry = Telemetry::new(&s.node_id);

    tracing::info!(
        "{} up: events on {}, OSC -> {}, telemetry on {}, conductor {}{}",
        s.node_id, s.listen, s.strobe, s.telemetry_listen, s.conductor,
        if s.dry_run { " (dry-run)" } else { "" }
    );

    let mut ebuf = vec![0u8; 65536];
    let mut tbuf = vec![0u8; 65536];
    let mut tick = tokio::time::interval(Duration::from_millis(s.telemetry_interval_ms));
    let mut dirty = false;

    loop {
        tokio::select! {
            r = events.recv_from(&mut ebuf) => {
                let (n, from) = r?;
                match PchiEvent::parse(&ebuf[..n]) {
                    Ok(ev) => match router.route(&ev) {
                        Ok(msgs) => for m in msgs { if let Err(e) = sender.send(&m).await { tracing::warn!("OSC send failed: {e}"); } },
                        Err(e) => tracing::warn!("{} from {from}: {e}", ev.event_type),
                    },
                    Err(e) => tracing::warn!("bad event JSON from {from}: {e}"),
                }
            }
            r = tele_in.recv_from(&mut tbuf) => {
                let (n, _) = r?;
                if let Ok((_, pkt)) = rosc::decoder::decode_udp(&tbuf[..n]) {
                    dirty |= apply_packet(&mut telemetry, &addrs.telemetry_prefix, &pkt);
                }
            }
            _ = tick.tick() => {
                if dirty {
                    if let Err(e) = conductor.publish(&telemetry).await { tracing::warn!("telemetry publish failed: {e}"); }
                    dirty = false;
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

fn apply_packet(t: &mut Telemetry, prefix: &str, pkt: &rosc::OscPacket) -> bool {
    match pkt {
        rosc::OscPacket::Message(m) => t.apply(prefix, m),
        rosc::OscPacket::Bundle(b) => b.content.iter().fold(false, |d, p| apply_packet(t, prefix, p) | d),
    }
}
