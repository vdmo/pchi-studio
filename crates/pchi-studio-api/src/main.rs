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
#[derive(Parser, Clone)]
#[command(version, about)]
struct Cli {
    /// Show file: a Studio .pchi file (showchart + rig) or a bare showchart (.pchi.json).
    #[arg(required_unless_present_any = ["midi_ports", "audio_devices", "new_roster_entry", "replay", "export_run"])]
    file: Option<String>,
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
    /// List the MIDI input ports this machine has (needs a build with --features midi) and exit.
    #[arg(long)]
    midi_ports: bool,
    /// A folder of tool profile files (*.json) to add to the built-in ones: how an action becomes the OSC a tool understands. A file with a built-in's id replaces it. See docs/profiles.md.
    #[arg(long, value_name = "DIR", env = "PCHI_PROFILES")]
    profiles: Option<String>,
    /// A folder of DMX fixture files (*.json) to add to the built-in ones: what each channel of a fixture means, so a cue can say wash1=blue. See docs/fixtures.md.
    #[arg(long, value_name = "DIR", env = "PCHI_FIXTURES")]
    fixtures: Option<String>,
    /// List the audio input devices this machine has (needs a build with --features audio) and exit.
    #[arg(long)]
    audio_devices: bool,
    /// Let an online audience vote: the token their gateway (a web page, a chat bot) sends with each vote to POST /vote. It opens that and nothing else, and must differ from --token. See docs/virtual-events.md.
    #[arg(long, env = "PCHI_AUDIENCE_TOKEN")]
    audience_token: Option<String>,
    /// Where the receipt-signing key lives (default: next to the log in the --governance folder). Put it somewhere the show folder's readers cannot reach, and keep a copy of the verifying key (printed at startup) somewhere the runtime's machine cannot change.
    #[arg(long, value_name = "FILE", env = "PCHI_GOVERNANCE_KEY")]
    governance_key: Option<String>,
    /// How long a signed-in session lasts, in hours (1 to 168). Default 8.
    #[arg(long, default_value_t = 8)]
    session_hours: u64,
    /// Keep signed-in sessions in this file so a restart does not sign everyone out. Only a hash of each session token is written (reading the file lets nobody in); it is owner-only on Linux and macOS. A session is dropped if it has expired or its person has left the roster or changed role. See docs/tls.md.
    #[arg(long, value_name = "FILE", env = "PCHI_SESSIONS")]
    sessions: Option<String>,
    /// Say that HTTPS is provided in front of this runtime (a reverse proxy such as Caddy or nginx), so the warning about keys crossing the network in clear is not printed. It changes nothing else. See docs/tls.md.
    #[arg(long)]
    behind_tls: bool,
    /// Make a roster entry for NAME:ROLE (or NAME:ROLE:DEPARTMENTS to limit it to some, for example Sam:editor:lighting) with a fresh random key, print the key once (give it to the person) and the entry (paste it into the roster file: it holds only a salted hash), and exit. Roles: operator, editor, lso, guest, agent, button.
    #[arg(long, value_name = "NAME:ROLE")]
    new_roster_entry: Option<String>,
    /// Write a signed, hash-chained receipt for every decision (edits, arming, GO, refusals, proposals) into this folder, in the format scripts/verify_governance_chain.py checks offline. The folder keeps the signing key too: protect it like the show file.
    #[arg(long, env = "PCHI_GOVERNANCE")]
    governance: Option<String>,
    /// Don't run: play this edited show file against the run recorded in --db and print what would differ (path, cues that would be cut off, timing). Needs no gear.
    #[arg(long, value_name = "FILE")]
    rehearse: Option<String>,
    /// Write the latest run in --db as a recording file (a single JSON file you can keep, send, replay and open in Studio) and exit. See docs/replay.md.
    #[arg(long, value_name = "FILE")]
    export_run: Option<String>,
    /// Don't run: print a recording file as a timeline (what happened, when, who decided what) and check that it replays exactly. Needs no show file.
    #[arg(long, value_name = "FILE")]
    replay: Option<String>,
    /// For --rehearse: play against this recording file instead of a --db log.
    #[arg(long, value_name = "FILE")]
    recording: Option<String>,
    /// Really send cues to the rig's devices (UDP/OSC to each device's host and port). Without it, deliveries are only reported as "dry-run".
    #[arg(long)]
    send: bool,
    /// The secret a standby proves itself with. On the primary: lets a standby read the log and tell this runtime it has taken over (it then stops). On a standby: the same value. Must differ from every other token and key. See docs/failover.md.
    #[arg(long, env = "PCHI_FAILOVER_KEY")]
    failover_key: Option<String>,
    /// Run as the STANDBY of the runtime at HOST:PORT: copy its log, send nothing, and take over if it goes quiet. Give it the same show file, its own --db and --governance, and --failover-key.
    #[arg(long, value_name = "HOST:PORT", requires = "failover_key")]
    standby_of: Option<String>,
    /// Standby only: take over by itself when the primary is silent (auto, the default), or only when a person posts to /takeover (manual).
    #[arg(long, default_value = "auto")]
    takeover: String,
    /// Standby only: seconds of silence from the primary before an auto takeover (2 to 600). Default 6.
    #[arg(long, default_value_t = 6)]
    takeover_after: u64,
    /// Standby only: the name the primary's record shows for it. Default "standby".
    #[arg(long)]
    standby_name: Option<String>,
    /// Let a Studio served from this origin (http://host:port, repeat or comma-separate for several) talk to this runtime from the browser: how Studio follows a takeover to a backup address. Only listed origins are answered; * is not accepted. See docs/failover.md.
    #[arg(long, value_name = "ORIGIN", env = "PCHI_ALLOW_ORIGIN", value_delimiter = ',')]
    allow_origin: Vec<String>,
    /// Standby only: the primary's --token, if different from this runtime's own.
    #[arg(long, env = "PCHI_PRIMARY_TOKEN")]
    primary_token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(spec) = &cli.new_roster_entry {
        let mut parts = spec.splitn(3, ':');
        let (name, role) = (parts.next().unwrap_or(""), parts.next().ok_or_else(|| anyhow::anyhow!("write NAME:ROLE, for example Alex:operator, or NAME:ROLE:DEPARTMENTS, for example Sam:editor:lighting"))?);
        let (key, mut entry) = pchi_studio_api::governance::new_entry(name, &role.trim().to_ascii_lowercase(), 200_000).map_err(anyhow::Error::msg)?;
        if let Some(depts) = parts.next().filter(|d| !d.trim().is_empty()) {
            entry["depts"] = serde_json::json!(depts.split(',').map(|d| d.trim().to_ascii_lowercase()).collect::<Vec<_>>());
            // the entry must read back as a roster entry: say so now rather than at the next start
            pchi_studio_api::governance::Roster::parse(&serde_json::json!({"people": [entry.clone()]}).to_string()).map_err(anyhow::Error::msg)?;
        }
        println!("Key for {name} (shown once: give it to them, it cannot be read back from the roster):\n\n    {key}\n\nRoster entry (paste into the roster file; it holds only a salted hash):\n\n{}\n", serde_json::to_string_pretty(&entry)?);
        return Ok(());
    }
    if cli.audio_devices {
        let devs = pchi_studio_api::ltc::list_devices();
        if devs.is_empty() {
            println!("no audio input devices found (or this build has no audio support: build with --features audio)");
        }
        for d in devs {
            println!("{d}");
        }
        return Ok(());
    }
    if cli.midi_ports {
        let ports = pchi_studio_api::midi::list_ports();
        if ports.is_empty() {
            println!("no MIDI input ports found (or this build has no MIDI support: build with --features midi)");
        }
        for p in ports {
            println!("{p}");
        }
        return Ok(());
    }
    if let Some(dir) = &cli.profiles {
        let lines = pchi_studio_api::profiles::load_dir(std::path::Path::new(dir)).map_err(anyhow::Error::msg).with_context(|| format!("profiles {dir}"))?;
        for l in lines {
            println!("profile: {l}");
        }
    }
    if let Some(dir) = &cli.fixtures {
        for l in pchi_studio_api::fixtures::load_dir(std::path::Path::new(dir)).map_err(anyhow::Error::msg).with_context(|| format!("fixtures {dir}"))? {
            println!("fixture: {l}");
        }
    }
    if let Some(path) = &cli.export_run {
        let db = cli.db.as_deref().ok_or_else(|| anyhow::anyhow!("--export-run needs --db: the log to take the run from"))?;
        let store = EventStore::open(db).with_context(|| format!("opening {db}"))?;
        let rec = pchi_studio_api::recording::export(&store.all()?).map_err(anyhow::Error::msg)?;
        std::fs::write(path, serde_json::to_string_pretty(&rec)?).with_context(|| format!("writing {path}"))?;
        println!("wrote {path}: {} records of {}", rec["records"].as_array().map_or(0, |r| r.len()), rec["show"]);
        return Ok(());
    }
    if let Some(path) = &cli.replay {
        let records = pchi_studio_api::recording::parse(&std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?).map_err(anyhow::Error::msg)?;
        let v = pchi_studio_api::recording::view(&records).map_err(anyhow::Error::msg)?;
        println!("{}: {}", v["show"].as_str().unwrap_or("show"), v["summary"]["line"].as_str().unwrap_or(""));
        println!("{}", pchi_studio_api::recording::verify(&records).map_err(anyhow::Error::msg)?["verdict"].as_str().unwrap_or(""));
        println!("path: {}", v["summary"]["path"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" → "));
        let who: Vec<&str> = v["summary"]["who"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
        if !who.is_empty() {
            println!("who decided things: {}", who.join(", "));
        }
        println!();
        for f in v["frames"].as_array().into_iter().flatten() {
            let t = f["t"].as_i64().unwrap_or(0) as f64 / 1000.0;
            println!("{:>4}:{:04.1}  {:<9} {}", (t / 60.0) as i64, t % 60.0, f["kind"].as_str().unwrap_or(""), f["text"].as_str().unwrap_or(""));
        }
        return Ok(());
    }
    let file = cli.file.clone().ok_or_else(|| anyhow::anyhow!("give a show file"))?;
    let (chart, devices) = rig::load(&std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))?)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parsing {file}"))?;
    let store = match &cli.db {
        Some(p) => EventStore::open(p).with_context(|| format!("opening {p}"))?,
        None => EventStore::open_in_memory()?,
    };
    if let Some(cand) = &cli.rehearse {
        let (cchart, cdev) = rig::load(&std::fs::read_to_string(cand).with_context(|| format!("reading {cand}"))?).map_err(anyhow::Error::msg).with_context(|| format!("parsing {cand}"))?;
        let dev = cdev.or(devices).unwrap_or_default();
        let records = match &cli.recording {
            Some(p) => pchi_studio_api::recording::parse(&std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?).map_err(anyhow::Error::msg)?,
            None if cli.db.is_some() => store.all()?,
            None => anyhow::bail!("--rehearse needs the run to play against: --db (a log) or --recording (a recording file)"),
        };
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
        eprintln!("warning: --send has no effect: {} has no rig (open it in Studio, add devices and Save as .pchi)", file);
    }
    if let Some(key) = cli.failover_key.as_deref().filter(|k| !k.is_empty()) {
        if cli.token.as_deref() == Some(key) || cli.edit_key.as_deref() == Some(key) || cli.audience_token.as_deref() == Some(key) {
            anyhow::bail!("--failover-key must differ from --token, --edit-key and --audience-token: it lets a standby read the whole log and stop this runtime");
        }
    }
    if let Some(primary) = cli.standby_of.clone() {
        return run_standby(cli, primary, devices, store).await;
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
    configure(&show, &cli, if cli.resume { "resumed the run recorded in the database" } else { "started the show" })?;
    if show.failover_enabled() {
        println!("failover: a standby with the failover key may follow this runtime (GET /replicate) and tell it, if it takes over, to stop (POST /fence)");
    }
    start_inputs(&show, &cli).await?;
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
    open_network_warning(&cli);
    let origins = allowed_origins(&cli)?;
    axum::serve(listener, pchi_studio_api::with_cors(router_with_token(show, cli.token), origins)).await?;
    Ok(())
}

/// The origins `--allow-origin` lists; `*` is refused.
fn allowed_origins(cli: &Cli) -> anyhow::Result<Vec<String>> {
    let list: Vec<String> = cli.allow_origin.iter().flat_map(|o| o.split(',')).map(|o| o.trim().trim_end_matches('/').to_string()).filter(|o| !o.is_empty()).collect();
    if list.iter().any(|o| o == "*" || !(o.starts_with("http://") || o.starts_with("https://"))) {
        anyhow::bail!("--allow-origin takes full origins such as http://10.0.0.5:5173, not * and not a bare host");
    }
    Ok(list)
}

fn open_network_warning(cli: &Cli) {
    let local = ["127.0.0.1", "localhost", "[::1]"].iter().any(|p| cli.bind.starts_with(p));
    if !local && !cli.behind_tls && (cli.roster.is_some() || cli.edit_key.as_deref().is_some_and(|k| !k.is_empty())) {
        eprintln!("warning: listening on {} over plain HTTP: keys, session tokens and the feed token cross the network in clear text. Put HTTPS in front of it (docs/tls.md) or add --behind-tls if that is already done.", cli.bind);
    }
    if cli.token.is_none() && !cli.bind.starts_with("127.0.0.1") && !cli.bind.starts_with("localhost") && !cli.bind.starts_with("[::1]") {
        eprintln!("warning: listening on {} with no --token: anyone on the network can send inputs to the show", cli.bind);
    }
}

/// Everything a show needs besides the show itself: who may edit, the roster, the audience token, signed receipts.
fn configure(show: &std::sync::Arc<Show>, cli: &Cli, how: &str) -> anyhow::Result<()> {
    show.set_edit_key(cli.edit_key.clone());
    show.set_session_hours(cli.session_hours);
    if let Some(t) = cli.audience_token.as_deref().filter(|t| !t.is_empty()) {
        if cli.token.as_deref() == Some(t) || cli.edit_key.as_deref() == Some(t) {
            anyhow::bail!("--audience-token must differ from --token and --edit-key: it is handed to an outside gateway and must open nothing but voting");
        }
        show.set_audience_token(Some(t.to_string()));
        println!("audience: voting is on (POST /vote with the audience token)");
    }
    if let Some(path) = &cli.roster {
        let roster = pchi_studio_api::governance::Roster::parse(&std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?).map_err(anyhow::Error::msg).with_context(|| format!("roster {path}"))?;
        println!("roster: {} people: {}", roster.len(), roster.summary());
        if !roster.has_role("operator") && cli.edit_key.as_deref().map_or(true, str::is_empty) {
            eprintln!("warning: nobody on the roster is an operator and there is no --edit-key: nobody can run the show or send edits");
        }
        if roster.plaintext_count() > 0 {
            eprintln!("warning: {} key(s) in {path} are written in clear. Replace each with a hashed entry from --new-roster-entry, and protect the file meanwhile.", roster.plaintext_count());
        }
        for (name, ms) in roster.expiring() {
            println!("roster: {name}'s key {} ({})", if ms < std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0) { "HAS EXPIRED" } else { "expires" }, pchi_studio_api::gearcheck::date_of_ms(ms));
        }
        show.set_roster(roster);
    }
    if let Some(path) = &cli.sessions {
        let n = show.use_sessions_file(path).map_err(anyhow::Error::msg)?;
        println!("sessions: kept in {path}{}", if n > 0 { format!(", {n} still signed in") } else { String::new() });
    }
    if let Some(dir) = &cli.governance {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
        let (key, log) = (cli.governance_key.clone().unwrap_or_else(|| format!("{dir}/governance_key.hex")), format!("{dir}/governance_log.jsonl"));
        if let Some(parent) = std::path::Path::new(&key).parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let log = primeswarm_pchi::GovernanceLog::open(&key, &log).with_context(|| format!("opening the governance log in {dir}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // the signing key is the one thing that makes the record trustworthy: only the runtime's user may read it
            if let Ok(meta) = std::fs::metadata(&key) {
                if meta.permissions().mode() & 0o077 != 0 {
                    let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
                    println!("governance: the signing key {key} could be read by others: set to owner-only");
                }
            }
        }
        println!("governance: signed receipts in {dir} (verifying key {}; keep a copy somewhere the runtime's machine cannot change)", log.verifying_key_hex());
        show.set_governance(log);
        show.receipt("show.start", "ALLOW", &pchi_studio_api::governance::Actor::Person(None), &format!("runtime {how}{}", if cli.send { " with sending ON" } else { " (dry-run)" }));
    }
    show.set_failover_key(cli.failover_key.clone());
    Ok(())
}

/// Open the inputs (LTC, MIDI, OSC buttons) the inputs file lists.
async fn start_inputs(show: &std::sync::Arc<Show>, cli: &Cli) -> anyhow::Result<()> {
    let Some(path) = &cli.inputs else { return Ok(()) };
    let file = pchi_studio_api::inputs::parse_file(&std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?).map_err(anyhow::Error::msg).with_context(|| format!("inputs {path}"))?;
    for spec in file.ltc {
        let input = pchi_studio_api::ltc::start(show, spec).map_err(anyhow::Error::msg)?;
        println!("input: {} reading LTC ({})", input.spec.name, pchi_studio_api::inputs::InputInfo::json(&*input)["listen"].as_str().unwrap_or(""));
    }
    for spec in file.midi {
        let n = spec.bindings.len();
        let input = pchi_studio_api::midi::start(show, spec).map_err(anyhow::Error::msg)?;
        println!("input: {} on MIDI port {} ({n} binding(s))", input.spec.name, pchi_studio_api::inputs::InputInfo::json(&*input)["listen"].as_str().unwrap_or(""));
    }
    for spec in file.inputs {
        let n = spec.bindings.len();
        let from = if spec.allow.is_empty() { "this machine only".to_string() } else { format!("from {}", spec.allow.join(", ")) };
        let input = pchi_studio_api::inputs::start(show, spec).await.map_err(anyhow::Error::msg)?;
        println!("input: {} listening on {} ({n} binding(s), {from})", input.spec.name, input.bound.lock().unwrap().map(|a| a.to_string()).unwrap_or_default());
    }
    Ok(())
}

/// Wait as a standby: copy the primary's log, answer "standby", and take over if it goes quiet.
async fn run_standby(cli: Cli, primary: String, devices: Option<Vec<rig::Device>>, store: EventStore) -> anyhow::Result<()> {
    use pchi_studio_api::failover::{activation, standby_router, Config, Standby};
    let key = cli.failover_key.clone().filter(|k| !k.is_empty()).ok_or_else(|| anyhow::anyhow!("a standby needs --failover-key (the same as the primary's): it is how the primary knows to let it read the log"))?;
    if cli.resume {
        anyhow::bail!("--resume and --standby-of do not go together: a standby builds the run from the primary's log by itself");
    }
    let mut cfg = Config::new(&primary, cli.standby_name.as_deref().unwrap_or("standby"), &key);
    cfg.token = cli.primary_token.clone().or_else(|| cli.token.clone());
    cfg.auto = match cli.takeover.as_str() {
        "auto" => true,
        "manual" => false,
        other => anyhow::bail!("--takeover is auto or manual, not '{other}'"),
    };
    cfg.takeover_after = std::time::Duration::from_secs(cli.takeover_after.clamp(2, 600));
    cfg.db = cli.db.clone();
    let c2 = cli.clone();
    let configure_fn: Box<dyn Fn(&std::sync::Arc<Show>) -> Result<(), String> + Send + Sync> = Box::new(move |show| configure(show, &c2, "took over as the standby").map_err(|e| format!("{e:#}")));
    let activate = activation(devices.clone(), cli.send, cli.token.clone(), configure_fn);
    let sb = Standby::new(cfg, store, activate);
    let c3 = cli.clone();
    sb.on_active(Box::new(move |show| {
        let (c, show) = (c3.clone(), show.clone());
        Box::pin(async move { start_inputs(&show, &c).await.map_err(|e| format!("{e:#}")) })
    }));
    let listener = tokio::net::TcpListener::bind(&cli.bind).await.with_context(|| format!("binding {}", cli.bind))?;
    let when = if cli.takeover == "auto" { format!("taking over after {} s of silence", cli.takeover_after.clamp(2, 600)) } else { "taking over only on POST /takeover".into() };
    println!("standby: following {primary}, {when}; answering on http://{} (GET /role says how it is doing)", cli.bind);
    println!("standby: sends nothing and opens no inputs until it takes over; then its rig is {}", if cli.send { "LIVE" } else { "dry-run" });
    open_network_warning(&cli);
    tokio::spawn(sb.clone().follow());
    axum::serve(listener, pchi_studio_api::with_cors(standby_router(sb), allowed_origins(&cli)?)).await?;
    Ok(())
}
