//! A pretend PLC that speaks Modbus TCP, with a small made-up "robot program" in it, for building and testing the Modbus link with no hardware.
//!
//! The register map is an INVENTION for tests, in the spirit of what an integrator might write; it is not any real PLC's or any robot's:
//!
//! | address | what |
//! |---|---|
//! | coil 10 | START: a rising edge starts the program numbered in holding register 100 |
//! | coil 11 | STOP: setting it stops the program |
//! | coils 30 to 39 | RESERVED: drives and fault acknowledgement. Nothing from PCHI may ever write these (a test checks it) |
//! | holding register 100 | the program number to start |
//! | discrete input 20 | REMOTE: 1 when it accepts commands from outside |
//! | discrete input 21 | SAFETY OK: 1 when the machine's safety status is fine |
//! | input register 200 | STATE: 0 idle, 1 running, 2 fault |
//! | input register 201 | the active program number |
//! | input register 202 | REJECT: 1 when the last start was refused (not remote, safety fault, unknown program) |
//!
//! It answers like the specification says (a write is echoed; an unknown address is exception 2), and writes down every request in the
//! text form the link profile uses, so a test can prove what was sent, and what was never sent.

use crate::modbus::{Cmd, Space};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const SIZE: usize = 256;

#[derive(Clone, Debug)]
pub struct PlcState {
    pub coils: Vec<bool>,
    pub inputs: Vec<bool>,
    pub regs: Vec<u16>,
    pub iregs: Vec<u16>,
    /// The program numbers its "robot program" knows.
    pub programs: Vec<u16>,
    pub down: bool,
    /// Every request received, in the link's text form (`write_coil 10 1`).
    pub seen: Vec<String>,
}

impl PlcState {
    fn new(programs: &[u16]) -> Self {
        let mut s = PlcState { coils: vec![false; SIZE], inputs: vec![false; SIZE], regs: vec![0; SIZE], iregs: vec![0; SIZE], programs: programs.to_vec(), down: false, seen: Vec::new() };
        s.inputs[20] = true;
        s.inputs[21] = true;
        s
    }

    /// The made-up "robot program": what writing a coil does.
    fn after_write(&mut self, was: bool, cmd: &Cmd) {
        if let Cmd::WriteCoil(10, true) = cmd {
            if !was {
                let n = self.regs[100];
                if self.inputs[20] && self.inputs[21] && self.programs.contains(&n) && self.iregs[200] != 2 {
                    self.iregs[200] = 1;
                    self.iregs[201] = n;
                    self.iregs[202] = 0;
                } else {
                    self.iregs[202] = 1;
                }
            }
        }
        if let Cmd::WriteCoil(11, true) = cmd {
            self.iregs[200] = if self.inputs[21] { 0 } else { 2 };
            self.iregs[201] = 0;
        }
    }
}

pub struct PlcSim {
    pub port: u16,
    state: Arc<Mutex<PlcState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for PlcSim {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn text(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Read(Space::Coil, a) => format!("read_coil {a}"),
        Cmd::Read(Space::Input, a) => format!("read_input {a}"),
        Cmd::Read(Space::Reg, a) => format!("read_reg {a}"),
        Cmd::Read(Space::InputReg, a) => format!("read_ireg {a}"),
        Cmd::WriteCoil(a, v) => format!("write_coil {a} {}", *v as u8),
        Cmd::WriteReg(a, v) => format!("write_reg {a} {v}"),
    }
}

/// Decode a request PDU into a command, or the exception code to answer with.
fn decode(p: &[u8]) -> Result<Cmd, u8> {
    if p.len() < 5 {
        return Err(3);
    }
    let a = u16::from_be_bytes([p[1], p[2]]);
    let v = u16::from_be_bytes([p[3], p[4]]);
    match p[0] {
        1 | 2 | 3 | 4 if v != 1 => Err(3), // only single reads
        1 => Ok(Cmd::Read(Space::Coil, a)),
        2 => Ok(Cmd::Read(Space::Input, a)),
        3 => Ok(Cmd::Read(Space::Reg, a)),
        4 => Ok(Cmd::Read(Space::InputReg, a)),
        5 if v == 0xFF00 => Ok(Cmd::WriteCoil(a, true)),
        5 if v == 0 => Ok(Cmd::WriteCoil(a, false)),
        5 => Err(3),
        6 => Ok(Cmd::WriteReg(a, v)),
        _ => Err(1),
    }
}

impl PlcSim {
    /// Start on a free loopback port, answering to `unit`, whose "robot program" knows these program numbers.
    pub async fn start(unit: u8, programs: &[u16]) -> Result<PlcSim, String> {
        let l = TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("cannot open a port for the pretend PLC: {e}"))?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        let state = Arc::new(Mutex::new(PlcState::new(programs)));
        let st = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else { continue };
                if st.lock().unwrap().down {
                    drop(sock);
                    continue;
                }
                let st = Arc::clone(&st);
                tokio::spawn(async move {
                    loop {
                        let mut head = [0u8; 7];
                        if sock.read_exact(&mut head).await.is_err() {
                            return;
                        }
                        let len = u16::from_be_bytes([head[4], head[5]]) as usize;
                        if !(2..=260).contains(&len) || head[6] != unit {
                            return; // a gateway drops what is not for it
                        }
                        let mut body = vec![0u8; len - 1];
                        if sock.read_exact(&mut body).await.is_err() {
                            return;
                        }
                        let reply_pdu = {
                            let mut g = st.lock().unwrap();
                            if g.down {
                                return;
                            }
                            match decode(&body) {
                                Err(code) => vec![body[0] | 0x80, code],
                                Ok(cmd) => {
                                    g.seen.push(text(&cmd));
                                    let addr = match cmd {
                                        Cmd::Read(_, a) | Cmd::WriteCoil(a, _) | Cmd::WriteReg(a, _) => a as usize,
                                    };
                                    if addr >= SIZE {
                                        vec![body[0] | 0x80, 2]
                                    } else {
                                        match cmd {
                                            Cmd::Read(Space::Coil, a) => vec![1, 1, g.coils[a as usize] as u8],
                                            Cmd::Read(Space::Input, a) => vec![2, 1, g.inputs[a as usize] as u8],
                                            Cmd::Read(Space::Reg, a) => [vec![3, 2], g.regs[a as usize].to_be_bytes().to_vec()].concat(),
                                            Cmd::Read(Space::InputReg, a) => [vec![4, 2], g.iregs[a as usize].to_be_bytes().to_vec()].concat(),
                                            Cmd::WriteCoil(a, v) => {
                                                let was = g.coils[a as usize];
                                                g.coils[a as usize] = v;
                                                g.after_write(was, &cmd);
                                                body.clone()
                                            }
                                            Cmd::WriteReg(a, v) => {
                                                g.regs[a as usize] = v;
                                                body.clone()
                                            }
                                        }
                                    }
                                }
                            }
                        };
                        let mut out = Vec::new();
                        out.extend(&head[0..4]);
                        out.extend(((reply_pdu.len() + 1) as u16).to_be_bytes());
                        out.push(unit);
                        out.extend(reply_pdu);
                        if sock.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Ok(PlcSim { port, state, task })
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut PlcState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    pub fn snapshot(&self) -> PlcState {
        self.state.lock().unwrap().clone()
    }

    pub fn seen(&self) -> Vec<String> {
        self.state.lock().unwrap().seen.clone()
    }

    /// `healthy`, `safety-fault`, `not-remote` or `down`.
    pub fn fault(&self, name: &str) -> Result<(), String> {
        let mut g = self.state.lock().unwrap();
        match name {
            "healthy" => {
                g.inputs[20] = true;
                g.inputs[21] = true;
                g.down = false;
                if g.iregs[200] == 2 {
                    g.iregs[200] = 0;
                }
            }
            "safety-fault" => {
                g.inputs[21] = false;
                g.iregs[200] = 2;
                g.iregs[201] = 0;
            }
            "not-remote" => g.inputs[20] = false,
            "down" => g.down = true,
            other => return Err(format!("'{other}' is not a pretend PLC fault: healthy, safety-fault, not-remote or down")),
        }
        Ok(())
    }
}
