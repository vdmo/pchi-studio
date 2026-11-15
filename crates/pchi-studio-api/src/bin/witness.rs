use clap::Parser;

/// The witness: decides which runtime may send, so a cut network cannot put two runtimes on the same lights.
///
/// Run it on a machine of its own. The primary and the standby both start with --witness HOST:PORT (this address) and the same --failover-key.
/// It holds nothing but who has the lease, in memory. `GET /primary` says where the show is. See docs/failover.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:3300")]
    bind: String,
    /// The failover key (the same as the runtimes').
    #[arg(long, env = "PCHI_FAILOVER_KEY")]
    failover_key: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.failover_key.is_empty() {
        anyhow::bail!("--failover-key must not be empty");
    }
    let listener = tokio::net::TcpListener::bind(&cli.bind).await?;
    println!("witness: on http://{} (POST /lease with the failover key; GET /primary says where the show is)", cli.bind);
    if !cli.bind.starts_with("127.0.0.1") {
        eprintln!("note: plain HTTP; the failover key crosses the network in clear text unless this is on a network you control (docs/tls.md)");
    }
    axum::serve(listener, pchi_studio_api::witness::router(pchi_studio_api::witness::Witness::new(&cli.failover_key))).await?;
    Ok(())
}
