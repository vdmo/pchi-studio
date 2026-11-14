use clap::Parser;
use pchi_studio_api::mcp::{serve_stdio, Client};

/// PCHI for AI agents: an MCP server (stdio) that lets an agent read a running show, rehearse an edit and propose it for a person
/// to approve. It cannot press GO, arm a laser, approve or edit directly: the runtime holds it to what an agent may do.
///
/// Add it to an MCP client as a command: `pchi-mcp --runtime 127.0.0.1:3100 --name nova`. See docs/mcp.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The runtime's address.
    #[arg(long, default_value = "127.0.0.1:3100")]
    runtime: String,
    /// The agent's name, as the record will show it (agent:<name>).
    #[arg(long, default_value = "agent")]
    name: String,
    /// The runtime's feed token, if it was started with --token.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// The agent's own roster key, if the runtime has a roster with a named agent (role agent).
    #[arg(long, env = "PCHI_AGENT_KEY")]
    key: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let name: String = cli.name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(32).collect();
    serve_stdio(Client { addr: cli.runtime, name: if name.is_empty() { "agent".into() } else { name }, token: cli.token, key: cli.key }).await;
}
