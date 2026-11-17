//! A pretend Universal Robots Dashboard Server: the text interface on TCP port 29999 that a UR controller offers for loading, playing,
//! pausing and stopping programs and for reading its state.
//!
//! Written from UR's own manual for the e-Series Dashboard Server (the command table: syntax and reply text). It exists so the
//! runtime's UR driver can be built and tested with no robot, and so a crew can rehearse against something that answers like one.
//!
//! What it is not: a robot. It has no arm, no safety system and no timing. It does not know what a UR program *does*. It answers the
//! commands the manual lists, writes down every command it was sent (so a test can prove what PCHI never sent), and can be made to
//! fail the ways a real controller does: out of Remote Control mode, in a protective stop, refusing `play`, gone.
//! Two details are from memory and not from the manual page: the greeting line sent on connect, and that `programState` may carry the
//! program name after the state. The driver's profile tolerates both.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// What the pretend controller is doing, and the faults it can be put in.
#[derive(Clone, Debug)]
pub struct UrState {
    /// Remote Control mode (the Dashboard Server's program commands are refused without it).
    pub remote: bool,
    /// `robotmode`: RUNNING, IDLE, POWER_OFF, ...
    pub mode: String,
    /// `safetystatus`: NORMAL, PROTECTIVE_STOP, ...
    pub safety: String,
    /// The loaded program's path, if any.
    pub loaded: Option<String>,
    /// STOPPED, PLAYING or PAUSED.
    pub program: &'static str,
    /// The program files that exist on the controller (names without the `.urp`).
    pub programs: BTreeSet<String>,
    /// `play` fails even when it should not (a program that cannot start).
    pub refuse_play: bool,
    /// The controller has gone: connections are closed and new ones refused.
    pub down: bool,
    /// Every command received, in order, exactly as sent (without the newline).
    pub seen: Vec<String>,
}

impl UrState {
    fn new(programs: &[&str]) -> Self {
        UrState { remote: true, mode: "RUNNING".into(), safety: "NORMAL".into(), loaded: None, program: "STOPPED", programs: programs.iter().map(|s| s.to_string()).collect(), refuse_play: false, down: false, seen: Vec::new() }
    }
}

pub struct UrSim {
    pub port: u16,
    state: Arc<Mutex<UrState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for UrSim {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const NOT_REMOTE: &str = "Command is not allowed due to safety reasons, please switch robot to Remote Control mode and reconnect to port 29999.";

/// The reply to one command, and whether to close the connection after it. `None` for a command it does not answer.
fn answer(st: &mut UrState, line: &str) -> (Option<String>, bool) {
    let cmd = line.trim();
    let lower = cmd.to_ascii_lowercase();
    let remote_only = |st: &UrState| !st.remote;
    let r = |s: &str| (Some(s.to_string()), false);
    if lower == "quit" {
        return (Some("Disconnected".into()), true);
    }
    if lower == "is in remote control" {
        return r(if st.remote { "true" } else { "false" });
    }
    if lower == "get loaded program" {
        return match &st.loaded {
            Some(p) => (Some(format!("Loaded program: {p}")), false),
            None => r("No program loaded"),
        };
    }
    if lower == "running" {
        return (Some(format!("Program running: {}", st.program == "PLAYING")), false);
    }
    if lower == "programstate" {
        return (Some(match &st.loaded {
            Some(p) => format!("{} {}", st.program, p.rsplit('/').next().unwrap_or(p)),
            None => st.program.to_string(),
        }), false);
    }
    if lower == "robotmode" {
        return (Some(format!("Robotmode: {}", st.mode)), false);
    }
    if lower == "safetystatus" {
        return (Some(format!("Safetystatus: {}", st.safety)), false);
    }
    if lower.starts_with("load ") {
        if remote_only(st) {
            return r(NOT_REMOTE);
        }
        let name = cmd[5..].trim();
        let stem = name.strip_suffix(".urp").unwrap_or(name);
        if st.programs.contains(stem) {
            st.loaded = Some(format!("/programs/{stem}.urp"));
            st.program = "STOPPED";
            return (Some(format!("Loading program: {name}")), false);
        }
        return (Some(format!("File not found: {name}")), false);
    }
    if lower == "play" {
        if remote_only(st) {
            return r(NOT_REMOTE);
        }
        if st.loaded.is_some() && st.safety == "NORMAL" && st.mode == "RUNNING" && !st.refuse_play {
            st.program = "PLAYING";
            return r("Starting program");
        }
        return r("Failed to execute: play");
    }
    if lower == "pause" {
        if remote_only(st) {
            return r(NOT_REMOTE);
        }
        if st.program == "PLAYING" {
            st.program = "PAUSED";
            return r("Pausing program");
        }
        return r("Failed to execute: pause");
    }
    if lower == "stop" {
        if remote_only(st) {
            return r(NOT_REMOTE);
        }
        if st.program != "STOPPED" {
            st.program = "STOPPED";
            return r("Stopped");
        }
        return r("Failed to execute: stop");
    }
    // anything else (power on, brake release, unlock protective stop, ...): a real controller would act on it. The pretend one only
    // writes it down, which is the whole point: a test can see that PCHI never sent it.
    (Some(format!("pretend UR does not act on: {cmd}")), false)
}

impl UrSim {
    /// Start on a free loopback port with these program files "on the controller".
    pub async fn start(programs: &[&str]) -> Result<UrSim, String> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("cannot open a port for the pretend UR: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let state = Arc::new(Mutex::new(UrState::new(programs)));
        let st = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { continue };
                if st.lock().unwrap().down {
                    drop(sock); // a controller that is gone does not answer
                    continue;
                }
                let st = Arc::clone(&st);
                tokio::spawn(async move {
                    let (rd, mut wr) = sock.into_split();
                    let _ = wr.write_all(b"Connected: Universal Robots Dashboard Server\n").await;
                    let mut lines = BufReader::new(rd).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let (reply, close) = {
                            let mut g = st.lock().unwrap();
                            if g.down {
                                return;
                            }
                            g.seen.push(line.trim().to_string());
                            answer(&mut g, &line)
                        };
                        if let Some(r) = reply {
                            if wr.write_all(format!("{r}\n").as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        if close {
                            return;
                        }
                    }
                });
            }
        });
        Ok(UrSim { port, state, task })
    }

    /// Change the controller's state (a fault, or a program finishing).
    pub fn with<R>(&self, f: impl FnOnce(&mut UrState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// A copy of what it is doing now.
    pub fn snapshot(&self) -> UrState {
        self.state.lock().unwrap().clone()
    }

    /// Every command received so far.
    pub fn seen(&self) -> Vec<String> {
        self.state.lock().unwrap().seen.clone()
    }

    /// Put it in a named fault, or take it out (`healthy`). The names: `protective-stop`, `local-control`, `refuse-play`, `down`, `healthy`.
    pub fn fault(&self, name: &str) -> Result<(), String> {
        let mut g = self.state.lock().unwrap();
        match name {
            "healthy" => {
                g.remote = true;
                g.safety = "NORMAL".into();
                g.mode = "RUNNING".into();
                g.refuse_play = false;
                g.down = false;
            }
            "protective-stop" => {
                g.safety = "PROTECTIVE_STOP".into();
                g.program = "STOPPED"; // the controller stops the program itself
            }
            "local-control" => g.remote = false,
            "refuse-play" => g.refuse_play = true,
            "down" => g.down = true,
            other => return Err(format!("'{other}' is not a UR fault: healthy, protective-stop, local-control, refuse-play or down")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn ask(sim: &UrSim, cmds: &[&str]) -> Vec<String> {
        let s = tokio::net::TcpStream::connect(("127.0.0.1", sim.port)).await.unwrap();
        let (rd, mut wr) = s.into_split();
        let mut lines = BufReader::new(rd).lines();
        let greeting = lines.next_line().await.unwrap().unwrap();
        assert!(greeting.starts_with("Connected"), "{greeting}");
        let mut out = Vec::new();
        for c in cmds {
            wr.write_all(format!("{c}\n").as_bytes()).await.unwrap();
            out.push(lines.next_line().await.unwrap().unwrap());
        }
        out
    }

    #[tokio::test]
    async fn it_answers_the_manuals_commands_with_the_manuals_words() {
        let sim = UrSim::start(&["OPEN_ARM"]).await.unwrap();
        let r = ask(&sim, &["is in remote control", "get loaded program", "play", "load OPEN_ARM.urp", "get loaded program", "play", "programState", "pause", "stop", "stop", "load NOPE.urp"]).await;
        assert_eq!(r[0], "true");
        assert_eq!(r[1], "No program loaded");
        assert_eq!(r[2], "Failed to execute: play");
        assert_eq!(r[3], "Loading program: OPEN_ARM.urp");
        assert_eq!(r[4], "Loaded program: /programs/OPEN_ARM.urp");
        assert_eq!(r[5], "Starting program");
        assert!(r[6].starts_with("PLAYING"), "{}", r[6]);
        assert_eq!(r[7], "Pausing program");
        assert_eq!(r[8], "Stopped");
        assert_eq!(r[9], "Failed to execute: stop");
        assert_eq!(r[10], "File not found: NOPE.urp");
    }

    #[tokio::test]
    async fn the_faults_do_what_a_controller_does_and_every_command_is_written_down() {
        let sim = UrSim::start(&["A"]).await.unwrap();
        sim.fault("local-control").unwrap();
        let r = ask(&sim, &["is in remote control", "load A.urp", "play"]).await;
        assert_eq!(r[0], "false");
        assert!(r[1].contains("Remote Control") && r[2].contains("Remote Control"), "{r:?}");
        sim.fault("healthy").unwrap();
        sim.fault("protective-stop").unwrap();
        assert_eq!(ask(&sim, &["safetystatus"]).await[0], "Safetystatus: PROTECTIVE_STOP");
        sim.fault("healthy").unwrap();
        ask(&sim, &["load A.urp"]).await;
        sim.fault("refuse-play").unwrap();
        assert_eq!(ask(&sim, &["play"]).await[0], "Failed to execute: play");
        // a command PCHI must never send is answered by nothing real, and recorded
        assert!(ask(&sim, &["power on"]).await[0].contains("does not act on"));
        assert!(sim.seen().contains(&"power on".to_string()));
        assert!(sim.fault("nonsense").is_err());
    }
}
