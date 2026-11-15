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
    let mut misses = Vec::new();
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
    Ok(Report { answerable: kw.len(), unanswerable, keyword: rates(&kw), vector: rates(&ve), hybrid: rates(&hy), correct, wrong, missed, quiet, answered_anyway: anyway, misses })
}

impl Report {
    pub fn to_json(&self) -> Value {
        let r = |x: &Rates| json!({"recall_at_1": x.at1, "recall_at_3": x.at3, "recall_at_5": x.at5, "mrr": x.mrr});
        json!({"answerable": self.answerable, "unanswerable": self.unanswerable, "keyword": r(&self.keyword), "vector": r(&self.vector), "hybrid": r(&self.hybrid),
               "shipped": {"correct": self.correct, "wrong": self.wrong, "abstained_but_had_answer": self.missed, "kept_quiet": self.quiet, "answered_anyway": self.answered_anyway}, "misses": self.misses})
    }

    pub fn markdown(&self) -> String {
        let row = |n: &str, r: &Rates| format!("| {n} | {:.0}% | {:.0}% | {:.0}% | {:.2} |\n", r.at1 * 100.0, r.at3 * 100.0, r.at5 * 100.0, r.mrr);
        let mut s = format!("# Memory evaluation\n\n{} answerable question(s), {} that memory should decline.\n\n| Search | recall@1 | recall@3 | recall@5 | MRR |\n|---|---|---|---|---|\n", self.answerable, self.unanswerable);
        s += &row("keyword (BM25)", &self.keyword);
        s += &row("vector (trigram)", &self.vector);
        s += &row("hybrid (RRF)", &self.hybrid);
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
