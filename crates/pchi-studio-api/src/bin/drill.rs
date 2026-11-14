use anyhow::Context;
use clap::Parser;
use pchi_studio_api::drill;
use pchi_studio_api::gearcheck::{self, Console};
use pchi_studio_api::rig;
use std::io::{BufRead, Write};

/// The show-day safety drill: before the doors, check that PCHI behaves safely with this show.
///
/// It builds a copy of the show in memory, sends NOTHING, and provokes trouble: a laser that must stay held until armed, an agent
/// that must not act, a press that comes too early, a device that cannot be reached, a restart in the middle, an edit that must be
/// undoable, timecode that stops. Then it asks the questions only people can answer. See docs/safety-drill.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Show file: a Studio .pchi file with a rig.
    file: String,
    /// Ask the human questions at the end (default: list them as "not asked").
    #[arg(long)]
    interactive: bool,
    /// Write the report here (markdown). Default: drill-report-<date>.md.
    #[arg(long)]
    report: Option<String>,
    /// Also write a signed receipt into this governance folder (the one the runtime uses with --governance), tied to the show by its hash.
    #[arg(long, value_name = "DIR")]
    sign: Option<String>,
    /// Tool profiles and fixture files, the same folders you give the runtime.
    #[arg(long, value_name = "DIR")]
    profiles: Option<String>,
    #[arg(long, value_name = "DIR")]
    fixtures: Option<String>,
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
    if let Some(dir) = &cli.profiles {
        for l in pchi_studio_api::profiles::load_dir(std::path::Path::new(dir)).map_err(anyhow::Error::msg).with_context(|| format!("profiles {dir}"))? {
            println!("profile: {l}");
        }
    }
    if let Some(dir) = &cli.fixtures {
        for l in pchi_studio_api::fixtures::load_dir(std::path::Path::new(dir)).map_err(anyhow::Error::msg).with_context(|| format!("fixtures {dir}"))? {
            println!("fixture: {l}");
        }
    }
    let (chart, devices) = rig::load(&std::fs::read_to_string(&cli.file).with_context(|| format!("reading {}", cli.file))?).map_err(anyhow::Error::msg).with_context(|| format!("parsing {}", cli.file))?;
    let devices = devices.unwrap_or_default();
    println!("Safety drill: {} device(s). Nothing is sent.\n", devices.len());
    let date = gearcheck::today();
    let mut terminal = Terminal;
    let report = drill::run(&chart, &devices, &date, if cli.interactive { Some(&mut terminal as &mut dyn Console) } else { None }).await;
    println!();
    for d in &report.drills {
        println!("  {:<8} {:<28} {}", d.outcome.to_uppercase(), d.name, d.detail);
    }
    println!("\n{}", report.verdict());
    let path = cli.report.unwrap_or_else(|| format!("drill-report-{date}.md"));
    std::fs::write(&path, drill::markdown(&report)).with_context(|| format!("writing {path}"))?;
    println!("report: {path}");
    if let Some(dir) = &cli.sign {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
        let log = primeswarm_pchi::GovernanceLog::open(&format!("{dir}/governance_key.hex"), &format!("{dir}/governance_log.jsonl")).with_context(|| format!("opening the governance log in {dir}"))?;
        println!("signed receipt: {}", drill::sign(&report, &log));
    }
    if !report.go() {
        std::process::exit(1);
    }
    Ok(())
}
