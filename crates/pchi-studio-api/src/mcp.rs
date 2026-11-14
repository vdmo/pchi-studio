//! An MCP server for PCHI, so an AI agent can look at a show, rehearse an idea and propose it: nothing more.
//!
//! MCP (the Model Context Protocol) is JSON-RPC 2.0, one message a line, over standard input and output. This speaks the part of it a
//! tool server needs (`initialize`, `ping`, `tools/list`, `tools/call`) and turns each tool into a request to a running runtime's HTTP
//! API, **as an agent**: it sends `X-Pchi-Actor: agent:<name>`, and the runtime holds it to what an agent may do.
//!
//! What an agent can do here: read the show and the run, run the Doors check, **rehearse** an edit against what already happened,
//! **propose** an edit (it waits for a person to approve it), and hit STOP LASERS (the safe direction).
//!
//! What is not offered, and would be refused if it were asked anyway: pressing GO, arming a laser, trying or test-firing a device,
//! approving a proposal, undoing a send, running a poll, editing directly. The runtime enforces that, not this file: leaving a tool
//! out is a convenience, the refusal is the guarantee.

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_TEXT: usize = 24_000;

/// Where the runtime is and who the agent says it is.
pub struct Client {
    /// `host:port` of the runtime.
    pub addr: String,
    /// The agent's name as the record will show it.
    pub name: String,
    /// The runtime's feed token, if it has one.
    pub token: Option<String>,
    /// The agent's own roster key, if the runtime has a roster with a named agent.
    pub key: Option<String>,
}

impl Client {
    async fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, Value), String> {
        let mut s = TcpStream::connect(&self.addr).await.map_err(|e| format!("cannot reach the PCHI runtime at {}: {e}. Is it running?", self.addr))?;
        let body = body.unwrap_or("");
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Pchi-Actor: agent:{}\r\n", self.addr, body.len(), self.name);
        if let Some(t) = &self.token {
            head += &format!("Authorization: Bearer {t}\r\n");
        }
        if let Some(k) = &self.key {
            head += &format!("X-Pchi-Edit-Key: {k}\r\n");
        }
        s.write_all(format!("{head}\r\n{body}").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(20), s.read_to_end(&mut out)).await.map_err(|_| "the runtime did not answer in time".to_string())?.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out);
        let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("the runtime sent something that is not HTTP")?;
        let (headers, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let rest = if headers.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(rest) } else { rest.to_string() };
        Ok((status, serde_json::from_str(rest.trim()).unwrap_or(Value::Null)))
    }

    /// A request that must succeed, or an error in words.
    async fn ok(&self, method: &str, path: &str, body: Option<&str>) -> Result<Value, String> {
        let (status, v) = self.call(method, path, body).await?;
        match status {
            200 | 202 => Ok(v),
            401 => Err("the runtime wants its token: start pchi-mcp with --token".into()),
            403 => Err(format!("refused: {}", v["error"].as_str().unwrap_or("an agent may not do that"))),
            _ => Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the runtime said {status}"))),
        }
    }
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((len, tail)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        rest = tail[n..].trim_start_matches("\r\n");
    }
    out
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

/// The tools, with what each is for and what it cannot do.
pub fn tools() -> Value {
    let show = json!({"type": "object", "description": "A whole show (the JSON from pchi_show, edited)."});
    json!([
        {"name": "pchi_state", "description": "Where the running show is now: the scene, how long it has been there, the context (including audience votes under 'vote'), timecode, and edits waiting for a person. Read-only.", "inputSchema": schema(json!({}), &[])},
        {"name": "pchi_show", "description": "The running show's full definition (scenes, transitions, timelines, cues) as JSON: the base to edit a copy of. Read-only.", "inputSchema": schema(json!({}), &[])},
        {"name": "pchi_doors_check", "description": "Check the whole show against the rig and each tool, as the operator would before the doors: what will not work, and what to look at. Read-only.", "inputSchema": schema(json!({}), &[])},
        {"name": "pchi_rehearse", "description": "Play an edited show against the run so far on a virtual clock and say what would differ: the path, cues that would no longer fire, scenes that run longer or shorter, what each cue would send. Changes nothing. Use it before proposing.", "inputSchema": schema(json!({"show": show.clone()}), &["show"])},
        {"name": "pchi_propose", "description": "Propose an edited show. It is NOT applied: it waits under 'Waiting for you' for a person to approve or reject it, with your reason and the rehearsal. Returns the proposal's id.", "inputSchema": schema(json!({"show": show, "said": {"type": "string", "description": "Why: what you or the person you work for asked for, in words. It is kept in the record."}}), &["show", "said"])},
        {"name": "pchi_propose_cue_change", "description": "The common small edit, without writing the whole show: move one cue in time and/or change its value. With dry_run true it only reports what would change and how it would have played (no proposal is made). Otherwise it proposes the change for a person to approve.", "inputSchema": schema(json!({
            "scene": {"type": "string"}, "lane": {"type": "string", "description": "The lane id (see pchi_show)."}, "cue": {"type": "string", "description": "The cue id."},
            "at_ms": {"type": "integer", "description": "The new time in the scene, in milliseconds.", "minimum": 0},
            "value": {"type": "string", "description": "The cue's new value (a cue number, a scene name, channels for DMX...)."},
            "said": {"type": "string", "description": "Why, in words. Kept in the record."}, "dry_run": {"type": "boolean"}}), &["scene", "lane", "cue", "said"])},
        {"name": "pchi_proposals", "description": "Every proposal so far and where each stands (pending, approved, rejected, superseded). Read-only.", "inputSchema": schema(json!({}), &[])},
        {"name": "pchi_report", "description": "How the run has gone: cues fired and how late, deliveries that failed, edits made, and who did what. Read-only.", "inputSchema": schema(json!({}), &[])},
        {"name": "pchi_stop_lasers", "description": "The hard stop: disarm every laser and send each tool's safe 'go dark' message. The one thing an agent may do to the live show, because it only makes things safer. A person must arm a laser again.", "inputSchema": schema(json!({}), &[])},
    ])
}

fn text_result(text: String, is_error: bool) -> Value {
    let text = if text.len() > MAX_TEXT { format!("{}\n... (cut: {} characters more)", &text[..text.char_indices().take_while(|(i, _)| *i < MAX_TEXT).last().map_or(0, |(i, c)| i + c.len_utf8())], text.len() - MAX_TEXT) } else { text };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl Client {
    /// Run one tool. The result is text for the agent to read; an `Err` is a tool failure it can read and react to.
    pub async fn run_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        let said = |a: &Value| a["said"].as_str().unwrap_or("").trim().to_string();
        match name {
            "pchi_state" => {
                let s = self.ok("GET", "/state", None).await?;
                Ok(pretty(&json!({"scene": s["state"], "elapsed_ms": s["elapsed_ms"], "context": s["context"], "timecode": s["timecode"], "polls": s["polls"], "waiting_for_a_person": s["proposals"], "output": s["output"], "edit_locked": s["edit_locked"]})))
            }
            "pchi_show" => Ok(pretty(&self.ok("GET", "/state", None).await?["definition"])),
            "pchi_doors_check" => {
                let p = self.ok("GET", "/preflight", None).await?;
                let rows: Vec<Value> = p["checks"].as_array().into_iter().flatten().filter(|c| c["level"] != "ok").cloned().collect();
                Ok(pretty(&json!({"summary": p["summary"], "to_look_at": rows})))
            }
            "pchi_rehearse" => {
                let show = args.get("show").ok_or("give the show to rehearse (pchi_show, edited)")?;
                let r = self.ok("POST", "/rehearse", Some(&show.to_string())).await?;
                Ok(pretty(&json!({"verdict": r["summary"]["verdict"], "differences": r["summary"]["lines"], "recorded_path": r["recorded_path"], "rehearsed_path": r["rehearsed_path"]})))
            }
            "pchi_propose" => {
                let show = args.get("show").ok_or("give the show to propose (pchi_show, edited)")?;
                let why = said(args);
                if why.is_empty() {
                    return Err("say why (the 'said' argument): it is kept in the record next to your proposal".into());
                }
                let r = self.ok("POST", &format!("/show?said={}", enc(&why)), Some(&show.to_string())).await?;
                Ok(pretty(&json!({"proposed": r["proposed"], "id": r["id"], "status": r["status"], "changes": r["diff"]["changes"], "notes": r["diff"]["notes"], "rehearsal": r["proposal"]["rehearsal"], "next": "A person approves or rejects it on the Live tab (Waiting for you). It changes nothing until then."})))
            }
            "pchi_propose_cue_change" => {
                let (scene, lane, cue) = (args["scene"].as_str().unwrap_or(""), args["lane"].as_str().unwrap_or(""), args["cue"].as_str().unwrap_or(""));
                let (at, value) = (args["at_ms"].as_u64(), args["value"].as_str());
                if at.is_none() && value.is_none() {
                    return Err("change something: give at_ms (a new time) and/or value".into());
                }
                let mut show = self.ok("GET", "/state", None).await?["definition"].clone();
                let found = show["states"].as_array_mut().and_then(|ss| ss.iter_mut().find(|s| s["id"] == scene)).and_then(|s| s["timeline"]["lanes"].as_array_mut()).and_then(|ls| ls.iter_mut().find(|l| l["id"] == lane)).and_then(|l| l["cues"].as_array_mut()).and_then(|cs| cs.iter_mut().find(|c| c["id"] == cue));
                let Some(c) = found else { return Err(format!("there is no cue '{cue}' in lane '{lane}' of scene '{scene}': look at pchi_show for the ids")) };
                if let Some(t) = at {
                    c["at_ms"] = json!(t);
                }
                if let Some(v) = value {
                    let key = c["action"]["payload"].as_object().and_then(|p| p.keys().next().cloned()).unwrap_or_else(|| "value".into());
                    c["action"]["payload"][key.as_str()] = json!(v);
                }
                if args["dry_run"].as_bool().unwrap_or(false) {
                    let diff = self.ok("POST", "/show?dry=1", Some(&show.to_string())).await?;
                    let reh = self.ok("POST", "/rehearse", Some(&show.to_string())).await?;
                    return Ok(pretty(&json!({"dry_run": true, "changes": diff["diff"]["changes"], "notes": diff["diff"]["notes"], "rehearsal": {"verdict": reh["summary"]["verdict"], "differences": reh["summary"]["lines"]}, "next": "Nothing was proposed. Call again without dry_run to propose it."})));
                }
                let why = said(args);
                if why.is_empty() {
                    return Err("say why (the 'said' argument): it is kept in the record next to your proposal".into());
                }
                let r = self.ok("POST", &format!("/show?said={}", enc(&why)), Some(&show.to_string())).await?;
                Ok(pretty(&json!({"proposed": r["proposed"], "id": r["id"], "changes": r["diff"]["changes"], "rehearsal": r["proposal"]["rehearsal"], "next": "A person approves or rejects it on the Live tab. It changes nothing until then."})))
            }
            "pchi_proposals" => Ok(pretty(&self.ok("GET", "/proposals", None).await?)),
            "pchi_report" => {
                let r = self.ok("GET", "/report", None).await?;
                Ok(pretty(&json!({"summary": r["summary"], "problems": r["problems"], "edits": r["edits"], "who_did_what": r["decisions"]})))
            }
            "pchi_stop_lasers" => Ok(pretty(&self.ok("POST", "/laser/stop", None).await?)),
            other => Err(format!("there is no tool '{other}'. This server can only read, rehearse, propose and stop lasers; it cannot press GO, arm, approve or edit directly.")),
        }
    }

    /// Handle one JSON-RPC message (a line). `None` for a notification, which gets no answer.
    pub async fn handle(&self, line: &str) -> Option<String> {
        let reply = |id: Value, r: Result<Value, (i64, String)>| -> String {
            match r {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string(),
            }
        };
        let Ok(msg) = serde_json::from_str::<Value>(line) else { return Some(reply(Value::Null, Err((-32700, "that is not JSON".into())))) };
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or("");
        let Some(id) = id else { return None }; // a notification (notifications/initialized and the like)
        Some(match method {
            "initialize" => {
                let want = msg["params"]["protocolVersion"].as_str().unwrap_or("");
                let version = if PROTOCOLS.contains(&want) { want } else { PROTOCOLS[0] };
                reply(id, Ok(json!({"protocolVersion": version, "capabilities": {"tools": {}}, "serverInfo": {"name": "pchi", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "PCHI runs live shows. You can read the show and the run, rehearse an edit, and propose it for a person to approve. You cannot press GO, arm lasers, or change the live show directly."})))
            }
            "ping" => reply(id, Ok(json!({}))),
            "tools/list" => reply(id, Ok(json!({"tools": tools()}))),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
                let r = self.run_tool(name, &args).await;
                reply(id, Ok(match r { Ok(t) => text_result(t, false), Err(e) => text_result(e, true) }))
            }
            other => reply(id, Err((-32601, format!("method '{other}' is not supported")))),
        })
    }
}

/// Serve MCP on standard input and output until it closes.
pub async fn serve_stdio(client: Client) {
    use tokio::io::{stdin, stdout, AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(stdin()).lines();
    let mut out = stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(r) = client.handle(&line).await {
            let _ = out.write_all(format!("{r}\n").as_bytes()).await;
            let _ = out.flush().await;
        }
    }
}
