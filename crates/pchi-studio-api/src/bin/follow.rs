use clap::Parser;
use std::sync::Arc;

/// One fixed address that always leads to whichever runtime is running the show.
///
/// Give a button box or gateway THIS address. It asks each --runtime every half second which one says it is active, and passes connections (HTTP and
/// WebSocket) to that one; when the show moves, the connections it carried are closed so they reconnect to the new one. While no runtime is active,
/// connections are refused. `--status` serves what it sees at GET /. See docs/failover.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Address to listen on (must differ from the runtimes' own if they run on this machine).
    #[arg(long, default_value = "0.0.0.0:3100")]
    listen: String,
    /// A runtime to follow, HOST:PORT. Give the primary and the standby.
    #[arg(long = "runtime", required = true, num_args = 1.., value_delimiter = ',')]
    runtimes: Vec<String>,
    /// The runtimes' --token, if they have one.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// Also serve what the follower sees (JSON) here.
    #[arg(long, value_name = "ADDR")]
    status: Option<String>,
    /// Milliseconds between looks.
    #[arg(long, default_value_t = 500)]
    every_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let f = pchi_studio_api::follow::Follower::new(&cli.runtimes, cli.token.clone());
    f.refresh().await;
    let listener = tokio::net::TcpListener::bind(&cli.listen).await?;
    println!("follow: {} leads to the show; following {}", cli.listen, cli.runtimes.join(", "));
    if let Some(addr) = &cli.status {
        let fs = Arc::clone(&f);
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move || {
                let fs = Arc::clone(&fs);
                async move { axum::Json(fs.view.lock().unwrap().json()) }
            }),
        );
        let l = tokio::net::TcpListener::bind(addr).await?;
        tokio::spawn(async move {
            let _ = axum::serve(l, app).await;
        });
        println!("follow: status on http://{addr}/");
    }
    tokio::spawn(Arc::clone(&f).watch(std::time::Duration::from_millis(cli.every_ms.max(100))));
    f.serve(listener).await;
    Ok(())
}
