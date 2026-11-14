use anyhow::Context;
use clap::Parser;
use pchi_eventstore::EventStore;
use pchi_studio_api::{output::Output, rig, router_with_token, Show};

/// Run a showchart and serve its live state to PCHI Studio.
///
/// It binds to localhost by default. To let other machines at the venue take
/// part (a lighting desk reporting ready, a button box sending GO), bind to a
/// network address AND set --token: without one, any client can send inputs.
///
/// Give it a Studio .pchi file and it also knows the rig: each cue is routed to
/// the device that handles it. Nothing is actually sent unless you add --send.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Show file: a Studio .pchi file (showchart + rig) or a bare showchart (.pchi.json).
    file: String,
    /// Record to this SQLite file (default: in memory, lost on exit).
    #[arg(long)]
    db: Option<String>,
    #[arg(long, default_value = "127.0.0.1:3100")]
    bind: String,
    /// Require this token (`?token=` or `Authorization: Bearer`) on every request. Strongly recommended with a non-local --bind.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// Require this key (`X-Pchi-Edit-Key`, or `?edit_key=`) to send an edited show, undo a send, or test-fire a device. Keeps department pages and viewers from changing the show.
    #[arg(long, env = "PCHI_EDIT_KEY")]
    edit_key: Option<String>,
    /// Carry on the run recorded in --db instead of starting a new one: the scene the show was in, its context and the cues still ahead. Cues that came due while the runtime was down are skipped (and listed), lasers start disarmed, and departments report READY again.
    #[arg(long, requires = "db")]
    resume: bool,
    /// Named people and their roles, a JSON list of {"name", "role", "key"}: operator (everything), editor (edit, approve, undo, try, test), lso (arm lasers; try and test lasers only), guest (look only), agent (a named agent: proposes only). Each person signs in with their own key (Studio's Sign in). The file holds the keys: protect it. See docs/roles.md.
    #[arg(long, value_name = "FILE", env = "PCHI_ROSTER")]
    roster: Option<String>,
    /// OSC inputs, a JSON file: a Stream Deck, tablet or button box that presses GO, sets a context value or hits STOP LASERS (never arms or edits). Listening beyond this machine needs an allow list of sender addresses. See docs/osc-input.md.
    #[arg(long, value_name = "FILE", env = "PCHI_INPUTS")]
    inputs: Option<String>,
    /// Write a signed, hash-chained receipt for every decision (edits, arming, GO, refusals, proposals) into this folder, in the format scripts/verify_governance_chain.py checks offline. The folder keeps the signing key too: protect it like the show file.
    #[arg(long, env = "PCHI_GOVERNANCE")]
    governance: Option<String>,
    /// Don't run: play this edited show file against the run recorded in --db and print what would differ (path, cues that would be cut off, timing). Needs no gear.
    #[arg(long, value_name = "FILE", requires = "db")]
    rehearse: Option<String>,
    /// Really send cues to the rig's devices (UDP/OSC to each device's host and port). Without it, deliveries are only reported as "dry-run".
    #[arg(long)]
    send: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (chart, devices) = rig::load(&std::fs::read_to_string(&cli.file).with_context(|| format!("reading {}", cli.file))?)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parsing {}", cli.file))?;
    let store = match &cli.db {
        Some(p) => EventStore::open(p).with_context(|| format!("opening {p}"))?,
        None => EventStore::open_in_memory()?,
    };
    if let Some(cand) = &cli.rehearse {
        let (cchart, cdev) = rig::load(&std::fs::read_to_string(cand).with_context(|| format!("reading {cand}"))?).map_err(anyhow::Error::msg).with_context(|| format!("parsing {cand}"))?;
        let dev = cdev.or(devices).unwrap_or_default();
        let records = store.all()?;
        let until = records.last().map_or(0, |r| r.timestamp);
        let r = pchi_studio_api::rehearse::run(&records, &cchart, &dev, until).map_err(anyhow::Error::msg)?;
        println!("{}", r["summary"]["verdict"].as_str().unwrap_or(""));
        for l in r["summary"]["lines"].as_array().into_iter().flatten() {
            println!("  {}", l.as_str().unwrap_or(""));
        }
        println!("recorded path:  {}", r["recorded_path"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" → "));
        println!("rehearsed path: {}", r["rehearsed_path"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" → "));
        return Ok(());
    }
    let routed = devices.as_ref().map(|d| d.len());
    if cli.send && routed.is_none() {
        eprintln!("warning: --send has no effect: {} has no rig (open it in Studio, add devices and Save as .pchi)", cli.file);
    }
    let output = devices.map(|d| Output::new(d, cli.send));
    let show = if cli.resume {
        let (show, info) = Show::resume(chart, store, output).map_err(anyhow::Error::msg).context("resuming")?;
        println!(
            "resumed in {} after {:.0} s ({:.0} s with the runtime down); skipped {} cue(s) and {} timer(s) that came due meanwhile",
            info["state"], info["elapsed_ms"].as_f64().unwrap_or(0.0) / 1000.0, info["downtime_ms"].as_f64().unwrap_or(0.0) / 1000.0,
            info["skipped_cues"].as_array().map_or(0, |a| a.len()), info["skipped_timers"].as_array().map_or(0, |a| a.len())
        );
        for c in info["skipped_cues"].as_array().into_iter().flatten() {
            println!("  skipped cue: {c}");
        }
        show
    } else {
        Show::start_with(chart, store, output).map_err(anyhow::Error::msg)?
    };
    show.set_edit_key(cli.edit_key.clone());
    if let Some(path) = &cli.roster {
        let roster = pchi_studio_api::governance::Roster::parse(&std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?).map_err(anyhow::Error::msg).with_context(|| format!("roster {path}"))?;
        println!("roster: {} people: {}", roster.len(), roster.summary());
        if !roster.has_role("operator") && cli.edit_key.as_deref().map_or(true, str::is_empty) {
            eprintln!("warning: nobody on the roster is an operator and there is no --edit-key: nobody can run the show or send edits");
        }
        show.set_roster(roster);
    }
    if let Some(dir) = &cli.governance {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
        let (key, log) = (format!("{dir}/governance_key.hex"), format!("{dir}/governance_log.jsonl"));
        let log = primeswarm_pchi::GovernanceLog::open(&key, &log).with_context(|| format!("opening the governance log in {dir}"))?;
        println!("governance: signed receipts in {dir} (verifying key {})", log.verifying_key_hex());
        show.set_governance(log);
        let how = if cli.resume { "resumed the run recorded in the database" } else { "started the show" };
        show.receipt("show.start", "ALLOW", &pchi_studio_api::governance::Actor::Person(None), &format!("runtime {how}{}", if cli.send { " with sending ON" } else { " (dry-run)" }));
    }
    if let Some(path) = &cli.inputs {
        let specs = pchi_studio_api::inputs::parse(&std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?).map_err(anyhow::Error::msg).with_context(|| format!("inputs {path}"))?;
        for spec in specs {
            let n = spec.bindings.len();
            let from = if spec.allow.is_empty() { "this machine only".to_string() } else { format!("from {}", spec.allow.join(", ")) };
            let input = pchi_studio_api::inputs::start(&show, spec).await.map_err(anyhow::Error::msg)?;
            println!("input: {} listening on {} ({n} binding(s), {from})", input.spec.name, input.bound.lock().unwrap().map(|a| a.to_string()).unwrap_or_default());
        }
    }
    let listener = tokio::net::TcpListener::bind(&cli.bind).await.with_context(|| format!("binding {}", cli.bind))?;
    println!("pchi-studio-api: {} on http://{} (ws://{}/ws), recording to {}",
        show.snapshot()["chart"]["id"], cli.bind, cli.bind, cli.db.as_deref().unwrap_or("memory"));
    if cli.edit_key.as_deref().is_some_and(|k| !k.is_empty()) {
        println!("edits: locked, sending a show, undo and test-fire need the operator key");
    }
    match routed {
        Some(n) if cli.send => println!("output: LIVE, sending cues to {n} rig device(s)"),
        Some(n) => println!("output: dry-run, {n} rig device(s) known; add --send to really send cues"),
        None => println!("output: none (no rig in the file)"),
    }
    if cli.token.is_none() && !cli.bind.starts_with("127.0.0.1") && !cli.bind.starts_with("localhost") && !cli.bind.starts_with("[::1]") {
        eprintln!("warning: listening on {} with no --token: anyone on the network can send inputs to the show", cli.bind);
    }
    axum::serve(listener, router_with_token(show, cli.token)).await?;
    Ok(())
}
