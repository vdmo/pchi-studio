//! Ableton Link, listen-only: see which Link peers are on the network and what tempo
//! their session runs at. It never sends a datagram, never joins a session and never
//! drives the show; it only reads.
//!
//! Basis: the wire layout was read from the published Link source (Ableton/link, GPL
//! v2 or later; files `discovery/v1/Messages.hpp`, `discovery/Payload.hpp`,
//! `discovery/NetworkByteStreamSerializable.hpp`, `discovery/IpInterface.hpp`,
//! `link/NodeState.hpp`, `link/Timeline.hpp`, `link/Tempo.hpp`, `link/Beats.hpp`,
//! `link/SessionId.hpp`, `link/StartStopState.hpp`). This file is a fresh reading of the
//! byte layout, not a copy of that code. The tests use bytes built from the same
//! reading, so they show the decoder agrees with that reading and nothing more:
//! no real Link peer (Live, Traktor, Serato, a Link-enabled app) was ever heard.
//!
//! Layout, all integers big-endian:
//! `"_asdp_v" 0x01` | type u8 (1 alive, 2 response, 3 byebye) | ttl u8 | group u16 (0) |
//! ident 8 bytes | entries, each `key u32, size u32, value`. Keys are four ASCII letters:
//! `tmln` = tempo (microseconds per beat, i64), beat origin (micro-beats, i64),
//! time origin (microseconds, i64); `sess` = session id (8 bytes); `stst` = playing
//! (1 byte), beats (i64), timestamp (i64). Other keys are skipped. A datagram is
//! under 512 bytes. Peers announce on multicast 224.76.78.75, port 20808, and answer
//! an alive with a unicast response.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const GROUP: Ipv4Addr = Ipv4Addr::new(224, 76, 78, 75);
pub const PORT: u16 = 20808;
const HEADER: [u8; 8] = *b"_asdp_v\x01";
const MAX_DATAGRAM: usize = 512;
/// A spoofed sender could invent endless identities; remember at most this many.
const MAX_PEERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Alive,
    Response,
    ByeBye,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub kind: Kind,
    pub ttl: u8,
    pub ident: [u8; 8],
    pub session: Option<[u8; 8]>,
    pub bpm: Option<f64>,
    pub playing: Option<bool>,
}

fn be_i64(b: &[u8]) -> i64 {
    i64::from_be_bytes(b[..8].try_into().expect("eight bytes"))
}

/// Read one datagram. Anything that does not match the layout is refused, never guessed at.
pub fn decode(d: &[u8]) -> Result<Message, String> {
    if d.len() >= MAX_DATAGRAM {
        return Err("datagram too large for Link".into());
    }
    if d.len() < 20 || d[..8] != HEADER {
        return Err("not a Link datagram".into());
    }
    let kind = match d[8] {
        1 => Kind::Alive,
        2 => Kind::Response,
        3 => Kind::ByeBye,
        t => return Err(format!("unknown Link message type {t}")),
    };
    let ttl = d[9];
    let ident: [u8; 8] = d[12..20].try_into().expect("eight bytes");
    let (mut session, mut bpm, mut playing) = (None, None, None);
    let mut at = 20;
    while at < d.len() {
        if at + 8 > d.len() {
            return Err("payload entry cut short".into());
        }
        let key = &d[at..at + 4];
        let size = u32::from_be_bytes(d[at + 4..at + 8].try_into().expect("four bytes")) as usize;
        let start = at + 8;
        let end = start.checked_add(size).filter(|e| *e <= d.len()).ok_or("payload entry larger than the datagram")?;
        let v = &d[start..end];
        match key {
            b"tmln" => {
                if size != 24 {
                    return Err("timeline entry is not 24 bytes".into());
                }
                let micros_per_beat = be_i64(v);
                if micros_per_beat <= 0 {
                    return Err("timeline tempo is not positive".into());
                }
                bpm = Some(60_000_000.0 / micros_per_beat as f64);
            }
            b"sess" => {
                if size != 8 {
                    return Err("session entry is not 8 bytes".into());
                }
                session = Some(v.try_into().expect("eight bytes"));
            }
            b"stst" => {
                if size != 17 {
                    return Err("start/stop entry is not 17 bytes".into());
                }
                playing = Some(v[0] != 0);
            }
            _ => {}
        }
        at = end;
    }
    Ok(Message { kind, ttl, ident, session, bpm, playing })
}

/// Build a datagram of the same layout. Only the tests and the stand-in use this.
pub fn encode(kind: Kind, ttl: u8, ident: [u8; 8], session: [u8; 8], bpm: f64, playing: bool) -> Vec<u8> {
    let mut o = HEADER.to_vec();
    o.push(match kind {
        Kind::Alive => 1,
        Kind::Response => 2,
        Kind::ByeBye => 3,
    });
    o.push(ttl);
    o.extend_from_slice(&[0, 0]);
    o.extend_from_slice(&ident);
    let mut entry = |key: &[u8; 4], value: &[u8]| {
        o.extend_from_slice(key);
        o.extend_from_slice(&(value.len() as u32).to_be_bytes());
        o.extend_from_slice(value);
    };
    let mut tl = Vec::new();
    tl.extend_from_slice(&((60_000_000.0 / bpm).round() as i64).to_be_bytes());
    tl.extend_from_slice(&0i64.to_be_bytes());
    tl.extend_from_slice(&0i64.to_be_bytes());
    entry(b"tmln", &tl);
    entry(b"sess", &session);
    let mut st = vec![playing as u8];
    st.extend_from_slice(&0i64.to_be_bytes());
    st.extend_from_slice(&0i64.to_be_bytes());
    entry(b"stst", &st);
    o
}

#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub ident: String,
    pub session: Option<String>,
    pub bpm: Option<f64>,
    pub playing: Option<bool>,
    pub from: SocketAddr,
}

fn hex(b: &[u8; 8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Read datagrams from `sock` for `window` and return the peers heard, by identity.
/// A byebye removes a peer. The socket is only ever read from.
pub fn listen_on(sock: &UdpSocket, window: Duration) -> std::io::Result<Vec<Peer>> {
    let until = Instant::now() + window;
    let mut peers: BTreeMap<[u8; 8], Peer> = BTreeMap::new();
    let mut buf = [0u8; 1024];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        sock.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Ok(m) = decode(&buf[..n]) else { continue };
                if m.kind == Kind::ByeBye {
                    peers.remove(&m.ident);
                } else if peers.contains_key(&m.ident) || peers.len() < MAX_PEERS {
                    peers.insert(m.ident, Peer { ident: hex(&m.ident), session: m.session.as_ref().map(hex), bpm: m.bpm, playing: m.playing, from });
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(peers.into_values().collect())
}

/// Join the Link multicast group and listen. Binding shares the port, so this works
/// next to an application that already runs Link on the same machine.
pub fn listen(window: Duration) -> std::io::Result<Vec<Peer>> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    s.set_reuse_port(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    s.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    listen_on(&UdpSocket::from(s), window)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    const SESS: [u8; 8] = [9, 9, 9, 9, 9, 9, 9, 9];

    #[test]
    fn the_bytes_are_exactly_the_layout_read_from_the_source() {
        let d = encode(Kind::Alive, 5, ID, SESS, 120.0, true);
        // header, alive, ttl 5, group 0, ident
        assert_eq!(&d[..20], b"_asdp_v\x01\x01\x05\x00\x00\x01\x02\x03\x04\x05\x06\x07\x08");
        // tmln: size 24, 500000 microseconds per beat = 120 bpm
        assert_eq!(&d[20..28], b"tmln\x00\x00\x00\x18");
        assert_eq!(&d[28..36], &500_000i64.to_be_bytes());
        // sess: size 8
        assert_eq!(&d[52..60], b"sess\x00\x00\x00\x08");
        // stst: size 17, playing
        assert_eq!(&d[68..76], b"stst\x00\x00\x00\x11");
        assert_eq!(d[76], 1);
        assert_eq!(d.len(), 20 + 32 + 16 + 25);
    }

    #[test]
    fn a_datagram_reads_back() {
        let m = decode(&encode(Kind::Response, 5, ID, SESS, 93.5, false)).unwrap();
        assert_eq!(m.kind, Kind::Response);
        assert_eq!(m.ident, ID);
        assert_eq!(m.session, Some(SESS));
        assert!((m.bpm.unwrap() - 93.5).abs() < 0.001);
        assert_eq!(m.playing, Some(false));
    }

    #[test]
    fn unknown_entries_are_skipped_and_the_rest_still_read() {
        let mut d = encode(Kind::Alive, 5, ID, SESS, 100.0, true);
        d.extend_from_slice(b"mep4\x00\x00\x00\x06\xc0\xa8\x00\x07\x4e\x20");
        d.extend_from_slice(b"zzzz\x00\x00\x00\x00");
        assert!((decode(&d).unwrap().bpm.unwrap() - 100.0).abs() < 0.001);
    }

    #[test]
    fn bad_datagrams_are_refused() {
        let good = encode(Kind::Alive, 5, ID, SESS, 120.0, true);
        assert!(decode(b"hello").is_err());
        assert!(decode(&[]).is_err());
        let mut wrong = good.clone();
        wrong[7] = 2; // version
        assert!(decode(&wrong).is_err());
        let mut ty = good.clone();
        ty[8] = 9;
        assert!(decode(&ty).is_err());
        assert!(decode(&good[..good.len() - 3]).is_err(), "cut inside an entry");
        let mut zero = good.clone();
        zero[28..36].copy_from_slice(&0i64.to_be_bytes());
        assert!(decode(&zero).is_err(), "a zero tempo is not a tempo");
        let mut neg = good.clone();
        neg[28..36].copy_from_slice(&(-5i64).to_be_bytes());
        assert!(decode(&neg).is_err());
        let mut size = good.clone();
        size[24..28].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(decode(&size).is_err(), "an entry larger than the datagram");
        let mut short = good.clone();
        short[24..28].copy_from_slice(&16u32.to_be_bytes());
        assert!(decode(&short).is_err(), "a timeline of the wrong size");
        assert!(decode(&vec![0u8; 600]).is_err());
    }

    #[test]
    fn peers_are_listed_and_a_byebye_removes_one() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = rx.local_addr().unwrap();
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let other = [7u8; 8];
        tx.send_to(&encode(Kind::Alive, 5, ID, SESS, 120.0, true), to).unwrap();
        tx.send_to(&encode(Kind::Alive, 5, other, [8; 8], 90.0, false), to).unwrap();
        tx.send_to(b"garbage", to).unwrap();
        tx.send_to(&encode(Kind::ByeBye, 0, other, [8; 8], 90.0, false), to).unwrap();
        let peers = listen_on(&rx, Duration::from_millis(300)).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].ident, "0102030405060708");
        assert_eq!(peers[0].session.as_deref(), Some("0909090909090909"));
        assert!((peers[0].bpm.unwrap() - 120.0).abs() < 0.001);
        assert_eq!(peers[0].playing, Some(true));
    }

    #[test]
    fn an_invented_crowd_of_peers_is_capped() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = rx.local_addr().unwrap();
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        for i in 0..100u8 {
            tx.send_to(&encode(Kind::Alive, 5, [i; 8], SESS, 120.0, true), to).unwrap();
        }
        assert_eq!(listen_on(&rx, Duration::from_millis(300)).unwrap().len(), MAX_PEERS);
    }
}
