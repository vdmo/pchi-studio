use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use pchi_eventstore::{recorder, replay_latest, EventStore};
use pchi_showchart::{simulate, ShowEngine, Showchart};
use std::net::UdpSocket;

#[derive(Parser)]
#[command(name = "pchi", version, about = "PCHI showchart tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check a showchart for structural problems.
    Validate { file: String },
    /// Print every path through the chart. Emits nothing.
    Simulate { file: String },
    /// Summarise a showchart: states, actions, event alphabet, dead ends, problems.
    Inspect { file: String },
    /// Run the chart against a list of events, printing state changes and actions.
    /// With no --events, follows the first transition from each state until none is left.
    Run {
        file: String,
        /// Comma-separated events, e.g. START,PORTAL_OPEN,END
        #[arg(long, value_delimiter = ',')]
        events: Vec<String>,
        /// Set a context value before running, e.g. --set energy=0.3 (JSON or plain text). Repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Also send each action as a PCHI event (JSON over UDP), e.g. 127.0.0.1:8890
        /// (the pchi-strobe-adapter's default listen address).
        #[arg(long)]
        emit: Option<String>,
        /// Record this run to a SQLite event log (appends; replay uses the latest run).
        #[arg(long)]
        db: Option<String>,
    },
    /// Replay the latest recorded run from the db alone and check it reproduces the recording.
    Replay { db: String },
    /// List every record in an event log.
    Events { db: String },
}

fn load(file: &str) -> anyhow::Result<Showchart> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {file}"))?;
    Showchart::from_json(&text).with_context(|| format!("parsing {file}"))
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Validate { file } => {
            let errs = load(&file)?.validate();
            if errs.is_empty() {
                println!("ok");
                return Ok(());
            }
            errs.iter().for_each(|e| println!("error: {e}"));
            bail!("{} problem(s)", errs.len());
        }
        Cmd::Simulate { file } => {
            let chart = load(&file)?;
            chart.validate().iter().for_each(|e| eprintln!("warning: {e}"));
            print!("{}", simulate::tree(&chart));
        }
        Cmd::Inspect { file } => {
            let c = load(&file)?;
            println!("initial: {}", c.initial);
            println!("states ({}):", c.states.len());
            for s in &c.states {
                println!(
                    "  {:<10} {:<16} enter:{} exit:{}{}",
                    s.id,
                    s.label,
                    s.enter_actions.len(),
                    s.exit_actions.len(),
                    if c.transitions.iter().any(|t| t.source == s.id) { "" } else { "  (terminal)" }
                );
            }
            println!("transitions ({}):", c.transitions.len());
            for t in &c.transitions {
                println!("  {} --{}--> {}", t.source, t.label(), t.target);
            }
            if !c.context.is_empty() {
                println!("context: {}", serde_json::to_string(&c.context)?);
            }
            for (name, expr) in &c.guards {
                println!("guard {name}: {expr}");
            }
            let mut events: Vec<&str> = c.transitions.iter().filter_map(|t| t.event.as_deref()).collect();
            events.sort();
            events.dedup();
            println!("events: {}", events.join(", "));
            let mut kinds: Vec<&str> = c
                .states
                .iter()
                .flat_map(|s| s.enter_actions.iter().chain(s.exit_actions.iter()))
                .map(|a| a.kind.as_str())
                .collect();
            kinds.sort();
            kinds.dedup();
            println!("action kinds: {}", if kinds.is_empty() { "-".into() } else { kinds.join(", ") });
            let problems = c.validate();
            if problems.is_empty() {
                println!("problems: none");
            } else {
                problems.iter().for_each(|p| println!("problem: {p}"));
            }
        }
        Cmd::Run { file, mut events, set, emit, db } => {
            let chart = load(&file)?;
            if events.is_empty() {
                // default path: first transition out of each state, no revisits
                let mut at = chart.initial.clone();
                let mut seen = std::collections::HashSet::new();
                while seen.insert(at.clone()) {
                    match chart.transitions.iter().find(|t| t.source == at) {
                        Some(t) => {
                            events.push(t.trigger().unwrap_or_default());
                            at = t.target.clone();
                        }
                        None => break,
                    }
                }
            }
            let mut engine = ShowEngine::new(chart).map_err(|errs| {
                anyhow::anyhow!(errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
            })?;
            let store = match &db {
                Some(p) => Some(EventStore::open(p).with_context(|| format!("opening {p}"))?),
                None => None,
            };
            if let Some(s) = &store {
                recorder::begin_run(s, &engine, now_ms())?;
            }
            for kv in &set {
                let (k, v) = kv.split_once('=').with_context(|| format!("--set expects KEY=VALUE, got '{kv}'"))?;
                let val = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()));
                match &store {
                    Some(s) => recorder::record_context(s, &mut engine, k, val, now_ms())?,
                    None => engine.set_context(k, val),
                }
            }
            let sock = match &emit {
                Some(_) => Some(UdpSocket::bind("127.0.0.1:0")?),
                None => None,
            };
            let send = |a: &pchi_showchart::Action| -> anyhow::Result<()> {
                println!("  action {} {}", a.kind, a.payload);
                if let (Some(s), Some(addr)) = (&sock, &emit) {
                    s.send_to(a.to_event().to_string().as_bytes(), addr)?;
                }
                Ok(())
            };
            println!("[{}]", engine.current_state);
            let start = match &store {
                Some(s) => recorder::record_start(s, &engine, now_ms())?,
                None => engine.start(),
            };
            for a in start {
                send(&a)?;
            }
            for ev in &events {
                println!("event {ev}");
                let outcome = match &store {
                    Some(s) => recorder::record_input(s, &mut engine, "cli", ev, now_ms())?,
                    None => engine.try_dispatch(ev),
                };
                match outcome {
                    Some(actions) => {
                        println!("[{}]", engine.current_state);
                        for a in actions {
                            send(&a)?;
                        }
                    }
                    None => println!("  ignored in [{}]", engine.current_state),
                }
            }
            if let Some(p) = &db {
                println!("recorded to {p}");
            }
        }
        Cmd::Replay { db } => {
            let store = EventStore::open(&db).with_context(|| format!("opening {db}"))?;
            let r = replay_latest(&store)?;
            println!("replaying latest run of '{}' from {db}", r.chart_id);
            println!("[{}]", r.initial);
            for s in &r.steps {
                if s.applied {
                    println!("{}  {} -> [{}]", clock(s.timestamp), s.event, s.to);
                    for a in &s.actions {
                        println!("             action {} {}", a.kind, a.payload);
                    }
                } else {
                    println!("{}  {} ignored in [{}]", clock(s.timestamp), s.event, s.from);
                }
            }
            if r.matches_recording() {
                println!("replay matches the recording (final state [{}])", r.final_state);
            } else {
                r.divergences.iter().for_each(|d| println!("DIVERGENCE: {d}"));
                bail!("replay does not match the recording");
            }
        }
        Cmd::Events { db } => {
            let store = EventStore::open(&db).with_context(|| format!("opening {db}"))?;
            for r in store.all()? {
                println!("#{:<4} {} {:<10} {:<16} {}", r.id, clock(r.timestamp), r.source, r.event_type, r.payload);
            }
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// HH:MM:SS.mmm (UTC) from epoch milliseconds.
fn clock(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    format!("{:02}:{:02}:{:02}.{:03}", s.div_euclid(3600) % 24, s.div_euclid(60) % 60, s % 60, ms.rem_euclid(1000))
}
