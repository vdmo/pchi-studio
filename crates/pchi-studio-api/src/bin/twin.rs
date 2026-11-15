use anyhow::Context;
use axum::{extract::{Path, State}, routing::{get, post}, Json, Router};
use clap::Parser;
use pchi_studio_api::twin::{rewrite_show, Fault, Twin};
use serde_json::{json, Value};
use std::sync::Arc;

/// A digital twin of a show's rig: stand-in devices on loopback that record what the runtime sends and can be made to fail.
///
/// It reads a show's rig, stands up one twin per UDP device, writes a copy of the show with every device pointed at its twin, and serves
/// a small control API so a script (or you, with curl) can see what each device received and make one go down or lag. Then run the
/// runtime on the copy with --send: nothing real is involved. See docs/twin.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The show file: a Studio .pchi with a rig.
    show: String,
    /// Where to write the copy of the show pointed at the twins. Default: the show's name with .twin.pchi.
    #[arg(long)]
    out: Option<String>,
    /// Where to serve the control API.
    #[arg(long, default_value = "127.0.0.1:3400")]
    control: String,
    /// A running runtime (HOST:PORT) to report every department READY to, as the people at the desks would.
    #[arg(long)]
    runtime: Option<String>,
    /// That runtime's --token, if it has one.
    #[arg(long, env = "PCHI_TOKEN")]
    token: Option<String>,
}

type Shared = Arc<(Twin, Option<String>, Option<String>)>;

async fn devices(State(s): State<Shared>) -> Json<Value> {
    Json(json!({"devices": s.0.json(), "skipped": s.0.skipped().iter().map(|(i, w)| json!({"id": i, "why": w})).collect::<Vec<_>>()}))
}

async fn seen(State(s): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    Json(json!({"device": id, "received": s.0.seen(&id).iter().map(|x| x.json()).collect::<Vec<_>>()}))
}

async fn fault(State(s): State<Shared>, Json(b): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
    let id = b["device"].as_str().unwrap_or("");
    match Fault::parse(b["fault"].as_str().unwrap_or("")).and_then(|f| s.0.set_fault(id, f).map(|_| f)) {
        Ok(f) => (axum::http::StatusCode::OK, Json(json!({"device": id, "fault": f.text()}))),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

async fn ready(State(s): State<Shared>, Json(b): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
    let Some(rt) = &s.1 else { return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "start the twin with --runtime to report readiness"}))) };
    match Twin::report_ready(rt, s.2.as_deref(), b["dept"].as_str().unwrap_or(""), b["ready"].as_bool().unwrap_or(true), b["note"].as_str().unwrap_or("twin")).await {
        Ok(()) => (axum::http::StatusCode::OK, Json(json!({"ok": true}))),
        Err(e) => (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error": e}))),
    }
}

async fn clear(State(s): State<Shared>) -> Json<Value> {
    s.0.clear();
    Json(json!({"cleared": true}))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.show).with_context(|| format!("reading {}", cli.show))?;
    let (_, rig) = pchi_studio_api::rig::load(&text).map_err(anyhow::Error::msg)?;
    let rig = rig.context("this show has no rig: open it in Studio, add devices and Save as .pchi")?;
    let twin = Twin::start(&rig).await.map_err(anyhow::Error::msg)?;
    let out = cli.out.clone().unwrap_or_else(|| format!("{}.twin.pchi", cli.show.trim_end_matches(".pchi")));
    std::fs::write(&out, rewrite_show(&text, &twin).map_err(anyhow::Error::msg)?).with_context(|| format!("writing {out}"))?;
    println!("twin: {} device(s) standing; the show pointed at them is {out}", twin.ids().len());
    for (id, why) in twin.skipped() {
        println!("  no twin for {id}: {why}");
    }
    println!("run it with:  pchi-studio-api {out} --send");
    if let Some(rt) = &cli.runtime {
        let mut depts: Vec<String> = twin.rig().iter().map(|d| d.dept.clone()).collect();
        depts.sort();
        depts.dedup();
        for d in depts {
            match Twin::report_ready(rt, cli.token.as_deref(), &d, true, "twin ready").await {
                Ok(()) => println!("reported {d} ready to {rt}"),
                Err(e) => eprintln!("could not report {d} ready to {rt}: {e}"),
            }
        }
    }
    let app = Router::new()
        .route("/devices", get(devices))
        .route("/seen/:id", get(seen))
        .route("/fault", post(fault))
        .route("/ready", post(ready))
        .route("/clear", post(clear))
        .with_state(Arc::new((twin, cli.runtime.clone(), cli.token.clone())));
    let listener = tokio::net::TcpListener::bind(&cli.control).await.with_context(|| format!("cannot listen on {}", cli.control))?;
    println!("control API on http://{}  (GET /devices, GET /seen/<id>, POST /fault {{\"device\":\"l1\",\"fault\":\"down\"}}, POST /ready, POST /clear)", cli.control);
    axum::serve(listener, app).await?;
    Ok(())
}
