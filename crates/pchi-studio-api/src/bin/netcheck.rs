use clap::Parser;
use pchi_studio_api::gearcheck::Console;
use pchi_studio_api::netcheck::{self, Config};
use std::io::{BufRead, Write};

/// A failover day: check, over the real network between the real machines, that the failover works, and write down what was measured.
///
/// First it only asks questions (delay and loss to each runtime, clocks, roles, whether the standby is following, a rehearsed takeover, the witness).
/// Then, if you agree, it times a real takeover while YOU cut the primary off, and how long the old primary takes to stop once it is back.
/// It sends nothing to any tool. See docs/failover.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The primary runtime, HOST:PORT.
    #[arg(long)]
    primary: String,
    /// The standby runtime, HOST:PORT.
    #[arg(long)]
    standby: String,
    /// The witness, HOST:PORT, if there is one.
    #[arg(long)]
    witness: Option<String>,
    /// The failover key (the runtimes' and witness's).
    #[arg(long, env = "PCHI_FAILOVER_KEY")]
    failover_key: String,
    /// The runtimes' --token, if they have one.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// Do not offer the cut test: only the checks that change nothing.
    #[arg(long)]
    no_cut: bool,
    /// Write the report here (markdown). Default: failover-report-<date>.md.
    #[arg(long)]
    report: Option<String>,
    /// Answers to ask of each runtime for the delay figures.
    #[arg(long, default_value_t = 10)]
    samples: usize,
}

struct Terminal;

impl Console for Terminal {
    fn say(&mut self, text: &str) {
        println!("{text}");
    }
    fn ask(&mut self, prompt: &str) -> String {
        print!("{prompt} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            return String::new();
        }
        line.trim_end().to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::new(&cli.primary, &cli.standby, &cli.failover_key);
    cfg.witness = cli.witness.clone();
    cfg.token = cli.token.clone();
    cfg.samples = cli.samples;
    let mut steps = netcheck::survey(&cfg).await;
    for s in &steps {
        println!("{:>8}  {}: {}", s.verdict, s.name, s.detail);
    }
    let broken = steps.iter().any(|s| s.verdict == "fail");
    if !cli.no_cut {
        if broken {
            println!("\nSome checks failed: fix those before the cut test (it would only tell you the same thing slower).");
        } else {
            let more = netcheck::cut_test(&cfg, &mut Terminal).await;
            for s in &more {
                println!("{:>8}  {}: {}", s.verdict, s.name, s.detail);
            }
            steps.extend(more);
        }
    }
    let date = pchi_studio_api::gearcheck::today();
    let path = cli.report.unwrap_or_else(|| format!("failover-report-{date}.md"));
    std::fs::write(&path, netcheck::report_markdown(&cfg, &steps, &date))?;
    println!("\nreport written to {path}");
    if steps.iter().any(|s| s.verdict == "fail") {
        std::process::exit(1);
    }
    Ok(())
}
