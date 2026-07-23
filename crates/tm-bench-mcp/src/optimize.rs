//! GEPA over tool descriptions (holistic review §5 P0.2).
//!
//! The tool descriptions are the only prompt TraceMind controls inside a
//! host's context, and the MCP benchmark showed they are the bottleneck:
//! the descriptions say "ingest text" while users say "remember", "note",
//! "save". This is the same reflective-mutation loop `tm-gepa` runs for
//! retrieval, specialised to descriptions:
//!
//!   reflect on the misses  →  the trigger words a description lacks are
//!   exactly the content words its failing utterances use, minus the words
//!   its rivals already own  →  propose adding the most discriminative of
//!   them  →  re-score on the training set  →  keep only if selection F1
//!   improves (the verifier gate).
//!
//! The output is a `{tool_name: description}` override map, applied by
//! `tm-mcp` the same way a GEPA-tuned `policy.json` is applied by the
//! retrieval engine — so the optimisation reaches the running product
//! rather than living in the benchmark.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::scenario::{score_selection, ScenarioSet};
use crate::selector::{tokenize, ToolDef, ToolSelector};

/// Split a scenario set into (propose, verify) halves by alternating index.
///
/// Proposals are harvested from the propose half; every edit is gated on the
/// verify half. Because the two halves are disjoint, a term that only helps
/// the one propose example it came from cannot move the verify score and is
/// rejected — which is what stops the loop memorising the fixture. This is
/// the same anchor-set discipline `tm-gepa` uses for retrieval, and it is
/// why the reported held-out number generalises. Alternating (rather than
/// splitting front/back) keeps every tool represented in both halves even
/// though the fixture groups scenarios by tool.
fn split_alternating(set: &ScenarioSet) -> (ScenarioSet, ScenarioSet) {
    let mut propose = Vec::new();
    let mut verify = Vec::new();
    for (i, sc) in set.scenarios.iter().enumerate() {
        if i % 2 == 0 {
            propose.push(sc.clone());
        } else {
            verify.push(sc.clone());
        }
    }
    (ScenarioSet { scenarios: propose }, ScenarioSet { scenarios: verify })
}

#[derive(Debug, Clone)]
pub struct OptimizeResult {
    pub baseline_f1: f32,
    pub best_f1: f32,
    pub rounds: usize,
    pub edits: usize,
    /// Final descriptions, keyed by tool name.
    pub descriptions: BTreeMap<String, String>,
    /// Human-readable log of what changed and why.
    pub history: Vec<String>,
}

/// Optimise `tools`' descriptions against `train`, keeping every edit that
/// improves training selection F1.
pub fn optimize_descriptions(
    tools: &[ToolDef],
    train: &ScenarioSet,
    max_rounds: usize,
) -> OptimizeResult {
    let mut descriptions: BTreeMap<String, String> =
        tools.iter().map(|t| (t.name.clone(), t.description.clone())).collect();

    let (propose_set, verify_set) = split_alternating(train);

    let build = |desc: &BTreeMap<String, String>| -> Vec<ToolDef> {
        tools
            .iter()
            .map(|t| ToolDef {
                name: t.name.clone(),
                description: desc.get(&t.name).cloned().unwrap_or_default(),
            })
            .collect()
    };

    // The gate metric is verify-half F1 — the half no proposal is drawn from.
    let verify_f1 = |desc: &BTreeMap<String, String>| -> f32 {
        score_selection(&verify_set, &ToolSelector::new(build(desc))).f1
    };

    let baseline_f1 = verify_f1(&descriptions);
    let mut best_f1 = baseline_f1;
    let mut history = Vec::new();
    let mut edits = 0usize;
    let mut round = 0usize;

    while round < max_rounds {
        round += 1;
        let report = score_selection(&propose_set, &ToolSelector::new(build(&descriptions)));

        // Reflect: collect, per gold tool, the content words from its missed
        // utterances that the tool's own description does not yet contain.
        // Count *distinct utterances* a term appears in, not raw frequency.
        let mut wanted: HashMap<String, HashMap<String, usize>> = HashMap::new();
        for o in report.outcomes.iter().filter(|o| !o.correct) {
            let Some(gold) = &o.expected else { continue };
            let have: HashSet<String> =
                tokenize(descriptions.get(gold).map(String::as_str).unwrap_or("")).into_iter().collect();
            let utterance_terms: HashSet<String> = tokenize(&o.utterance).into_iter().collect();
            for term in utterance_terms {
                if !have.contains(&term) {
                    *wanted.entry(gold.clone()).or_default().entry(term).or_insert(0) += 1;
                }
            }
        }

        if wanted.is_empty() {
            break; // nothing left to reflect on
        }

        // How discriminative is a term? A term already common across many
        // descriptions is a poor addition — it would just re-create the
        // "every tool says memory" ambiguity. Prefer terms owned by few.
        let mut desc_freq: HashMap<String, usize> = HashMap::new();
        for d in descriptions.values() {
            for term in tokenize(d).into_iter().collect::<HashSet<_>>() {
                *desc_freq.entry(term).or_insert(0) += 1;
            }
        }

        let mut improved_this_round = false;
        for (tool, terms) in &wanted {
            // Rank candidate terms: frequently-missed and rarely-owned first.
            let mut ranked: Vec<(&String, f32)> = terms
                .iter()
                .map(|(term, miss_count)| {
                    let owned = *desc_freq.get(term).unwrap_or(&0) as f32;
                    (term, *miss_count as f32 / (1.0 + owned))
                })
                .collect();
            ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            for (term, _) in ranked.into_iter().take(3) {
                let mut trial = descriptions.clone();
                let entry = trial.get_mut(tool).unwrap();
                entry.push(' ');
                entry.push_str(term);
                let trial_f1 = verify_f1(&trial);
                // Verifier gate: keep only edits that improve the held-out
                // verify half. An edit that merely memorises its own propose
                // example cannot move this score and is rejected.
                if trial_f1 > best_f1 + 1e-6 {
                    history.push(format!(
                        "round {round}: +'{term}' to {tool}  (F1 {best_f1:.3} -> {trial_f1:.3})"
                    ));
                    descriptions = trial;
                    best_f1 = trial_f1;
                    edits += 1;
                    improved_this_round = true;
                }
            }
        }

        if !improved_this_round {
            break;
        }
    }

    OptimizeResult {
        baseline_f1,
        best_f1,
        rounds: round,
        edits,
        descriptions,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef { name: "memory_store".into(), description: "ingest text".into() },
            ToolDef { name: "memory_query".into(), description: "retrieve answer".into() },
        ]
    }

    fn train() -> ScenarioSet {
        // "remember" appears in both split halves for store; "retrieve" is
        // already in the query description. Interleaved so the alternating
        // split keeps both tools in propose and verify.
        let mk = |id: &str, u: &str, t: &str| Scenario {
            id: id.into(), utterance: u.into(), expected_tool: Some(t.into()), arguments: None,
        };
        // Grouped by tool (like the real fixture) so alternating-index
        // splitting keeps each tool in both halves.
        ScenarioSet {
            scenarios: vec![
                mk("s1", "remember this important thing", "memory_store"),
                mk("s2", "remember the launch date", "memory_store"),
                mk("s3", "remember to buy milk", "memory_store"),
                mk("s4", "remember my flight time", "memory_store"),
                mk("q1", "retrieve the first answer", "memory_query"),
                mk("q2", "retrieve the second answer", "memory_query"),
            ],
        }
    }

    #[test]
    fn optimization_improves_or_holds_training_f1() {
        let r = optimize_descriptions(&tools(), &train(), 5);
        assert!(r.best_f1 >= r.baseline_f1, "F1 regressed: {} < {}", r.best_f1, r.baseline_f1);
    }

    #[test]
    fn learns_a_trigger_word_that_generalises() {
        // "remember" appears in store utterances across both split halves,
        // so adding it improves the held-out verify half and is kept.
        let r = optimize_descriptions(&tools(), &train(), 5);
        let store = r.descriptions.get("memory_store").unwrap().to_lowercase();
        assert!(
            store.contains("remember"),
            "store description did not learn the generalising trigger word: {store}"
        );
        assert!(r.best_f1 > r.baseline_f1, "should have strictly improved held-out F1");
    }

    #[test]
    fn one_off_content_words_are_not_learned() {
        // "mysql" appears in exactly one propose-half utterance and never in
        // the verify half, so it cannot improve the gate metric and must be
        // rejected — the held-out gate is what prevents memorisation.
        let mk = |id: &str, u: &str, t: &str| Scenario {
            id: id.into(), utterance: u.into(), expected_tool: Some(t.into()), arguments: None,
        };
        let set = ScenarioSet {
            scenarios: vec![
                mk("s1", "remember we picked mysql today", "memory_store"),
                mk("s2", "remember the launch date", "memory_store"),
                mk("q1", "retrieve the answer", "memory_query"),
                mk("q2", "retrieve another answer", "memory_query"),
            ],
        };
        let r = optimize_descriptions(&tools(), &set, 5);
        let store = r.descriptions.get("memory_store").unwrap().to_lowercase();
        assert!(!store.contains("mysql"), "learned a one-off content word: {store}");
    }

    #[test]
    fn every_edit_is_gated_on_improvement() {
        let r = optimize_descriptions(&tools(), &train(), 5);
        // The history only records strict improvements, so edits == history len.
        assert_eq!(r.edits, r.history.len());
    }

    #[test]
    fn converges_when_nothing_left_to_learn() {
        // Descriptions already perfect: no edits, terminates.
        let perfect = vec![
            ToolDef { name: "memory_store".into(), description: "remember note store ingest".into() },
            ToolDef { name: "memory_query".into(), description: "retrieve answer recall".into() },
        ];
        let r = optimize_descriptions(&perfect, &train(), 5);
        assert_eq!(r.edits, 0);
    }
}
