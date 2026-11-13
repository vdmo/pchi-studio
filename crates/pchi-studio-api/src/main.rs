use anyhow::Context;
use clap::Parser;
use pchi_eventstore::EventStore;
use pchi_studio_api::{output::Output, rig, router_with_token, Show};

/// Run a showchart and serve its live state to PCHI Studio.
///
/// It binds to localhost by default. To let other machines at the venue take
/// part (a lighting desk reporting ready, a button box sending GO), bind to a
/// network address AND set --token: without one, any client can send inputs.
///
/// Give it a Studio .pchi file and it also knows the rig: each cue is routed to
/// the device that handles it. Nothing is actually sent unless you add --send.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Show file: a Studio .pchi file (showchart + rig) or a bare showchart (.pchi.json).
    file: String,
    /// Record to this SQLite file (default: in memory, lost on exit).
    #[arg(long)]
    db: Option<String>,
    #[arg(long, default_value = "127.0.0.1:3100")]
    bind: String,
    /// Require this token (`?token=` or `Authorization: Bearer`) on every request. Strongly recommended with a non-local --bind.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// Require this key (`X-Pchi-Edit-Key`, or `?edit_key=`) to send an edited show, undo a send, or test-fire a device. Keeps department pages and viewers from changing the show.
    #[arg(long, env = "PCHI_EDIT_KEY")]
    edit_key: Option<String>,
    /// Really send cues to the rig's devices (UDP/OSC to each device's host and port). Without it, deliveries are only reported as "dry-run".
    #[arg(long)]
    send: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (chart, devices) = rig::load(&std::fs::read_to_string(&cli.file).with_context(|| format!("reading {}", cli.file))?)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parsing {}", cli.file))?;
    let store = match &cli.db {
        Some(p) => EventStore::open(p).with_context(|| format!("opening {p}"))?,
        None => EventStore::open_in_memory()?,
    };
    let routed = devices.as_ref().map(|d| d.len());
    if cli.send && routed.is_none() {
        eprintln!("warning: --send has no effect: {} has no rig (open it in Studio, add devices and Save as .pchi)", cli.file);
    }
    let show = Show::start_with(chart, store, devices.map(|d| Output::new(d, cli.send))).map_err(anyhow::Error::msg)?;
    show.set_edit_key(cli.edit_key.clone());
    let listener = tokio::net::TcpListener::bind(&cli.bind).await.with_context(|| format!("binding {}", cli.bind))?;
    println!("pchi-studio-api: {} on http://{} (ws://{}/ws), recording to {}",
        show.snapshot()["chart"]["id"], cli.bind, cli.bind, cli.db.as_deref().unwrap_or("memory"));
    if cli.edit_key.as_deref().is_some_and(|k| !k.is_empty()) {
        println!("edits: locked, sending a show, undo and test-fire need the operator key");
    }
    match routed {
        Some(n) if cli.send => println!("output: LIVE, sending cues to {n} rig device(s)"),
        Some(n) => println!("output: dry-run, {n} rig device(s) known; add --send to really send cues"),
        None => println!("output: none (no rig in the file)"),
    }
    if cli.token.is_none() && !cli.bind.starts_with("127.0.0.1") && !cli.bind.starts_with("localhost") && !cli.bind.starts_with("[::1]") {
        eprintln!("warning: listening on {} with no --token: anyone on the network can send inputs to the show", cli.bind);
    }
    axum::serve(listener, router_with_token(show, cli.token)).await?;
    Ok(())
}
