use anyhow::Context;
use clap::Parser;
use pchi_studio_api::bench::{builtin_scenarios, report_json, report_markdown, run_all, Agent, Options, Scenario, SHOW};
use std::time::Duration;

/// Give an agent jobs against a real runtime and a digital twin of the rig, and grade what it did.
///
/// Each scenario builds a runtime and a twin, brings them to a known situation, gives the agent a task, and grades the outcome (read from the
/// runtime) and the conduct (read from a trace of every request the agent made). Exit code 1 if any scenario failed.
///
/// Reference agents (`--agent careful|sloppy|reckless|idle`) check the benchmark itself: careful must pass, the others must be caught.
/// A real agent is run with --agent-cmd: a shell command that finds the runtime in PCHI_RUNTIME (HOST:PORT) and its job in PCHI_TASK,
/// for example one that starts pchi-mcp --runtime $PCHI_RUNTIME --name $PCHI_AGENT_NAME for an MCP-capable agent. See docs/bench.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// A reference agent: careful, sloppy, reckless or idle.
    #[arg(long, conflicts_with = "agent_cmd")]
    agent: Option<String>,
    /// A command that is the agent. It gets PCHI_RUNTIME, PCHI_AGENT_NAME, PCHI_TASK and PCHI_SCENARIO in its environment.
    #[arg(long, value_name = "COMMAND")]
    agent_cmd: Option<String>,
    /// Scenarios as a JSON file (same shape as the built-in ones). Default: the built-in set.
    #[arg(long, value_name = "FILE")]
    scenarios: Option<String>,
    /// Run only these scenario ids (comma separated).
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Seconds an agent command may take per scenario.
    #[arg(long, default_value_t = 120)]
    timeout: u64,
    /// The score (0 to 1) a scenario needs as well as passing every critical check.
    #[arg(long, default_value_t = 0.8)]
    pass_mark: f64,
    /// Write the markdown report here.
    #[arg(long, value_name = "FILE")]
    report: Option<String>,
    /// Write every result, with the full trace of calls, as JSON here.
    #[arg(long, value_name = "FILE")]
    json: Option<String>,
    /// List the scenarios and exit.
    #[arg(long)]
    list: bool,
    /// Print the bench show file (the one every scenario runs on) and exit.
    #[arg(long)]
    print_show: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.print_show {
        println!("{SHOW}");
        return Ok(());
    }
    let mut scenarios: Vec<Scenario> = match &cli.scenarios {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?).with_context(|| format!("{p} is not a scenario list"))?,
        None => builtin_scenarios(),
    };
    if !cli.only.is_empty() {
        scenarios.retain(|s| cli.only.contains(&s.id));
    }
    if cli.list {
        for s in &scenarios {
            println!("{:<22} {}\n{:<22} task: {}", s.id, s.title, "", s.task);
        }
        return Ok(());
    }
    anyhow::ensure!(!scenarios.is_empty(), "no scenarios to run");
    let agent = match (&cli.agent, &cli.agent_cmd) {
        (Some(a), _) => Agent::parse(a).map_err(anyhow::Error::msg)?,
        (None, Some(c)) => Agent::Command(c.clone()),
        (None, None) => anyhow::bail!("give --agent careful|sloppy|reckless|idle, or --agent-cmd COMMAND"),
    };
    let opts = Options { timeout: Duration::from_secs(cli.timeout), pass_mark: cli.pass_mark };
    let out = run_all(&scenarios, &agent, &opts).await;
    let md = report_markdown(&out);
    println!("{md}");
    if let Some(p) = &cli.report {
        std::fs::write(p, &md).with_context(|| format!("writing {p}"))?;
    }
    if let Some(p) = &cli.json {
        std::fs::write(p, serde_json::to_string_pretty(&report_json(&out))?).with_context(|| format!("writing {p}"))?;
    }
    if out.iter().any(|o| !o.pass) {
        std::process::exit(1);
    }
    Ok(())
}
