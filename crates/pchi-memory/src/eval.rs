//! Does memory find the right thing, and does it keep quiet when it should? Measured, with no model and no network.
//!
//! A **golden set** is a list of memories (each with a `key`) and a list of questions, each saying which key(s) should come back, or that nothing should
//! (`"abstain": true`). For every question this measures, separately:
//!
//! * **recall@1, @3, @5 and MRR** of each way of searching (keyword, vector, hybrid), over the answerable questions;
//! * what the shipped `recall` does: answered **correctly**, answered **wrongly** (the worst outcome: confident and wrong), or **abstained** on a question that had
//!   an answer; and, over the unanswerable questions, abstained **correctly** or answered **wrongly**.
//!
//! The idea (a recall gate you can fail a build on, and a comparison of retrieval strategies with no API cost) is the one `pgrag-eval-lite` uses for document
//! chunks; this is the same discipline pointed at a show's memory. A golden set is only as good as whoever wrote it: write yours from your own shows.

use crate::{search, Kind, Memory, New, RecallOpts, Who};
use serde_json::{json, Value};

/// The synthetic sample that ships with the crate.
pub const SAMPLE: &str = include_str!("../golden/sample.json");

#[derive(Debug, Clone)]
pub struct Rates {
    pub at1: f64,
    pub at3: f64,
    pub at5: f64,
    pub mrr: f64,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub answerable: usize,
    pub unanswerable: usize,
    pub keyword: Rates,
    pub vector: Rates,
    pub hybrid: Rates,
    /// Of the answerable questions: shipped recall answered with the right memory first / with the wrong ones / said it did not know.
    pub correct: usize,
    pub wrong: usize,
    pub missed: usize,
    /// Of the unanswerable: kept quiet / answered anyway.
    pub quiet: usize,
    pub answered_anyway: usize,
    /// Questions the hybrid search missed at 5, and where the shipped recall went wrong: for a person to read.
    pub misses: Vec<String>,
    /// Recall by how many of the question's words the right memory shares: where the words differ is where a search earns its keep.
    pub buckets: Vec<Bucket>,
    /// Things wrong with the golden set itself (too small, too easy, a memory never asked about).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Bucket {
    pub name: &'static str,
    pub n: usize,
    pub keyword: f64,
    pub vector: f64,
    pub hybrid: f64,
}

fn rates(hits: &[Option<usize>]) -> Rates {
    let n = hits.len().max(1) as f64;
    let at = |k: usize| hits.iter().filter(|h| h.is_some_and(|r| r < k)).count() as f64 / n;
    Rates { at1: at(1), at3: at(3), at5: at(5), mrr: hits.iter().map(|h| h.map_or(0.0, |r| 1.0 / (r as f64 + 1.0))).sum::<f64>() / n }
}

/// Run a golden set (JSON: `memories` and `questions`).
pub fn run(golden: &Value) -> Result<Report, String> {
    let mem = Memory::in_memory().map_err(|e| e.0)?;
    let who = Who::person("eval");
    let mut by_key: std::collections::BTreeMap<String, String> = Default::default();
    for m in golden["memories"].as_array().ok_or("the golden set needs a `memories` list")? {
        let (key, text) = (m["key"].as_str().ok_or("each memory needs a `key`")?, m["text"].as_str().ok_or("each memory needs `text`")?);
        let kind = m["kind"].as_str().and_then(Kind::parse).unwrap_or(Kind::Convention);
        let mut n = New::new(kind, text);
        n.verify = m["verified"].as_bool().unwrap_or(true);
        n.dept = m["dept"].as_str().unwrap_or("").to_string();
        let (r, _) = mem.remember(&who, n, 1_800_000_000_000).map_err(|e| e.0)?;
        by_key.insert(key.to_string(), r.id);
    }
    let pool = mem.list(&Default::default()).map_err(|e| e.0)?;
    let docs: Vec<search::Doc> = pool.iter().map(|r| search::Doc::new(&format!("{} {} {} {}", r.text, r.key.clone().unwrap_or_default(), r.kind.name(), r.dept))).collect();
    let (mut kw, mut ve, mut hy) = (Vec::new(), Vec::new(), Vec::new());
    let (mut correct, mut wrong, mut missed, mut quiet, mut anyway, mut unanswerable) = (0, 0, 0, 0, 0, 0);
    let mut overlaps: Vec<f64> = Vec::new();
    let mut misses: Vec<String> = Vec::new();
    for q in golden["questions"].as_array().ok_or("the golden set needs a `questions` list")? {
        let text = q["q"].as_str().ok_or("each question needs `q`")?;
        let abstain = q["abstain"].as_bool().unwrap_or(false);
        let r = mem.recall(text, &RecallOpts { reinforce: false, ..Default::default() }, 1_800_000_000_000).map_err(|e| e.0)?;
        if abstain {
            unanswerable += 1;
            if r.abstained.is_some() {
                quiet += 1;
            } else {
                anyway += 1;
                misses.push(format!("should not have answered \"{text}\": it said \"{}\"", r.hits[0].record.text));
            }
            continue;
        }
        let expect: Vec<&String> = q["expect"].as_array().ok_or("an answerable question needs `expect`")?.iter().filter_map(|k| k.as_str()).map(|k| by_key.get(k).ok_or(format!("question \"{text}\" expects an unknown key '{k}'"))).collect::<Result<_, _>>()?;
        let pos = |order: &[usize]| order.iter().position(|&i| expect.contains(&&pool[i].id));
        let shared = expect.iter().filter_map(|id| pool.iter().position(|r| &r.id == *id)).map(|i| overlap(text, &docs[i].toks)).fold(0.0, f64::max);
        overlaps.push(shared);
        let (a, b, c) = search::rankings(&docs, text);
        kw.push(pos(&a));
        ve.push(pos(&b));
        let h = pos(&c);
        if !h.is_some_and(|r| r < 5) {
            misses.push(format!("the hybrid search did not find an answer to \"{text}\" in its top 5"));
        }
        hy.push(h);
        match (&r.abstained, r.hits.first()) {
            (None, Some(top)) if expect.contains(&&top.record.id) => correct += 1,
            (None, Some(top)) => {
                wrong += 1;
                misses.push(format!("answered \"{text}\" with the wrong memory: \"{}\"", top.record.text));
            }
            _ => missed += 1,
        }
    }
    let bucket = |name: &'static str, lo: f64, hi: f64| {
        let idx: Vec<usize> = (0..overlaps.len()).filter(|&i| overlaps[i] >= lo && overlaps[i] < hi).collect();
        let at5 = |v: &[Option<usize>]| if idx.is_empty() { 0.0 } else { idx.iter().filter(|&&i| v[i].is_some_and(|r| r < 5)).count() as f64 / idx.len() as f64 };
        Bucket { name, n: idx.len(), keyword: at5(&kw), vector: at5(&ve), hybrid: at5(&hy) }
    };
    let buckets = vec![bucket("shares most of its words with the answer (60 % or more)", 0.6, 1.01), bucket("shares some (30 to 60 %)", 0.3, 0.6), bucket("shares few or none (under 30 %)", 0.0, 0.3)];
    Ok(Report { answerable: kw.len(), unanswerable, keyword: rates(&kw), vector: rates(&ve), hybrid: rates(&hy), correct, wrong, missed, quiet, answered_anyway: anyway, misses, buckets, warnings: lint(golden) })
}

/// The share of the question's distinct content words that appear (allowing for endings and typos) in the memory's words.
pub fn overlap(question: &str, memory_toks: &[String]) -> f64 {
    let mut q = search::tokens(question);
    q.sort();
    q.dedup();
    if q.is_empty() {
        return 0.0;
    }
    q.iter().filter(|t| memory_toks.iter().any(|m| search::same_word(m, t))).count() as f64 / q.len() as f64
}

/// What is wrong with the golden set itself. A set that is small, or whose questions copy the wording of their answers, says little about recall.
pub fn lint(golden: &Value) -> Vec<String> {
    let mut w = Vec::new();
    let mems = golden["memories"].as_array().cloned().unwrap_or_default();
    let qs = golden["questions"].as_array().cloned().unwrap_or_default();
    let answerable: Vec<&Value> = qs.iter().filter(|q| q["abstain"].as_bool() != Some(true)).collect();
    let unanswerable = qs.len() - answerable.len();
    if answerable.len() < 30 {
        w.push(format!("only {} answerable question(s): under 30 and a single question moves recall by {:.0} points", answerable.len(), 100.0 / answerable.len().max(1) as f64));
    }
    if unanswerable < 5 {
        w.push(format!("only {unanswerable} question(s) that memory should decline: add at least 5, or its habit of answering things it was never told is not tested"));
    }
    let mut keys: Vec<&str> = mems.iter().filter_map(|m| m["key"].as_str()).collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    if keys.len() != n {
        w.push("two memories share a key: keys must be unique".into());
    }
    let asked: std::collections::BTreeSet<&str> = answerable.iter().flat_map(|q| q["expect"].as_array().into_iter().flatten().filter_map(|k| k.as_str())).collect();
    let never: Vec<&str> = keys.iter().copied().filter(|k| !asked.contains(k)).collect();
    if !never.is_empty() {
        w.push(format!("{} memory(ies) no question expects ({}): they can only ever be wrong answers", never.len(), never.iter().take(5).copied().collect::<Vec<_>>().join(", ")));
    }
    let text_of = |k: &str| mems.iter().find(|m| m["key"] == k).and_then(|m| m["text"].as_str()).unwrap_or("");
    let copied = answerable.iter().filter(|q| q["expect"].as_array().into_iter().flatten().filter_map(|k| k.as_str()).any(|k| overlap(q["q"].as_str().unwrap_or(""), &search::tokens(text_of(k))) >= 0.9)).count();
    if copied * 3 > answerable.len().max(1) {
        w.push(format!("{copied} of {} questions repeat almost every word of their answer: they test little. Ask the way a person would, in other words", answerable.len()));
    }
    let mut seen: Vec<String> = qs.iter().filter_map(|q| q["q"].as_str()).map(|s| s.trim().to_lowercase()).collect();
    let m = seen.len();
    seen.sort();
    seen.dedup();
    if seen.len() != m {
        w.push("the same question is asked twice".into());
    }
    w
}

impl Report {
    pub fn to_json(&self) -> Value {
        let r = |x: &Rates| json!({"recall_at_1": x.at1, "recall_at_3": x.at3, "recall_at_5": x.at5, "mrr": x.mrr});
        json!({"answerable": self.answerable, "unanswerable": self.unanswerable, "keyword": r(&self.keyword), "vector": r(&self.vector), "hybrid": r(&self.hybrid),
               "shipped": {"correct": self.correct, "wrong": self.wrong, "abstained_but_had_answer": self.missed, "kept_quiet": self.quiet, "answered_anyway": self.answered_anyway}, "misses": self.misses, "warnings": self.warnings,
               "buckets": self.buckets.iter().map(|b| json!({"name": b.name, "n": b.n, "keyword": b.keyword, "vector": b.vector, "hybrid": b.hybrid})).collect::<Vec<_>>()})
    }

    pub fn markdown(&self) -> String {
        let row = |n: &str, r: &Rates| format!("| {n} | {:.0}% | {:.0}% | {:.0}% | {:.2} |\n", r.at1 * 100.0, r.at3 * 100.0, r.at5 * 100.0, r.mrr);
        let mut s = format!("# Memory evaluation\n\n{} answerable question(s), {} that memory should decline.\n\n", self.answerable, self.unanswerable);
        if !self.warnings.is_empty() {
            s += "**About this golden set** (read before trusting the numbers):\n\n";
            for w in &self.warnings {
                s += &format!("- {w}\n");
            }
            s += "\n";
        }
        s += "| Search | recall@1 | recall@3 | recall@5 | MRR |\n|---|---|---|---|---|\n";
        s += &row("keyword (BM25)", &self.keyword);
        s += &row("vector (trigram)", &self.vector);
        s += &row("hybrid (RRF)", &self.hybrid);
        s += "\nRecall@5 by how much of the question's wording the right memory shares (where the words differ is where a search earns its keep):\n\n| The question | n | keyword | vector | hybrid |\n|---|---|---|---|---|\n";
        for b in &self.buckets {
            s += &format!("| {} | {} | {:.0}% | {:.0}% | {:.0}% |\n", b.name, b.n, b.keyword * 100.0, b.vector * 100.0, b.hybrid * 100.0);
        }
        s += &format!(
            "\nShipped recall, with its abstention rules:\n\n- answerable: **{}** answered correctly, **{}** answered with the wrong memory, {} declined although there was an answer\n- unanswerable: **{}** declined, **{}** answered anyway\n",
            self.correct, self.wrong, self.missed, self.quiet, self.answered_anyway
        );
        if !self.misses.is_empty() {
            s += "\n## What went wrong\n\n";
            for m in &self.misses {
                s += &format!("- {m}\n");
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_runs_and_says_where_it_fails() {
        let g: Value = serde_json::from_str(SAMPLE).unwrap();
        let r = run(&g).unwrap();
        eprintln!("{}", r.markdown());
        assert_eq!((r.answerable, r.unanswerable), (25, 5));
        // each way of searching finds most things; the hybrid is not worse than either alone at 5
        // measured: keyword 84 %, vector and hybrid 88 %; the three that share no word with their answer are found by none
        assert!((r.keyword.at5 - 0.84).abs() < 0.01 && (r.vector.at5 - 0.88).abs() < 0.01 && (r.hybrid.at5 - 0.88).abs() < 0.01, "{:?} {:?} {:?}", r.keyword, r.vector, r.hybrid);
        assert!(r.vector.at5 > r.keyword.at5, "the typo question is why the trigram search is there");
        // answers are mostly right and what is wrong is reported, not hidden
        assert_eq!((r.correct, r.wrong, r.missed), (22, 0, 3), "measured on the sample");
        assert_eq!(r.correct + r.wrong + r.missed, r.answerable);
        assert_eq!(r.quiet + r.answered_anyway, r.unanswerable);
        assert_eq!((r.quiet, r.answered_anyway), (5, 0));
        assert_eq!(r.misses.len(), r.wrong + r.answered_anyway + r.keyword_vs_hybrid_misses());
    }

    #[test]
    fn a_golden_set_with_a_mistake_is_refused_in_words() {
        let g = json!({"memories": [{"key": "a", "text": "x y z"}], "questions": [{"q": "x", "expect": ["nope"]}]});
        assert!(run(&g).unwrap_err().contains("unknown key 'nope'"));
        assert!(run(&json!({"questions": []})).unwrap_err().contains("memories"));
    }
}

impl Report {
    #[cfg(test)]
    fn keyword_vs_hybrid_misses(&self) -> usize {
        self.misses.iter().filter(|m| m.starts_with("the hybrid search")).count()
    }
}

#[cfg(test)]
mod lint_tests {
    use super::*;

    #[test]
    fn a_golden_set_that_says_little_is_told_so() {
        let g = json!({"memories": [{"key": "a", "text": "The haze cue fired late because the desk was busy."}, {"key": "b", "text": "Lasers are armed last."}],
                       "questions": [{"q": "the haze cue fired late because the desk was busy", "expect": ["a"]}, {"q": "the haze cue fired late because the desk was busy", "expect": ["a"]}]});
        let w = lint(&g);
        assert!(w.iter().any(|x| x.contains("only 2 answerable")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("only 0 question(s) that memory should decline")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("no question expects (b)")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("repeat almost every word")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("asked twice")), "{w:?}");
        assert!((overlap("why was the haze late", &search::tokens("The haze cue fired late.")) - 1.0).abs() < 1e-9);
        assert!(overlap("smoke effect", &search::tokens("The haze cue fired late.")) < 0.01);
    }
}
