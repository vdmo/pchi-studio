use anyhow::Context;
use clap::Parser;
use pchi_studio_api::bench::{builtin_scenarios, report_json, report_markdown, run_all, summarize, verify_evidence, write_evidence, Agent, Options, Scenario, SHOW};
use std::time::Duration;

/// Give an agent jobs against a real runtime and a digital twin of the rig, and grade what it did.
///
/// Each scenario builds a runtime and a twin, brings them to a known situation, gives the agent a task, and grades the outcome (read from the
/// runtime) and the conduct (read from a trace of every request the agent made). Exit code 1 if a critical check failed in any run, or a
/// scenario's pass rate is below --min-pass-rate.
///
/// Reference agents (`--agent careful|sloppy|reckless|idle`) check the benchmark itself: careful must pass, the others must be caught.
/// A real agent is run with --agent-cmd: a shell command that finds the runtime in PCHI_RUNTIME (HOST:PORT) and its job in PCHI_TASK;
/// bench/agents/claude.sh is one for Claude Code. See docs/bench.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// A reference agent: careful, sloppy, reckless or idle.
    #[arg(long, conflicts_with = "agent_cmd")]
    agent: Option<String>,
    /// A command that is the agent. It gets PCHI_RUNTIME, PCHI_AGENT_NAME, PCHI_TASK, PCHI_SCENARIO and PCHI_USAGE_FILE in its environment.
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
    /// Run each scenario this many times. A language-model agent is one sample per run: 5 to 10 is a start.
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// The pass rate (0 to 1) every scenario needs across its repeats, as well as no critical failure in any run.
    #[arg(long, default_value_t = 1.0)]
    min_pass_rate: f64,
    /// Write one evidence file per run, and an index, into this NEW folder: the task, every request the agent made, each check, what the run
    /// was made with, and a SHA-256 receipt. Never overwrites.
    #[arg(long, value_name = "DIR")]
    evidence: Option<String>,
    /// Check evidence files (or an index, and the cases it lists) against their receipts, and exit.
    #[arg(long, value_name = "FILE", num_args = 1..)]
    verify: Vec<String>,
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
    if !cli.verify.is_empty() {
        let mut bad = false;
        for f in &cli.verify {
            match verify_evidence(f) {
                Ok(m) => println!("ok: {m}"),
                Err(e) => {
                    bad = true;
                    println!("FAILED: {e}");
                }
            }
        }
        std::process::exit(if bad { 1 } else { 0 });
    }
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
            println!("{:<22} {}{}\n{:<22} task: {}", s.id, s.title, if s.negative { "  [negative]" } else { "" }, "", s.task);
        }
        return Ok(());
    }
    anyhow::ensure!(!scenarios.is_empty(), "no scenarios to run");
    let agent = match (&cli.agent, &cli.agent_cmd) {
        (Some(a), _) => Agent::parse(a).map_err(anyhow::Error::msg)?,
        (None, Some(c)) => Agent::Command(c.clone()),
        (None, None) => anyhow::bail!("give --agent careful|sloppy|reckless|idle, or --agent-cmd COMMAND"),
    };
    let opts = Options { timeout: Duration::from_secs(cli.timeout), pass_mark: cli.pass_mark, repeat: cli.repeat.max(1) };
    let out = run_all(&scenarios, &agent, &opts).await;
    let md = report_markdown(&out);
    println!("{md}");
    if let Some(p) = &cli.report {
        std::fs::write(p, &md).with_context(|| format!("writing {p}"))?;
    }
    if let Some(p) = &cli.json {
        std::fs::write(p, serde_json::to_string_pretty(&report_json(&out))?).with_context(|| format!("writing {p}"))?;
    }
    if let Some(d) = &cli.evidence {
        let files = write_evidence(d, &out, &scenarios).map_err(anyhow::Error::msg)?;
        println!("evidence: {} file(s) written to {d} (check them later with: pchi-bench --verify {d}/index.json)", files.len());
    }
    let failed = summarize(&out).iter().any(|s| s.critical_failures > 0 || s.pass_rate() < cli.min_pass_rate);
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
