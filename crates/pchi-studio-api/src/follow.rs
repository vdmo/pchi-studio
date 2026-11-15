//! Follow the show: a fixed address that always leads to whichever runtime is running it.
//!
//! Studio follows a takeover by itself (docs/failover.md), but a button box, a gateway or a script has one address in its settings. `pchi-follow` listens on
//! that address, asks each runtime every half second which of them is running the show (`GET /role`), and passes connections (HTTP and WebSocket alike,
//! it moves bytes) to that one. When the show moves, the connections it was carrying are closed so their owners reconnect to the new one.
//!
//! The rule is the same one the runtimes use: only a runtime that says it is `active` is the show; if two say so (a cut network, before the old one has been
//! fenced) the one on the later takeover wins; if none does, connections are refused until one does (a few seconds during a takeover).

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What one runtime said about itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub addr: String,
    /// "active", "standby", "fenced", "paused", or "" if it did not answer.
    pub role: String,
    pub epoch: u64,
}

/// Which of the runtimes seen is the show: its index, and whether two claimed it.
pub fn choose(seen: &[Seen]) -> (Option<usize>, bool) {
    let active: Vec<usize> = (0..seen.len()).filter(|&i| seen[i].role == "active").collect();
    let best = active.iter().copied().max_by_key(|&i| (seen[i].epoch, std::cmp::Reverse(i)));
    (best, active.len() > 1)
}

/// What the follower knows now.
#[derive(Default)]
pub struct View {
    pub seen: Vec<Seen>,
    pub target: Option<usize>,
    pub ambiguous: bool,
    /// How many times the show has moved while this ran.
    pub moves: u64,
}

impl View {
    pub fn json(&self) -> Value {
        json!({"target": self.target.map(|i| self.seen[i].addr.clone()), "ambiguous": self.ambiguous, "moves": self.moves,
               "runtimes": self.seen.iter().map(|s| json!({"addr": s.addr, "role": s.role, "epoch": s.epoch})).collect::<Vec<_>>()})
    }
}

pub struct Follower {
    pub view: Mutex<View>,
    changed: tokio::sync::watch::Sender<u64>,
    pub token: Option<String>,
}

pub async fn look(addr: &str, token: Option<&str>) -> Seen {
    let bearer = token.map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(b) = &bearer {
        headers.push(("Authorization", b));
    }
    match crate::failover::http_t(addr, "GET", "/role", &headers, None, Duration::from_millis(1500)).await {
        Ok((200, v)) => Seen { addr: addr.to_string(), role: v["role"].as_str().unwrap_or("").to_string(), epoch: v["epoch"].as_u64().unwrap_or(0) },
        _ => Seen { addr: addr.to_string(), role: String::new(), epoch: 0 },
    }
}

impl Follower {
    pub fn new(runtimes: &[String], token: Option<String>) -> Arc<Self> {
        let (tx, _rx) = tokio::sync::watch::channel(0u64);
        let seen = runtimes.iter().map(|a| Seen { addr: a.clone(), role: String::new(), epoch: 0 }).collect();
        Arc::new(Follower { view: Mutex::new(View { seen, ..Default::default() }), changed: tx, token })
    }

    /// Look at every runtime once and update which one is the show. Returns true if the show moved.
    pub async fn refresh(&self) -> bool {
        let addrs: Vec<String> = self.view.lock().unwrap().seen.iter().map(|s| s.addr.clone()).collect();
        let mut seen = Vec::new();
        for a in &addrs {
            seen.push(look(a, self.token.as_deref()).await);
        }
        let (target, ambiguous) = choose(&seen);
        let mut v = self.view.lock().unwrap();
        let moved = v.target != target;
        if moved {
            v.moves += 1;
            if let Some(i) = target {
                eprintln!("follow: the show is at {} (it says active, takeover {})", seen[i].addr, seen[i].epoch);
            } else {
                eprintln!("follow: no runtime says it is running the show: refusing connections until one does");
            }
        }
        if ambiguous && !v.ambiguous {
            eprintln!("follow: two runtimes say they are running the show (a cut network, not yet fenced): following the one on the later takeover");
        }
        v.seen = seen;
        v.target = target;
        v.ambiguous = ambiguous;
        let n = v.moves;
        drop(v);
        if moved {
            let _ = self.changed.send(n);
        }
        moved
    }

    pub fn target_addr(&self) -> Option<String> {
        let v = self.view.lock().unwrap();
        v.target.map(|i| v.seen[i].addr.clone())
    }

    /// Ask every `every`, forever.
    pub async fn watch(self: Arc<Self>, every: Duration) {
        loop {
            self.refresh().await;
            tokio::time::sleep(every).await;
        }
    }

    /// Carry one connection to the show, and drop it the moment the show moves.
    pub async fn carry(self: Arc<Self>, mut client: tokio::net::TcpStream) {
        let Some(addr) = self.target_addr() else { return }; // dropped: nothing is running the show
        let mut rx = self.changed.subscribe();
        rx.borrow_and_update();
        let Ok(Ok(mut up)) = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await else { return };
        tokio::select! {
            _ = tokio::io::copy_bidirectional(&mut client, &mut up) => {}
            _ = rx.changed() => {}
        }
    }

    /// Pass connections from `listener` to the show.
    pub async fn serve(self: Arc<Self>, listener: tokio::net::TcpListener) {
        loop {
            let Ok((c, _)) = listener.accept().await else { continue };
            tokio::spawn(Arc::clone(&self).carry(c));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(addr: &str, role: &str, epoch: u64) -> Seen {
        Seen { addr: addr.into(), role: role.into(), epoch }
    }

    #[test]
    fn the_show_is_the_runtime_that_says_it_is_active() {
        assert_eq!(choose(&[s("a", "active", 0), s("b", "standby", 0)]), (Some(0), false));
        assert_eq!(choose(&[s("a", "", 0), s("b", "active", 1)]), (Some(1), false));
        assert_eq!(choose(&[s("a", "fenced", 1), s("b", "active", 1)]), (Some(1), false));
    }

    #[test]
    fn nobody_active_means_nobody_is_followed_and_two_active_means_the_later_takeover() {
        assert_eq!(choose(&[s("a", "paused", 0), s("b", "standby", 0)]), (None, false));
        assert_eq!(choose(&[s("a", "active", 0), s("b", "active", 1)]), (Some(1), true));
        assert_eq!(choose(&[s("a", "active", 2), s("b", "active", 1)]), (Some(0), true));
    }
}
