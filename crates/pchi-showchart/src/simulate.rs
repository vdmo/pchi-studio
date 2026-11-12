//! Simulation: explore every path through a chart without emitting anything.

use crate::showchart::Showchart;
use std::collections::HashSet;

/// Indented tree of reachable transitions, like a statechart simulator:
///
/// ```text
/// PRESHOW
/// └── START
///      INTRO
/// ```
pub fn tree(chart: &Showchart) -> String {
    fn walk(c: &Showchart, state: &str, depth: usize, seen: &mut HashSet<String>, out: &mut String) {
        let pad = "     ".repeat(depth);
        let label = c.state(state).map(|s| s.label.as_str()).filter(|l| !l.is_empty());
        out.push_str(&format!("{pad}{state}{}\n", label.map(|l| format!("  ({l})")).unwrap_or_default()));
        if !seen.insert(state.to_string()) {
            if c.transitions.iter().any(|t| t.source == state) {
                out.push_str(&format!("{pad}  (already shown above)\n"));
            }
            return;
        }
        for t in c.transitions.iter().filter(|t| t.source == state) {
            out.push_str(&format!("{pad}└── {}\n", t.label()));
            walk(c, &t.target, depth + 1, seen, out);
        }
    }
    let mut out = String::new();
    walk(chart, &chart.initial, 0, &mut HashSet::new(), &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_lists_the_path_and_survives_cycles() {
        let mut c = Showchart::from_json(include_str!("../examples/portal.json")).unwrap();
        let t = tree(&c);
        assert!(t.contains("└── START") && t.contains("FINALE"));
        c.transitions.push(crate::Transition::on("FINALE", "PRESHOW", "RESET"));
        assert!(tree(&c).contains("already shown above"));
    }
}
