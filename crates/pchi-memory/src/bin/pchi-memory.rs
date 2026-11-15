use anyhow::Context;
use clap::{Parser, Subcommand};
use pchi_memory::{derive, eval, Filter, Kind, Memory, New, RecallOpts, Status, Who};

/// Agent memory for PCHI shows: what happened and what was decided, typed, weighted by who vouched for it, fading, and able to say it does not know.
/// See docs/memory.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The memory file (SQLite).
    #[arg(long, default_value = "memory.db", env = "PCHI_MEMORY")]
    db: String,
    /// Act as this name. By default a person named after you; `--agent` acts as an agent, which can only write drafts.
    #[arg(long = "as")]
    name: Option<String>,
    /// Act as an agent: it may write drafts and recall, and nothing else.
    #[arg(long)]
    agent: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Make drafts from a recording (a --record file or a one-file recording) of a run. Deriving it again changes nothing.
    Derive { recording: String },
    /// Write a memory (a draft, unless a person adds --verify).
    Remember {
        text: String,
        #[arg(long, default_value = "convention")]
        kind: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value = "")]
        show: String,
        #[arg(long, default_value = "")]
        dept: String,
        #[arg(long)]
        verify: bool,
    },
    /// Ask a question of memory.
    Recall {
        question: String,
        #[arg(long)]
        show: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 5)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// List memories.
    List {
        #[arg(long)]
        status: Option<String>,
    },
    /// A person says this memory is right.
    Verify { id: String },
    /// A person replaces a memory with a new, verified one (new words for the same thing).
    Supersede { id: String, text: String },
    /// Two memories disagree: a person says KEEP is right and OVER is wrong. OVER is archived and points to KEEP.
    Settle {
        keep: String,
        #[arg(long)]
        over: String,
    },
    /// Keep a memory from fading.
    Pin { id: String },
    /// Delete a memory for good (needs --yes).
    Forget {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Archive what has faded away.
    Sweep,
    /// The audit trail.
    Audit {
        #[arg(long, default_value_t = 30)]
        last: usize,
    },
    /// Every memory and the audit trail as JSON lines.
    Export,
    /// Counts.
    Stats,
    /// Start a golden set from real recordings: writes the memories the runs give rise to (kept as keys m1, m2, ...) and an empty list of questions for a PERSON to write, in their own words, not copied from the notes. Questions you make up from the answer's wording test little; `golden-check` says so.
    GoldenInit {
        recordings: Vec<String>,
        #[arg(long, default_value = "golden.json")]
        out: String,
    },
    /// Check a golden set for what makes it say little: too few questions, none that memory should decline, questions that repeat their answer's words, memories nobody asks about.
    GoldenCheck {
        golden: String,
        /// Exit non-zero on any warning, not only on an error.
        #[arg(long)]
        strict: bool,
    },
    /// Measure memory against a golden set (see docs/memory.md). `--sample` uses the synthetic one that ships with the crate.
    Eval {
        golden: Option<String>,
        #[arg(long)]
        sample: bool,
        /// Exit non-zero if the hybrid search's recall@5 is below this (0 to 1).
        #[arg(long)]
        fail_under_recall5: Option<f64>,
        /// Exit non-zero if shipped recall answers wrongly more than this many questions.
        #[arg(long)]
        max_wrong: Option<usize>,
        #[arg(long)]
        report: Option<String>,
        /// Compare with a baseline saved earlier: fail if recall@5 fell, fewer questions are answered correctly, or more are answered wrongly.
        #[arg(long)]
        baseline: Option<String>,
        /// Save these results as the baseline to compare against next time.
        #[arg(long)]
        save_baseline: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let now = pchi_memory::now_ms();
    let me = cli.name.clone().unwrap_or_else(|| std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "cli".into()));
    let who = if cli.agent { Who::agent(&me) } else { Who::person(&me) };
    if let Cmd::GoldenInit { recordings, out } = &cli.cmd {
        let mut seen = std::collections::BTreeSet::new();
        let mut mems = Vec::new();
        for p in recordings {
            let records = derive::parse_recording(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?).map_err(anyhow::Error::msg)?;
            for n in derive::from_run(&records).map_err(anyhow::Error::msg)? {
                if seen.insert(pchi_memory::make_id(n.kind, &n.key, &n.text, &n.show)) {
                    mems.push(serde_json::json!({"key": format!("m{}", mems.len() + 1), "kind": n.kind.name(), "text": n.text, "dept": n.dept, "source": p}));
                }
            }
        }
        let g = serde_json::json!({"note": "A golden set made from real recordings. Write the questions yourself, the way a person would ask them, in other words than the notes use: {\"q\": \"...\", \"expect\": [\"m3\"]}, and some the memory should decline: {\"q\": \"...\", \"abstain\": true}. Then run golden-check, then eval.", "memories": mems, "questions": []});
        std::fs::write(out, serde_json::to_string_pretty(&g)?)?;
        println!("{} memory(ies) from {} recording(s) written to {out}: now write at least 30 questions and 5 it should decline.", g["memories"].as_array().map_or(0, |a| a.len()), recordings.len());
        return Ok(());
    }
    if let Cmd::GoldenCheck { golden, strict } = &cli.cmd {
        let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(golden).with_context(|| format!("reading {golden}"))?).context("the golden set is not JSON")?;
        let r = eval::run(&g).map_err(anyhow::Error::msg)?;
        for w in &r.warnings {
            println!("warning: {w}");
        }
        println!("{} memory(ies), {} answerable question(s), {} to decline; {} warning(s).", g["memories"].as_array().map_or(0, |a| a.len()), r.answerable, r.unanswerable, r.warnings.len());
        if *strict && !r.warnings.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Cmd::Eval { golden, sample, fail_under_recall5, max_wrong, report, baseline, save_baseline } = &cli.cmd {
        let text = match (golden, sample) {
            (Some(p), _) => std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?,
            (None, true) => eval::SAMPLE.to_string(),
            _ => anyhow::bail!("give a golden set file, or --sample"),
        };
        let r = eval::run(&serde_json::from_str(&text).context("the golden set is not JSON")?).map_err(anyhow::Error::msg)?;
        println!("{}", r.markdown());
        if let Some(p) = report {
            std::fs::write(p, r.markdown())?;
        }
        let mut failed = false;
        if let Some(min) = fail_under_recall5 {
            if r.hybrid.at5 < *min {
                eprintln!("FAIL: hybrid recall@5 is {:.2}, below {min}", r.hybrid.at5);
                failed = true;
            }
        }
        if let Some(max) = max_wrong {
            if r.wrong + r.answered_anyway > *max {
                eprintln!("FAIL: {} wrong confident answers, more than {max}", r.wrong + r.answered_anyway);
                failed = true;
            }
        }
        if let Some(p) = save_baseline {
            std::fs::write(p, serde_json::to_string_pretty(&r.to_json())?)?;
            println!("baseline saved to {p}");
        }
        if let Some(p) = baseline {
            let b: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?)?;
            let (was5, now5) = (b["hybrid"]["recall_at_5"].as_f64().unwrap_or(0.0), r.hybrid.at5);
            let (was_ok, was_bad) = (b["shipped"]["correct"].as_u64().unwrap_or(0) as usize, (b["shipped"]["wrong"].as_u64().unwrap_or(0) + b["shipped"]["answered_anyway"].as_u64().unwrap_or(0)) as usize);
            if now5 + 1e-9 < was5 {
                eprintln!("REGRESSION: hybrid recall@5 fell from {was5:.2} to {now5:.2}");
                failed = true;
            }
            if r.correct < was_ok {
                eprintln!("REGRESSION: {} questions answered correctly, was {was_ok}", r.correct);
                failed = true;
            }
            if r.wrong + r.answered_anyway > was_bad {
                eprintln!("REGRESSION: {} wrong confident answers, was {was_bad}", r.wrong + r.answered_anyway);
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        return Ok(());
    }
    let mem = Memory::open(&cli.db).map_err(|e| anyhow::anyhow!("{e}"))?;
    let e = |x: pchi_memory::Error| anyhow::anyhow!("{x}");
    match cli.cmd {
        Cmd::Derive { recording } => {
            let records = derive::parse_recording(&std::fs::read_to_string(&recording).with_context(|| format!("reading {recording}"))?).map_err(anyhow::Error::msg)?;
            let d = derive::derive_into(&mem, &records, &Who::agent("derive"), now).map_err(anyhow::Error::msg)?;
            println!("{} new draft(s), {} already known{}. Read them with `list --status draft`, and verify the ones worth keeping.", d.new, d.already, if d.conflicts > 0 { format!(", {} in conflict with something remembered", d.conflicts) } else { String::new() });
        }
        Cmd::Remember { text, kind, key, show, dept, verify } => {
            let kind = Kind::parse(&kind).ok_or_else(|| anyhow::anyhow!("kind is one of: convention, venue, incident, decision, preference, task"))?;
            let (r, fresh) = mem.remember(&who, New { kind, text, key, show, dept, verify, ..Default::default() }, now).map_err(e)?;
            println!("{} {} ({}){}", if fresh { "remembered" } else { "already remembered" }, r.id, r.status.name(), if r.conflicts_with.is_empty() { String::new() } else { format!(", in conflict with {}", r.conflicts_with.join(", ")) });
        }
        Cmd::Recall { question, show, kind, limit, json } => {
            let kind = kind.as_deref().map(|k| Kind::parse(k).ok_or_else(|| anyhow::anyhow!("unknown kind {k}"))).transpose()?;
            let r = mem.recall(&question, &RecallOpts { limit, show, kind, reinforce: true }, now).map_err(e)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r.to_json())?);
            } else if let Some(why) = &r.abstained {
                println!("I don't know: {why}.");
                for h in &r.closest {
                    println!("  closest ({:.2}): [{}] {}", h.alignment, h.record.id, h.record.text);
                }
            } else {
                for h in &r.hits {
                    println!("[{}] {} {:<9} {:.2}  {}", h.record.id, h.record.kind.name(), h.trust, h.activation, h.record.text);
                }
            }
        }
        Cmd::List { status } => {
            let status = match status.as_deref() {
                Some("draft") => Some(Status::Draft),
                Some("verified") => Some(Status::Verified),
                Some("archived") => Some(Status::Archived),
                Some(o) => anyhow::bail!("status is draft, verified or archived, not {o}"),
                None => None,
            };
            for r in mem.list(&Filter { status, ..Default::default() }).map_err(e)? {
                println!("[{}] {:<10} {:<8} {}{}", r.id, r.kind.name(), r.status.name(), r.text, if r.conflicts_with.is_empty() { "" } else { "  (in conflict)" });
            }
        }
        Cmd::Verify { id } => println!("verified {}", mem.verify(&who, &id, now).map_err(e)?.id),
        Cmd::Supersede { id, text } => {
            let old = mem.get(&id).map_err(e)?.ok_or_else(|| anyhow::anyhow!("no memory {id}"))?;
            let r = mem.supersede(&who, &id, New { kind: old.kind, text, ..Default::default() }, now).map_err(e)?;
            println!("{id} is archived; {} replaces it", r.id);
        }
        Cmd::Settle { keep, over } => {
            mem.settle(&who, &keep, &over, now).map_err(e)?;
            println!("{keep} is kept; {over} is archived");
        }
        Cmd::Pin { id } => {
            mem.pin(&who, &id, true, now).map_err(e)?;
            println!("pinned {id}");
        }
        Cmd::Forget { id, yes } => {
            mem.forget(&who, &id, yes, now).map_err(e)?;
            println!("deleted {id}");
        }
        Cmd::Sweep => println!("archived {} faded memory(ies)", mem.sweep(now).map_err(e)?.len()),
        Cmd::Audit { last } => {
            for a in mem.audit(last).map_err(e)? {
                println!("{} {:<9} {} by {}  {}", a["ts"], a["action"].as_str().unwrap_or(""), a["id"].as_str().unwrap_or("-"), a["by"].as_str().unwrap_or(""), a["detail"].as_str().unwrap_or(""));
            }
        }
        Cmd::Export => print!("{}", mem.export_jsonl().map_err(e)?),
        Cmd::Stats => println!("{}", mem.stats().map_err(e)?),
        Cmd::Eval { .. } | Cmd::GoldenInit { .. } | Cmd::GoldenCheck { .. } => unreachable!(),
    }
    Ok(())
}
