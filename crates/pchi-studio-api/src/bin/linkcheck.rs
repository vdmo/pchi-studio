use anyhow::Context;
use clap::Parser;
use pchi_studio_api::textlink;
use serde_json::json;

/// Look at a robot (or a simulator) and compare what it really says with what a link profile assumes. READ-ONLY.
///
/// It connects to HOST:PORT and sends only the profile's status reads (for the UR profile: is in remote control, safetystatus, robotmode,
/// programState). It never sends anything that could start, pause, stop, load or change anything. It prints each reply next to what the
/// profile expects, whether the device greeted on connect, and what the profile would conclude (ready, or not and why).
///
/// Use it first against a simulator (URSim), then against the real robot, before pchi-gearcheck. A line marked DIFFERENT means the profile
/// was written from a manual and the device disagrees: fix the profile (or the simulator), do not work around it. `--out FILE` keeps the
/// raw replies as evidence. `--commands` prints every command the profile can ever send, for the robot's programmer to review: no connection.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The link profile's id, for example ur.
    profile: String,
    /// The device's address.
    #[arg(long)]
    host: Option<String>,
    /// The device's port (default: the profile's).
    #[arg(long)]
    port: Option<u16>,
    /// A folder of extra profile files (like the runtime's --profiles).
    #[arg(long, value_name = "DIR")]
    profiles: Option<String>,
    /// Write the raw replies and verdicts to this file (JSON).
    #[arg(long, value_name = "FILE")]
    out: Option<String>,
    /// Print every command this profile can ever send and exit. Connects to nothing.
    #[arg(long)]
    commands: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(dir) = &cli.profiles {
        for l in pchi_studio_api::profiles::load_dir(std::path::Path::new(dir)).map_err(anyhow::Error::msg).with_context(|| format!("profiles {dir}"))? {
            println!("profile: {l}");
        }
    }
    let p = textlink::get(&cli.profile).with_context(|| format!("there is no link profile '{}' (known: {})", cli.profile, textlink::list().iter().map(|p| p.id.clone()).collect::<Vec<_>>().join(", ")))?;
    if cli.commands {
        println!("{} ({}): the whole of what PCHI can ever send to this device is:", p.name, p.id);
        for c in p.commands() {
            println!("    {c}");
        }
        println!("\n({{0}} is a program name from the device's own allow-list. Nothing else is ever sent.)");
        return Ok(());
    }
    let host = cli.host.clone().context("give --host (and --port if it is not the profile's)")?;
    let port = cli.port.or(p.port).context("this profile has no default port: give --port")?;
    let addr = format!("{host}:{port}");
    println!("{}: reading {addr}. Only status reads are sent: nothing is started, stopped or changed.\n", p.name);
    let r = match textlink::probe(&p, &addr).await {
        Ok(r) => r,
        Err(e) => {
            println!("could not reach it: {e}");
            std::process::exit(2);
        }
    };
    match &r.greeting {
        Some(g) => println!("on connect it said:  \"{g}\""),
        None => println!("on connect it said nothing (no greeting within 1.5 s)"),
    }
    for x in &r.reads {
        match (&x.reply, &x.error) {
            (Some(reply), _) if x.matched => println!("  ok         {:<24} -> \"{}\"   (takes {} = {})", x.send, reply, x.var, x.value.clone().unwrap_or_default()),
            (Some(reply), _) => println!("  DIFFERENT  {:<24} -> \"{}\"   (the profile expected a reply like {})", x.send, reply, x.var),
            (None, Some(e)) => println!("  FAILED     {:<24} -> {e}", x.send),
            _ => {}
        }
    }
    println!();
    match r.ready {
        Some(true) => println!("the profile reads this device as READY"),
        Some(false) => println!("the profile reads this device as NOT READY: {}", r.note),
        None => println!("the profile could not decide: not every read was answered in the form it expects"),
    }
    if let Some(out) = &cli.out {
        let doc = json!({
            "profile": p.id, "address": r.addr, "greeting": r.greeting, "ready": r.ready, "note": r.note,
            "reads": r.reads.iter().map(|x| json!({"send": x.send, "reply": x.reply, "matched": x.matched, "value": x.value, "error": x.error})).collect::<Vec<_>>(),
            "read_only": true,
        });
        std::fs::write(out, serde_json::to_string_pretty(&doc)?).with_context(|| format!("writing {out}"))?;
        println!("raw replies written to {out}");
    }
    if r.all_matched() {
        println!("\nevery reply is in the form the profile expects.");
        Ok(())
    } else {
        println!("\nat least one reply is not what the profile expects: see the lines above.");
        std::process::exit(1)
    }
}
