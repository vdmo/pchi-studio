use anyhow::Context;
use clap::Parser;
use pchi_studio_api::failover::http;
use serde_json::Value;
use std::process::Command;

/// Run a venue day in order and keep everything it finds in one folder.
///
/// It runs, one after another: the doors check on the running show (GET /preflight), the safety drill, the gear check (you at the keyboard, it SENDS FOR REAL
/// after you say yes), the failover day check if you give a standby, and, if you give a recording, the run report of the rehearsal. Each step's report goes into
/// the output folder with an index of what ran, how it ended, and the tool versions. At the end it asks what you did NOT try, and writes that down too:
/// a claim is proven only for what was tried. See docs/real-gear-day.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The show file (.pchi with a rig).
    show: String,
    /// The running runtime, HOST:PORT: for the doors check. Skipped if not given.
    #[arg(long)]
    runtime: Option<String>,
    /// The runtime's --token, if it has one.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
    /// Failover day: the primary and standby (HOST:PORT), with the failover key. Skipped unless both are given.
    #[arg(long)]
    primary: Option<String>,
    #[arg(long)]
    standby: Option<String>,
    #[arg(long)]
    witness: Option<String>,
    #[arg(long, env = "PCHI_FAILOVER_KEY")]
    failover_key: Option<String>,
    /// A recording of the rehearsal run (a --record file), for the run report.
    #[arg(long)]
    recording: Option<String>,
    /// Steps to leave out, comma separated: preflight, drill, gear, failover, report.
    #[arg(long, value_delimiter = ',')]
    skip: Vec<String>,
    /// Where to keep everything. Default: docs/gear-results/<date>.
    #[arg(long)]
    out: Option<String>,
    /// Profiles and fixtures folders, the same you give the runtime.
    #[arg(long)]
    profiles: Option<String>,
    #[arg(long)]
    fixtures: Option<String>,
}

struct Done {
    step: &'static str,
    result: String,
    files: Vec<String>,
    ok: Option<bool>,
}

fn sibling(name: &str) -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut p = dir.join(name);
    if cfg!(windows) {
        p.set_extension("exe");
    }
    p
}

fn version(name: &str) -> String {
    Command::new(sibling(name)).arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{name}: not found next to this program"))
}

fn run(name: &str, args: &[String]) -> anyhow::Result<bool> {
    println!("\n=== {name} {} ===", args.join(" "));
    let st = Command::new(sibling(name)).args(args).status().with_context(|| format!("starting {name} (it should sit next to this program)"))?;
    Ok(st.success())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let date = pchi_studio_api::gearcheck::today();
    let out = cli.out.clone().unwrap_or_else(|| format!("docs/gear-results/{date}"));
    std::fs::create_dir_all(&out).with_context(|| format!("creating {out}"))?;
    let skip = |s: &str| cli.skip.iter().any(|x| x == s);
    let mut done: Vec<Done> = Vec::new();
    let tools = |t: &[String]| -> Vec<String> {
        let mut v = t.to_vec();
        if let Some(p) = &cli.profiles {
            v.extend(["--profiles".into(), p.clone()]);
        }
        if let Some(f) = &cli.fixtures {
            v.extend(["--fixtures".into(), f.clone()]);
        }
        v
    };

    // 1. the doors check, on the running show
    if let (Some(rt), false) = (&cli.runtime, skip("preflight")) {
        let bearer = cli.token.as_ref().map(|t| format!("Bearer {t}"));
        let headers: Vec<(&str, &str)> = bearer.iter().map(|b| ("Authorization", b.as_str())).collect();
        match http(rt, "GET", "/preflight", &headers, None).await {
            Ok((200, v)) => {
                let path = format!("{out}/preflight.json");
                std::fs::write(&path, serde_json::to_string_pretty(&v)?)?;
                let fails = v["checks"].as_array().into_iter().flatten().filter(|c| c["level"] == "fail").count();
                let warns = v["checks"].as_array().into_iter().flatten().filter(|c| c["level"] == "warn").count();
                for c in v["checks"].as_array().into_iter().flatten().filter(|c| c["level"] != "ok") {
                    println!("{:>5}  {}", c["level"].as_str().unwrap_or(""), c["text"].as_str().unwrap_or(""));
                }
                done.push(Done { step: "doors check (GET /preflight)", result: format!("{fails} fail, {warns} warn"), files: vec![path], ok: Some(fails == 0) });
            }
            Ok((c, v)) => done.push(Done { step: "doors check (GET /preflight)", result: format!("answered {c}: {}", v["error"].as_str().unwrap_or("?")), files: vec![], ok: Some(false) }),
            Err(e) => done.push(Done { step: "doors check (GET /preflight)", result: e, files: vec![], ok: Some(false) }),
        }
    }
    // 2. the safety drill
    if !skip("drill") {
        let rep = format!("{out}/drill-report.md");
        let ok = run("pchi-drill", &tools(&[cli.show.clone(), "--interactive".into(), "--report".into(), rep.clone()]))?;
        done.push(Done { step: "safety drill (pchi-drill)", result: if ok { "finished".into() } else { "reported a problem".into() }, files: vec![rep], ok: Some(ok) });
    }
    // 3. the gear check
    if !skip("gear") {
        let rep = format!("{out}/gear-report.md");
        let ok = run("pchi-gearcheck", &tools(&[cli.show.clone(), "--report".into(), rep.clone()]))?;
        done.push(Done { step: "gear check (pchi-gearcheck): SENDS FOR REAL", result: if ok { "finished".into() } else { "ended with a problem".into() }, files: vec![rep], ok: Some(ok) });
    }
    // 4. the failover day
    if let (Some(p), Some(s), Some(k), false) = (&cli.primary, &cli.standby, &cli.failover_key, skip("failover")) {
        let rep = format!("{out}/failover-report.md");
        let mut a = vec!["--primary".into(), p.clone(), "--standby".into(), s.clone(), "--failover-key".into(), k.clone(), "--report".into(), rep.clone()];
        if let Some(w) = &cli.witness {
            a.extend(["--witness".into(), w.clone()]);
        }
        if let Some(t) = &cli.token {
            a.extend(["--token".into(), t.clone()]);
        }
        let ok = run("pchi-netcheck", &a)?;
        done.push(Done { step: "failover day (pchi-netcheck)", result: if ok { "no failures".into() } else { "a check failed".into() }, files: vec![rep], ok: Some(ok) });
    }
    // 5. the run report of the rehearsal
    if let (Some(rec), false) = (&cli.recording, skip("report")) {
        let rep = format!("{out}/run-report.html");
        let ok = run("pchi-run-report", &[rec.clone(), "--out".into(), rep.clone()])?;
        done.push(Done { step: "run report of the rehearsal (pchi-run-report)", result: if ok { "written".into() } else { "failed".into() }, files: vec![rep], ok: Some(ok) });
    }

    // what was not tried: said by a person, kept with the results
    println!("\nWhat did you NOT try today (a tool, a department, the network, a long run)? One line, or press Enter for nothing:");
    let mut untried = String::new();
    let _ = std::io::stdin().read_line(&mut untried);
    let untried = untried.trim();

    let mut md = format!("# Venue day, {date}\n\nShow: `{}`.\n\n## What ran\n\n| Step | Result | Files |\n|---|---|---|\n", cli.show);
    for d in &done {
        md += &format!("| {} | {}{} | {} |\n", d.step, match d.ok { Some(true) => "ok: ", Some(false) => "**problem**: ", None => "" }, d.result.replace('|', "/"), d.files.iter().map(|f| format!("`{}`", std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).collect::<Vec<_>>().join(", "));
    }
    let skipped: Vec<&str> = ["preflight", "drill", "gear", "failover", "report"].into_iter().filter(|s| !done.iter().any(|d| d.step.contains(match *s { "preflight" => "doors", "drill" => "drill", "gear" => "gear", "failover" => "failover", _ => "run report" }))).collect();
    if !skipped.is_empty() {
        md += &format!("\nNot run: {}.\n", skipped.join(", "));
    }
    md += &format!("\n## Not tried\n\n{}\n\n## Versions\n\n", if untried.is_empty() { "Nothing was listed as untried.".to_string() } else { untried.to_string() });
    for t in ["pchi-studio-api", "pchi-gearcheck", "pchi-drill", "pchi-netcheck", "pchi-run-report"] {
        md += &format!("- {}\n", version(t));
    }
    md += "\nA claim is proven only for the version and set-up listed here. Put what failed, and what you changed because of it, in the commit message.\n";
    let index = format!("{out}/index.md");
    std::fs::write(&index, md)?;
    println!("\nindex written to {index}");
    let _: Value = Value::Null;
    if done.iter().any(|d| d.ok == Some(false)) {
        std::process::exit(1);
    }
    Ok(())
}
