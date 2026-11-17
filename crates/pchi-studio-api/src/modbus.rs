//! A minimal Modbus TCP client, for a PLC that sits between PCHI and a machine (the usual way a robot controller is told to start a
//! program: a PLC drives its inputs). Written from the Modbus Application Protocol and Modbus TCP specifications.
//!
//! It speaks the six simplest requests and nothing more: read coils, read discrete inputs, read holding registers, read input registers,
//! write a single coil, write a single register. Addresses are the protocol's own, counting from 0 (not the 40001 style).
//!
//! Commands are written as small lines of text so a link profile can say exactly what is sent, and a person can read it:
//!
//! ```text
//! read_coil 20        read_input 21        read_reg 200        read_ireg 201
//! write_coil 10 1     write_reg 100 3
//! ```
//!
//! A read answers `<address>=<value>` (a coil is 0 or 1). A write answers `OK` once the device has echoed it back, as the specification
//! says it must. A device that refuses answers `EXCEPTION <code> <name>`: that is the device saying no, not a broken link.
//!
//! What this is not: a general Modbus library. No ranges, no multi-register writes, no 32-bit values, no serial. And it knows nothing of
//! what any address means: that is the PLC programmer's contract, and the profile's job to say.

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// Single bits you can write (and read): coils.
    Coil,
    /// Single bits you can only read: discrete inputs.
    Input,
    /// 16-bit values you can write (and read): holding registers.
    Reg,
    /// 16-bit values you can only read: input registers.
    InputReg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Read(Space, u16),
    WriteCoil(u16, bool),
    WriteReg(u16, u16),
}

impl Cmd {
    pub fn is_write(&self) -> bool {
        matches!(self, Cmd::WriteCoil(..) | Cmd::WriteReg(..))
    }

    /// What a write changes: the space (coil or holding register) and the address. None for a read.
    pub fn writes(&self) -> Option<(Space, u16)> {
        match self {
            Cmd::WriteCoil(a, _) => Some((Space::Coil, *a)),
            Cmd::WriteReg(a, _) => Some((Space::Reg, *a)),
            Cmd::Read(..) => None,
        }
    }
}

/// Read a command line: `read_coil 20`, `write_reg 100 3`, ...
pub fn parse(cmd: &str) -> Result<Cmd, String> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let num = |s: &str, what: &str| s.parse::<u16>().map_err(|_| format!("'{s}' is not {what} (a whole number 0 to 65535)"));
    match parts.as_slice() {
        ["read_coil", a] => Ok(Cmd::Read(Space::Coil, num(a, "an address")?)),
        ["read_input", a] => Ok(Cmd::Read(Space::Input, num(a, "an address")?)),
        ["read_reg", a] => Ok(Cmd::Read(Space::Reg, num(a, "an address")?)),
        ["read_ireg", a] => Ok(Cmd::Read(Space::InputReg, num(a, "an address")?)),
        ["write_coil", a, v] => match *v {
            "0" => Ok(Cmd::WriteCoil(num(a, "an address")?, false)),
            "1" => Ok(Cmd::WriteCoil(num(a, "an address")?, true)),
            other => Err(format!("a coil is written 0 or 1, not '{other}'")),
        },
        ["write_reg", a, v] => Ok(Cmd::WriteReg(num(a, "an address")?, num(v, "a value")?)),
        _ => Err(format!("'{cmd}' is not a Modbus command PCHI knows: read_coil, read_input, read_reg, read_ireg <address>, write_coil <address> <0|1>, write_reg <address> <value>")),
    }
}

/// An address range a profile promises never to write, such as `coil:30-39` or `reg:100`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forbidden {
    pub space: Space,
    pub lo: u16,
    pub hi: u16,
}

pub fn parse_forbidden(s: &str) -> Result<Forbidden, String> {
    let (sp, range) = s.trim().split_once(':').ok_or_else(|| format!("'{s}' must look like coil:30-39 or reg:100"))?;
    let space = match sp.trim() {
        "coil" => Space::Coil,
        "reg" => Space::Reg,
        other => return Err(format!("'{other}' in '{s}': only coil and reg can be written")),
    };
    let n = |x: &str| x.trim().parse::<u16>().map_err(|_| format!("'{x}' in '{s}' is not an address"));
    let (lo, hi) = match range.split_once('-') {
        Some((a, b)) => (n(a)?, n(b)?),
        None => (n(range)?, n(range)?),
    };
    if lo > hi {
        return Err(format!("'{s}': the range runs backwards"));
    }
    Ok(Forbidden { space, lo, hi })
}

impl Forbidden {
    pub fn covers(&self, cmd: &Cmd) -> bool {
        cmd.writes().is_some_and(|(sp, a)| sp == self.space && a >= self.lo && a <= self.hi)
    }
}

fn exception_name(code: u8) -> &'static str {
    match code {
        1 => "illegal function",
        2 => "illegal data address",
        3 => "illegal data value",
        4 => "device failure",
        5 => "acknowledge",
        6 => "device busy",
        8 => "memory parity error",
        10 => "gateway path unavailable",
        11 => "gateway target failed to respond",
        _ => "exception",
    }
}

/// The request's PDU (function code and data).
pub fn pdu(cmd: &Cmd) -> Vec<u8> {
    let (f, a, v): (u8, u16, u16) = match cmd {
        Cmd::Read(Space::Coil, a) => (1, *a, 1),
        Cmd::Read(Space::Input, a) => (2, *a, 1),
        Cmd::Read(Space::Reg, a) => (3, *a, 1),
        Cmd::Read(Space::InputReg, a) => (4, *a, 1),
        Cmd::WriteCoil(a, on) => (5, *a, if *on { 0xFF00 } else { 0 }),
        Cmd::WriteReg(a, v) => (6, *a, *v),
    };
    let mut out = vec![f];
    out.extend(a.to_be_bytes());
    out.extend(v.to_be_bytes());
    out
}

/// The whole request frame: the Modbus TCP header (transaction id, protocol 0, length, unit) and the PDU.
pub fn frame(txid: u16, unit: u8, cmd: &Cmd) -> Vec<u8> {
    let p = pdu(cmd);
    let mut out = Vec::with_capacity(7 + p.len());
    out.extend(txid.to_be_bytes());
    out.extend(0u16.to_be_bytes());
    out.extend(((p.len() + 1) as u16).to_be_bytes());
    out.push(unit);
    out.extend(p);
    out
}

/// What a response PDU means for `cmd`, as the text a profile reads. `Err` is a reply that is not a valid answer to this request.
pub fn render_reply(cmd: &Cmd, pdu: &[u8]) -> Result<String, String> {
    let want = pdu_function(cmd);
    let Some(&f) = pdu.first() else { return Err("an empty reply".into()) };
    if f == want | 0x80 {
        let code = pdu.get(1).copied().unwrap_or(0);
        return Ok(format!("EXCEPTION {code} {}", exception_name(code)));
    }
    if f != want {
        return Err(format!("the reply is to function {f}, not {want}"));
    }
    match cmd {
        Cmd::Read(Space::Coil | Space::Input, a) => {
            if pdu.len() < 3 || pdu[1] != 1 {
                return Err("a bit read answered with the wrong length".into());
            }
            Ok(format!("{a}={}", pdu[2] & 1))
        }
        Cmd::Read(Space::Reg | Space::InputReg, a) => {
            if pdu.len() < 4 || pdu[1] != 2 {
                return Err("a register read answered with the wrong length".into());
            }
            Ok(format!("{a}={}", u16::from_be_bytes([pdu[2], pdu[3]])))
        }
        Cmd::WriteCoil(..) | Cmd::WriteReg(..) => {
            // the specification says a write is answered by echoing the request: anything else is not a confirmed write
            if pdu == pdu_of(cmd).as_slice() {
                Ok("OK".into())
            } else {
                Err("the device did not echo the write back, so it is not confirmed".into())
            }
        }
    }
}

fn pdu_of(cmd: &Cmd) -> Vec<u8> {
    pdu(cmd)
}

fn pdu_function(cmd: &Cmd) -> u8 {
    pdu(cmd)[0]
}

/// Send one command and read its answer, on an open connection. The caller drops the connection on any `Err`.
pub async fn do_command(r: &mut BufReader<TcpStream>, unit: u8, txid: u16, cmd: &Cmd) -> Result<String, String> {
    r.get_mut().write_all(&frame(txid, unit, cmd)).await.map_err(|e| format!("could not send: {e}"))?;
    let mut head = [0u8; 7];
    r.read_exact(&mut head).await.map_err(|e| format!("could not read the reply: {e}"))?;
    let (rtx, proto, len, runit) = (u16::from_be_bytes([head[0], head[1]]), u16::from_be_bytes([head[2], head[3]]), u16::from_be_bytes([head[4], head[5]]) as usize, head[6]);
    if rtx != txid || proto != 0 || runit != unit {
        return Err(format!("the reply is not to this request (transaction {rtx}, protocol {proto}, unit {runit})"));
    }
    if !(2..=260).contains(&len) {
        return Err(format!("a reply of length {len} is not a Modbus reply"));
    }
    let mut body = vec![0u8; len - 1];
    r.read_exact(&mut body).await.map_err(|e| format!("could not read the reply: {e}"))?;
    render_reply(cmd, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_strictly() {
        assert_eq!(parse("read_coil 20").unwrap(), Cmd::Read(Space::Coil, 20));
        assert_eq!(parse("  write_reg   100  3 ").unwrap(), Cmd::WriteReg(100, 3));
        assert_eq!(parse("write_coil 10 1").unwrap(), Cmd::WriteCoil(10, true));
        for bad in ["write_coil 10 2", "write_reg 70000 1", "write_reg 1", "read_reg", "read_coil -1", "write_regs 1 2", "write_reg 1 2 3", "read_reg 1; write_reg 2 3"] {
            assert!(parse(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn frames_follow_the_specification() {
        // function 3, address 200, quantity 1, transaction 7, unit 1
        assert_eq!(frame(7, 1, &Cmd::Read(Space::Reg, 200)), [0, 7, 0, 0, 0, 6, 1, 3, 0, 200, 0, 1]);
        // a coil is switched on with 0xFF00 and off with 0x0000
        assert_eq!(pdu(&Cmd::WriteCoil(10, true)), [5, 0, 10, 0xFF, 0]);
        assert_eq!(pdu(&Cmd::WriteCoil(10, false)), [5, 0, 10, 0, 0]);
        assert_eq!(pdu(&Cmd::WriteReg(100, 258)), [6, 0, 100, 1, 2]);
    }

    #[test]
    fn replies_are_read_as_text_and_a_refusal_is_an_exception_not_an_error() {
        assert_eq!(render_reply(&Cmd::Read(Space::Reg, 200), &[3, 2, 0, 5]).unwrap(), "200=5");
        assert_eq!(render_reply(&Cmd::Read(Space::Coil, 20), &[1, 1, 1]).unwrap(), "20=1");
        assert_eq!(render_reply(&Cmd::WriteReg(100, 3), &[6, 0, 100, 0, 3]).unwrap(), "OK");
        assert_eq!(render_reply(&Cmd::WriteReg(100, 3), &[0x86, 2]).unwrap(), "EXCEPTION 2 illegal data address");
        // a write that is not echoed exactly is not confirmed; a reply to the wrong function is not an answer at all
        assert!(render_reply(&Cmd::WriteReg(100, 3), &[6, 0, 100, 0, 4]).is_err());
        assert!(render_reply(&Cmd::WriteReg(100, 3), &[3, 2, 0, 3]).is_err());
        assert!(render_reply(&Cmd::Read(Space::Reg, 1), &[3, 4, 0, 0]).is_err());
    }

    #[test]
    fn forbidden_addresses_cover_exactly_what_they_say() {
        let f = parse_forbidden("coil:30-39").unwrap();
        assert!(f.covers(&Cmd::WriteCoil(30, true)) && f.covers(&Cmd::WriteCoil(39, false)));
        assert!(!f.covers(&Cmd::WriteCoil(40, true)) && !f.covers(&Cmd::WriteCoil(29, true)));
        assert!(!f.covers(&Cmd::WriteReg(35, 1)), "a register is not a coil");
        assert!(!f.covers(&Cmd::Read(Space::Coil, 35)), "a read is not a write");
        assert!(parse_forbidden("reg:100").unwrap().covers(&Cmd::WriteReg(100, 0)));
        for bad in ["coil", "input:3", "coil:9-3", "coil:x"] {
            assert!(parse_forbidden(bad).is_err(), "{bad}");
        }
    }
}
