//! Audience votes: a live poll an online audience can answer, whose result the show can read.
//!
//! A person opens a poll (`POST /polls/<id>/open` with its options), the audience's gateway (a web page, a chat bot) sends each vote
//! to `POST /vote`, and a person closes it. While it runs, the tally is kept in the show's context under `vote`, where a guard can
//! read it (a guard's dotted names walk into objects):
//!
//! ```text
//! vote.next.hype      votes for the option "hype" in the poll "next"
//! vote.next.total     all votes
//! vote.next.winner    the leading option's key ("hype"), or "" while it is a tie or nobody has voted
//! vote.next.open      true while it runs
//! ```
//!
//! So a transition can wait for the room: `DROP`, guarded by `vote.next.winner == "hype"`.
//!
//! What it is, and is not: the audience can only add votes. Its token opens `POST /vote` and nothing else (it cannot read the show,
//! press GO, open or close a poll, or edit). A vote is one voter id, one vote per poll (a later vote replaces the earlier one).
//! The runtime cannot tell whether the voter ids are real people: the gateway that holds the token is trusted to give each person
//! one id, so ballot stuffing is the gateway's problem to prevent. Free-text chat is not accepted at all.

use std::collections::{BTreeMap, HashMap};

const MAX_POLLS: usize = 8;
const MAX_OPTIONS: usize = 10;
const MAX_VOTERS: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Tally {
    pub id: String,
    pub open: bool,
    /// (option, votes), in the order the options were given.
    pub counts: Vec<(String, u32)>,
    pub total: u32,
    pub winner: Option<String>,
}

struct Poll {
    options: Vec<String>,
    open: bool,
    ballots: HashMap<String, usize>,
}

#[derive(Default)]
pub struct Votes {
    polls: BTreeMap<String, Poll>,
}

/// The key an option has in the show's context: lower case, spaces and odd characters as `_`.
pub fn key_of(option: &str) -> String {
    option.trim().to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

pub fn poll_id_ok(id: &str) -> bool {
    // no '-': a poll's name is part of a guard's path (vote.<poll>.winner), where '-' would read as a minus
    !id.is_empty() && id.len() <= 24 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn voter_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || "_.:@-".contains(c))
}

impl Votes {
    /// Open a poll (or re-open a closed one, starting over).
    pub fn open(&mut self, id: &str, options: &[String]) -> Result<Tally, String> {
        if !poll_id_ok(id) {
            return Err(format!("'{id}' is not a poll name: lower case letters, digits and _, up to 24"));
        }
        if self.polls.get(id).is_some_and(|p| p.open) {
            return Err(format!("the poll '{id}' is already open: close it first"));
        }
        if !self.polls.contains_key(id) && self.polls.len() >= MAX_POLLS {
            return Err(format!("there are already {MAX_POLLS} polls: reuse a name"));
        }
        if options.len() < 2 || options.len() > MAX_OPTIONS {
            return Err(format!("a poll has 2 to {MAX_OPTIONS} options"));
        }
        let mut seen = Vec::new();
        for o in options {
            let k = key_of(o);
            if o.trim().is_empty() || o.len() > 32 || o.chars().any(|c| c.is_control()) || k.chars().all(|c| c == '_') {
                return Err(format!("'{o}' is not a usable option: up to 32 characters, with some letters or digits"));
            }
            if seen.contains(&k) {
                return Err(format!("two options read as '{k}'"));
            }
            seen.push(k);
        }
        let opts: Vec<String> = options.iter().map(|o| o.trim().to_string()).collect();
        self.polls.insert(id.to_string(), Poll { options: opts, open: true, ballots: HashMap::new() });
        Ok(self.tally(id).expect("just opened"))
    }

    pub fn close(&mut self, id: &str) -> Result<Tally, String> {
        let p = self.polls.get_mut(id).ok_or_else(|| format!("there is no poll '{id}'"))?;
        if !p.open {
            return Err(format!("the poll '{id}' is already closed"));
        }
        p.open = false;
        Ok(self.tally(id).expect("exists"))
    }

    /// One vote. `option` may be written in any case. A voter's later vote replaces their earlier one.
    pub fn vote(&mut self, id: &str, option: &str, voter: &str) -> Result<Tally, String> {
        if !voter_ok(voter) {
            return Err("a vote needs a voter id: letters, digits and _ . : @ -, up to 64".into());
        }
        let p = self.polls.get_mut(id).ok_or_else(|| format!("there is no poll '{id}'"))?;
        if !p.open {
            return Err(format!("the poll '{id}' is closed"));
        }
        let want = key_of(option);
        let idx = p.options.iter().position(|o| key_of(o) == want).ok_or_else(|| format!("'{option}' is not an option of '{id}' ({})", p.options.join(", ")))?;
        if !p.ballots.contains_key(voter) && p.ballots.len() >= MAX_VOTERS {
            return Err("this poll has all the voters it will take".into());
        }
        p.ballots.insert(voter.to_string(), idx);
        Ok(self.tally(id).expect("exists"))
    }

    pub fn tally(&self, id: &str) -> Option<Tally> {
        let p = self.polls.get(id)?;
        let mut counts: Vec<(String, u32)> = p.options.iter().map(|o| (o.clone(), 0)).collect();
        for &i in p.ballots.values() {
            counts[i].1 += 1;
        }
        let top = counts.iter().map(|c| c.1).max().unwrap_or(0);
        let leaders: Vec<&(String, u32)> = counts.iter().filter(|c| c.1 == top).collect();
        let winner = (top > 0 && leaders.len() == 1).then(|| leaders[0].0.clone());
        Some(Tally { id: id.to_string(), open: p.open, total: counts.iter().map(|c| c.1).sum(), counts, winner })
    }

    pub fn all(&self) -> Vec<Tally> {
        self.polls.keys().filter_map(|id| self.tally(id)).collect()
    }
}

pub fn tally_json(t: &Tally) -> serde_json::Value {
    serde_json::json!({
        "type": "poll", "id": t.id, "open": t.open, "total": t.total, "winner": t.winner,
        "options": t.counts.iter().map(|(o, n)| serde_json::json!({"option": o, "votes": n})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(o: &[&str]) -> Vec<String> {
        o.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_poll_counts_one_vote_per_voter_and_names_a_winner_only_when_there_is_one() {
        let mut v = Votes::default();
        let t = v.open("next", &opts(&["Calm", "Hype", "Weird"])).unwrap();
        assert_eq!((t.total, t.winner), (0, None));
        v.vote("next", "hype", "ann").unwrap();
        assert_eq!(v.tally("next").unwrap().winner.as_deref(), Some("Hype"));
        v.vote("next", "CALM", "bob").unwrap();
        assert_eq!(v.tally("next").unwrap().winner, None, "a tie has no winner");
        v.vote("next", "calm", "cy").unwrap();
        assert_eq!(v.tally("next").unwrap().winner.as_deref(), Some("Calm"));
        // a voter changing their mind is one vote, not two
        v.vote("next", "hype", "bob").unwrap();
        let t = v.vote("next", "hype", "cy").unwrap();
        assert_eq!((t.total, t.counts.clone()), (3, vec![("Calm".into(), 0), ("Hype".into(), 3), ("Weird".into(), 0)]));
        assert_eq!(t.winner.as_deref(), Some("Hype"));
    }

    #[test]
    fn a_poll_refuses_what_it_should() {
        let mut v = Votes::default();
        let e = |r: Result<Tally, String>| r.err().unwrap_or_default();
        assert!(e(v.open("Bad Name", &opts(&["a", "b"]))).contains("not a poll name"));
        assert!(e(v.open("p", &opts(&["only"]))).contains("2 to 10 options"));
        assert!(e(v.open("p", &opts(&["a", "A "]))).contains("two options read as 'a'"));
        assert!(e(v.open("p", &opts(&["a", "!!!"]))).contains("not a usable option"));
        v.open("p", &opts(&["a", "b"])).unwrap();
        assert!(e(v.open("p", &opts(&["a", "b"]))).contains("already open"));
        assert!(e(v.vote("p", "zzz", "x")).contains("not an option of 'p' (a, b)"));
        assert!(e(v.vote("p", "a", "bad voter!")).contains("needs a voter id"));
        assert!(e(v.vote("nope", "a", "x")).contains("no poll 'nope'"));
        v.vote("p", "a", "x").unwrap();
        v.close("p").unwrap();
        assert!(e(v.vote("p", "a", "y")).contains("closed"), "no votes after it closes");
        assert!(e(v.close("p")).contains("already closed"));
        // closed polls keep their result, and a poll may be opened again fresh
        assert_eq!(v.tally("p").unwrap().winner.as_deref(), Some("a"));
        assert_eq!(v.open("p", &opts(&["a", "b"])).unwrap().total, 0);
        // there is a limit to how many polls and voters
        let mut v = Votes::default();
        for i in 0..MAX_POLLS {
            v.open(&format!("p{i}"), &opts(&["a", "b"])).unwrap();
        }
        assert!(e(v.open("onemore", &opts(&["a", "b"]))).contains("already 8 polls"));
        for i in 0..MAX_VOTERS {
            v.vote("p0", "a", &format!("v{i}")).unwrap();
        }
        assert!(e(v.vote("p0", "a", "one-more")).contains("all the voters"));
        assert!(v.vote("p0", "b", "v1").is_ok(), "but someone already in may still change their vote");
    }

    #[test]
    fn options_have_context_keys() {
        assert_eq!(key_of("Big Drop!"), "big_drop_");
        assert_eq!(key_of(" hype "), "hype");
    }
}
