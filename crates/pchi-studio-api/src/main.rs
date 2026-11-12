use anyhow::Context;
use clap::Parser;
use pchi_eventstore::EventStore;
use pchi_showchart::Showchart;
use pchi_studio_api::{router, Show};

/// Run a showchart and serve its live state to PCHI Studio.
///
/// No authentication: it binds to localhost by default. Do not expose it
/// beyond a trusted network, since any client can send inputs.
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
    axum::serve(listener, router(show)).await?;
    Ok(())
}
