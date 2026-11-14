use anyhow::Context;
use clap::Parser;
use pchi_studio_api::gearcheck::{self, Console, Options};
use pchi_studio_api::{output::Output, rig};
use std::io::{BufRead, Write};

/// Check each device in a show's rig against the real tool, one at a time, with you at the keyboard.
///
/// For every device it confirms it is safe (lasers: LASER SAFE; audio desks: PA muted), sends ONE cue of your choosing
/// and shows the exact OSC message, asks whether the tool reacted, listens for what the tool sends back, and, for tools
/// that can be probed, asks you to pull and restore the network cable. It SENDS FOR REAL, only after you say yes.
/// See docs/gear-check.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Show file: a Studio .pchi file with a rig.
    file: String,
    /// Check only these device ids (comma separated). Default: every device with a tool profile.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Write the report here (markdown). Default: gear-report-<date>.md next to where you run it.
    #[arg(long)]
    report: Option<String>,
    /// Tick the Status column of this checklist (docs/foh-proof.md) for what was actually tried. Skipped devices change nothing.
    #[arg(long, value_name = "FILE")]
    update_doc: Option<String>,
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
            return String::new(); // input closed: every question is answered "skip"
        }
        line.trim_end().to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (_, devices) = rig::load(&std::fs::read_to_string(&cli.file).with_context(|| format!("reading {}", cli.file))?).map_err(anyhow::Error::msg).with_context(|| format!("parsing {}", cli.file))?;
    let devices = devices.filter(|d| !d.is_empty()).ok_or_else(|| anyhow::anyhow!("{} has no rig: add devices in Studio's Rig tab and Save as .pchi", cli.file))?;
    println!("Gear check: {} device(s). This sends real messages, one device at a time, only after you confirm.\n", devices.len());
    let out = Output::new(devices, true);
    let results = gearcheck::run(&mut Terminal, &out, &cli.only, &Options::for_output(&out)).await;
    let date = gearcheck::today();
    let report = cli.report.unwrap_or_else(|| format!("gear-report-{date}.md"));
    std::fs::write(&report, gearcheck::report_markdown(&results, &date)).with_context(|| format!("writing {report}"))?;
    println!("\n{}", results.iter().map(|r| format!("  {:<10} {:<12} {}", r.verdict, r.tool, r.label)).collect::<Vec<_>>().join("\n"));
    println!("\nreport: {report}");
    if let Some(doc) = &cli.update_doc {
        let text = std::fs::read_to_string(doc).with_context(|| format!("reading {doc}"))?;
        std::fs::write(doc, gearcheck::update_doc(&text, &results, &date)).with_context(|| format!("writing {doc}"))?;
        println!("checklist updated: {doc}");
    }
    Ok(())
}
