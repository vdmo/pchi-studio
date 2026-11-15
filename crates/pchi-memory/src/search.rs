//! The three ways memory is searched, kept apart so each can be measured alone.
//!
//! * **keyword**: BM25 over the words of each memory.
//! * **vector**: cosine similarity of hashed character trigrams. This is NOT a neural embedding: it is a cheap, local, deterministic stand-in that catches
//!   what keywords miss (`delayed` / `delay`, a typo, `firework` / `fireworks`) and nothing deeper (it does not know that "pyro" and "fireworks" mean the same).
//! * **hybrid**: reciprocal-rank fusion of the two.
//!
//! Nothing here calls a model or the network. A real embedding model can be added behind the same shape later; `eval` is how you would know whether it helps.

const DIM: usize = 512;

const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "how", "in", "is", "it", "its", "of", "on", "or", "that", "the", "this", "to", "was", "were", "what", "when", "where", "which", "who", "why", "with", "did", "do", "does", "we", "our", "last", "time",
];

/// A very light stem: `armed`, `arms`, `arming` all become `arm`; `lasers` becomes `laser`; nothing cleverer.
fn stem(w: &str) -> String {
    let n = w.len();
    if n > 5 && w.ends_with("ing") {
        w[..n - 3].to_string()
    } else if n > 4 && w.ends_with("ed") {
        w[..n - 2].to_string()
    } else if n > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") {
        w[..n - 1].to_string()
    } else {
        w.to_string()
    }
}

/// Lower-case words, without stop words, lightly stemmed (a plural `s` is dropped).
pub fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(|w| w.to_lowercase()).filter(|w| !STOP.contains(&w.as_str())).map(|w| stem(&w)).collect()
}

fn fnv(s: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as usize
}

/// A unit-length vector of hashed character trigrams (with sub-linear counts).
pub fn vectorize(toks: &[String]) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    for t in toks {
        let padded: Vec<char> = format!("^{t}$").chars().collect();
        if padded.len() < 3 {
            continue;
        }
        for w in padded.windows(3) {
            let g: String = w.iter().collect();
            v[fnv(&g) % DIM] += 1.0;
        }
    }
    for x in v.iter_mut() {
        *x = (1.0 + *x).ln() * if *x > 0.0 { 1.0 } else { 0.0 };
    }
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x * y) as f64).sum()
}

/// A searchable memory.
pub struct Doc {
    pub toks: Vec<String>,
    pub vec: Vec<f32>,
}

impl Doc {
    pub fn new(text: &str) -> Self {
        let toks = tokens(text);
        let vec = vectorize(&toks);
        Doc { toks, vec }
    }
}

/// BM25 of each document for the query (0 for a document sharing no word with it).
pub fn bm25(docs: &[Doc], q: &[String]) -> Vec<f64> {
    let n = docs.len() as f64;
    if docs.is_empty() {
        return vec![];
    }
    let avg = docs.iter().map(|d| d.toks.len()).sum::<usize>() as f64 / n;
    let (k1, b) = (1.2, 0.75);
    let mut uniq: Vec<&String> = q.iter().collect();
    uniq.sort();
    uniq.dedup();
    docs.iter()
        .map(|d| {
            let len = d.toks.len() as f64;
            uniq.iter()
                .map(|t| {
                    let df = docs.iter().filter(|x| x.toks.contains(t)).count() as f64;
                    if df == 0.0 {
                        return 0.0;
                    }
                    let tf = d.toks.iter().filter(|x| x == t).count() as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * len / avg.max(1.0)))
                })
                .sum()
        })
        .collect()
}

/// The fraction of the query's distinct words the document has (a word also counts if one is the start of the other, from four letters).
pub fn coverage(doc: &[String], q: &[String]) -> f64 {
    let mut uniq: Vec<&String> = q.iter().collect();
    uniq.sort();
    uniq.dedup();
    if uniq.is_empty() {
        return 0.0;
    }
    let hit = |t: &&String| doc.iter().any(|d| d == *t || (t.len() >= 4 && d.len() >= 4 && (d.starts_with(t.as_str()) || t.starts_with(d.as_str()))));
    uniq.iter().filter(|t| hit(t)).count() as f64 / uniq.len() as f64
}

/// The same word, allowing for a typo or a different ending: equal, or one starts the other (from four letters), or their character trigrams overlap by 60 % (from five).
pub fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.len() >= 4 && b.len() >= 4 && (a.starts_with(b) || b.starts_with(a)) {
        return true;
    }
    if a.len() < 5 || b.len() < 5 {
        return false;
    }
    let grams = |w: &str| -> std::collections::BTreeSet<String> { let p: Vec<char> = format!("^{w}$").chars().collect(); p.windows(3).map(|g| g.iter().collect()).collect() };
    let (x, y) = (grams(a), grams(b));
    2.0 * x.intersection(&y).count() as f64 / (x.len() + y.len()) as f64 >= 0.6
}

/// Like `coverage`, but each query word counts by how rare it is in the memory: matching a word every memory has says little, and a word NO memory has
/// (a name, a device, a topic that was never written down) weighs as much as a word only one memory has, so a question about something unknown is not answered by its nearest neighbour.
pub fn coverage_idf(docs: &[Doc], di: usize, q: &[String]) -> f64 {
    let mut uniq: Vec<&String> = q.iter().collect();
    uniq.sort();
    uniq.dedup();
    let n = docs.len() as f64;
    let has = |d: &Doc, t: &String| d.toks.iter().any(|x| same_word(x, t));
    let (mut got, mut all) = (0.0, 0.0);
    for t in uniq {
        let df = docs.iter().filter(|d| has(d, t)).count() as f64;
        // a word no memory has weighs as much as one only a single memory has (not more: "went wrong" is not a topic), and no word counts for less than 1
        let w = (((n - df.max(1.0) + 0.5) / (df.max(1.0) + 0.5) + 1.0).ln()).max(1.0);
        all += w;
        if has(&docs[di], t) {
            got += w;
        }
    }
    if all > 0.0 { got / all } else { 0.0 }
}

/// Indices ordered best first, only those with a score above zero.
pub fn order(scores: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).filter(|&i| scores[i] > 1e-9).collect();
    idx.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    idx
}

/// Reciprocal-rank fusion of ranked lists (k = 60), best first.
pub fn rrf(lists: &[Vec<usize>]) -> Vec<usize> {
    let mut score: std::collections::BTreeMap<usize, f64> = Default::default();
    for l in lists {
        for (rank, &i) in l.iter().enumerate() {
            *score.entry(i).or_default() += 1.0 / (60.0 + rank as f64 + 1.0);
        }
    }
    let mut v: Vec<(usize, f64)> = score.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(i, _)| i).collect()
}

/// Rank the documents for a query three ways: (keyword, vector, hybrid), each best first.
pub fn rankings(docs: &[Doc], query: &str) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let q = tokens(query);
    let qv = vectorize(&q);
    let kw = order(&bm25(docs, &q));
    // a vector score counts only when it is clearly more than chance overlap of trigrams
    let vs: Vec<f64> = docs.iter().map(|d| cosine(&d.vec, &qv)).map(|c| if c >= 0.2 { c } else { 0.0 }).collect();
    let vec = order(&vs);
    let hy = rrf(&[kw.clone(), vec.clone()]);
    (kw, vec, hy)
}
