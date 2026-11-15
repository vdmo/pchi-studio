use anyhow::Context;
use clap::Parser;

/// Turn a recording of a run into one page to read and hand over: where the show went, how late the cues were, what failed to reach a device, every edit,
/// takeover and pause, who decided what, and whether the signed receipts hold together. One self-contained HTML file (no scripts, no network).
///
/// The recording is a `--record` file, a one-file export (`GET /recording`, `--export-run`), or the database the show ran against (`--db`).
/// See docs/report.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The recording file (pchi-run/1 or pchi-runlog/1). Or use --db.
    recording: Option<String>,
    /// Read the latest run from this event database instead.
    #[arg(long, conflicts_with = "recording")]
    db: Option<String>,
    /// The signed receipts (`GET /governance/export` saved to a file), to check and include.
    #[arg(long, value_name = "FILE")]
    chain: Option<String>,
    /// Where to write the page. Default: run-report.html.
    #[arg(long, default_value = "run-report.html")]
    out: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let records = match (&cli.recording, &cli.db) {
        (Some(p), _) => pchi_studio_api::recording::parse(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?).map_err(anyhow::Error::msg)?,
        (None, Some(db)) => pchi_eventstore::EventStore::open(db).with_context(|| format!("opening {db}"))?.all()?,
        (None, None) => anyhow::bail!("give a recording file, or --db"),
    };
    let chain = match &cli.chain {
        Some(p) => {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?).with_context(|| format!("{p} is not JSON"))?;
            Some(pchi_studio_api::report::check_chain(&v))
        }
        None => None,
    };
    let page = pchi_studio_api::report::html(&records, chain.as_ref()).map_err(anyhow::Error::msg)?;
    std::fs::write(&cli.out, page).with_context(|| format!("writing {}", cli.out))?;
    println!("report written to {}", cli.out);
    if let Some(c) = &chain {
        println!("signed record: {}{}", if c.ok { "holds together: " } else { "DOES NOT VERIFY: " }, c.text);
    }
    Ok(())
}
