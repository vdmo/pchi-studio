use anyhow::Context;
use clap::Parser;
use std::collections::HashMap;

/// Bring a cue list from another tool into a PCHI show.
///
/// Reads a cue sheet (CSV or TSV: `time`, `action`, `value`, `label`, `number`, `lane`, `length`, `scene` columns) or a QLab 4 workspace (.qlab4), and writes a
/// show file (.pchi) you can run or open in Studio. Cues with no PCHI equivalent come in as notes on the right lane and send nothing; each is listed.
/// See docs/import.md.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The cue sheet (.csv, .tsv, .txt) or QLab 4 workspace (.qlab4).
    input: String,
    /// Where to write the show. Default: the input's name with .pchi.
    #[arg(long)]
    out: Option<String>,
    /// The show's name. Default: the input's file name.
    #[arg(long)]
    name: Option<String>,
    /// QLab only: give a QLab cue type an action, TYPE=KIND (for example `Audio=audio.snapshot`). Repeat for more. Light is lighting.look unless you map it.
    #[arg(long = "map", value_name = "TYPE=KIND")]
    maps: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let path = std::path::Path::new(&cli.input);
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", cli.input))?;
    let name = cli.name.clone().unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "imported show".into()));
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut map = HashMap::new();
    for m in &cli.maps {
        let (k, v) = m.split_once('=').ok_or_else(|| anyhow::anyhow!("--map takes TYPE=KIND, for example Audio=audio.snapshot"))?;
        map.insert(k.trim().to_string(), v.trim().to_string());
    }
    let imported = match ext.as_str() {
        "qlab4" => pchi_studio_api::import::from_qlab4(&text, &name, &map),
        "qlab5" => anyhow::bail!("QLab 5 workspaces are not read. In QLab 5 you can export the cue list as a CSV-like text, or save the workspace in QLab 4 format; then import that."),
        "noisette" | "score" => anyhow::bail!("Chataigne (.noisette) and ossia (.score) files are not read yet. Export or type the cue list as a cue sheet (docs/import.md)."),
        _ => pchi_studio_api::import::from_csv(&text, &name),
    }
    .map_err(anyhow::Error::msg)?;
    let out = cli.out.clone().unwrap_or_else(|| path.with_extension("pchi").to_string_lossy().into_owned());
    if std::path::Path::new(&out).exists() && out == cli.input {
        anyhow::bail!("the output would overwrite the input: choose --out");
    }
    std::fs::write(&out, serde_json::to_string_pretty(&imported.file)?).with_context(|| format!("writing {out}"))?;
    for n in &imported.notes {
        println!("{n}");
    }
    println!("written to {out}");
    Ok(())
}
