use anyhow::Context;
use clap::Parser;
use pchi_eventstore::EventStore;
use pchi_showchart::Showchart;
use pchi_studio_api::{router_with_token, Show};

/// Run a showchart and serve its live state to PCHI Studio.
///
/// It binds to localhost by default. To let other machines at the venue take
/// part (a lighting desk reporting ready, a button box sending GO), bind to a
/// network address AND set --token: without one, any client can send inputs.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Showchart file (.pchi.json).
    file: String,
    /// Record to this SQLite file (default: in memory, lost on exit).
    #[arg(long)]
    db: Option<String>,
    #[arg(long, default_value = "127.0.0.1:3100")]
    bind: String,
    /// Require this token (`?token=` or `Authorization: Bearer`) on every request. Strongly recommended with a non-local --bind.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let chart = Showchart::from_json(&std::fs::read_to_string(&cli.file).with_context(|| format!("reading {}", cli.file))?)
        .with_context(|| format!("parsing {}", cli.file))?;
    let store = match &cli.db {
        Some(p) => EventStore::open(p).with_context(|| format!("opening {p}"))?,
        None => EventStore::open_in_memory()?,
    };
    let show = Show::start(chart, store).map_err(anyhow::Error::msg)?;
    let listener = tokio::net::TcpListener::bind(&cli.bind).await.with_context(|| format!("binding {}", cli.bind))?;
    println!("pchi-studio-api: {} on http://{} (ws://{}/ws), recording to {}",
        show.snapshot()["chart"]["id"], cli.bind, cli.bind, cli.db.as_deref().unwrap_or("memory"));
    if cli.token.is_none() && !cli.bind.starts_with("127.0.0.1") && !cli.bind.starts_with("localhost") && !cli.bind.starts_with("[::1]") {
        eprintln!("warning: listening on {} with no --token: anyone on the network can send inputs to the show", cli.bind);
    }
    axum::serve(listener, router_with_token(show, cli.token)).await?;
    Ok(())
}
