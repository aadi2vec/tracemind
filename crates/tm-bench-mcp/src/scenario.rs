//! Scenario definitions and scoring for the MCP harness.

use serde::{Deserialize, Serialize};

use crate::selector::ToolSelector;

/// One scripted host turn: what the user said, and which tool a correct host
/// should call in response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    /// The user utterance the host is reacting to.
    pub utterance: String,
    /// The tool a correct host should call. `None` (or "none") means the
    /// host should call *no* memory tool — a genuine negative, used to
    /// measure over-triggering.
    #[serde(default)]
    pub expected_tool: Option<String>,
    /// Arguments to send when this scenario is executed against the live
    /// server (contract + latency check). Omitted for selection-only rows.
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
}

impl Scenario {
    /// The gold tool, normalising the "no tool" sentinel.
    pub fn gold(&self) -> Option<&str> {
        match self.expected_tool.as_deref() {
            Some("none") | Some("") => None,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioSet {
    pub scenarios: Vec<Scenario>,
}

/// Per-scenario selection outcome.
#[derive(Debug, Clone, Serialize)]
pub struct SelectionOutcome {
    pub id: String,
    pub utterance: String,
    pub expected: Option<String>,
    pub predicted: Option<String>,
    pub correct: bool,
}

/// Aggregate selection metrics.
///
/// Precision/recall are computed over *tool invocations*: a positive is a
/// scenario whose gold tool is some tool (not "none").
///
/// - **recall** = of scenarios that should call a tool, how many did the
///   selector route to the *right* tool.
/// - **precision** = of scenarios where the selector chose to call a tool,
///   how many were correct (charges for over-triggering on negatives).
#[derive(Debug, Clone, Serialize)]
pub struct SelectionReport {
    pub total: usize,
    pub correct: usize,
    pub accuracy: f32,
    pub precision: f32,
    pub recall: f32,
    pub f1: f32,
    /// Scenarios where the host should have stayed quiet but the selector
    /// fired a tool.
    pub false_triggers: usize,
    pub outcomes: Vec<SelectionOutcome>,
}

/// Score a scenario set against a selector.
pub fn score_selection(set: &ScenarioSet, selector: &ToolSelector) -> SelectionReport {
    let mut outcomes = Vec::with_capacity(set.scenarios.len());
    let mut correct = 0usize;
    let (mut tp, mut fp, mut fn_, mut false_triggers) = (0usize, 0usize, 0usize, 0usize);

    for sc in &set.scenarios {
        let predicted = selector.select(&sc.utterance);
        let gold = sc.gold();
        let is_correct = predicted.as_deref() == gold;
        if is_correct {
            correct += 1;
        }

        match (gold, predicted.as_deref()) {
            (Some(_), Some(_)) if is_correct => tp += 1,
            (Some(_), Some(_)) => {
                // wrong tool chosen: both a miss and a false fire.
                fp += 1;
                fn_ += 1;
            }
            (Some(_), None) => fn_ += 1, // should have fired, stayed silent
            (None, Some(_)) => {
                fp += 1;
                false_triggers += 1;
            }
            (None, None) => {} // correct silence
        }

        outcomes.push(SelectionOutcome {
            id: sc.id.clone(),
            utterance: sc.utterance.clone(),
            expected: gold.map(str::to_string),
            predicted,
            correct: is_correct,
        });
    }

    let total = set.scenarios.len();
    let precision = ratio(tp, tp + fp);
    let recall = ratio(tp, tp + fn_);
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    SelectionReport {
        total,
        correct,
        accuracy: ratio(correct, total),
        precision,
        recall,
        f1,
        false_triggers,
        outcomes,
    }
}

fn ratio(num: usize, den: usize) -> f32 {
    if den == 0 {
        0.0
    } else {
        num as f32 / den as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::ToolDef;

    fn selector() -> ToolSelector {
        ToolSelector::new(vec![
            ToolDef { name: "memory_store".into(), description: "ingest deposit text entities".into() },
            ToolDef { name: "memory_query".into(), description: "retrieve recall question answer".into() },
            ToolDef { name: "memory_contradict".into(), description: "contradict conflict retraction belief".into() },
        ])
    }

    fn set() -> ScenarioSet {
        ScenarioSet {
            scenarios: vec![
                Scenario { id: "a".into(), utterance: "ingest this deposit".into(), expected_tool: Some("memory_store".into()), arguments: None },
                Scenario { id: "b".into(), utterance: "recall the answer".into(), expected_tool: Some("memory_query".into()), arguments: None },
                Scenario { id: "c".into(), utterance: "does this conflict".into(), expected_tool: Some("memory_contradict".into()), arguments: None },
                Scenario { id: "d".into(), utterance: "photosynthesis chlorophyll".into(), expected_tool: Some("none".into()), arguments: None },
            ],
        }
    }

    #[test]
    fn perfect_selection_scores_one() {
        let r = score_selection(&set(), &selector());
        assert_eq!(r.correct, 4, "{:#?}", r.outcomes);
        assert!((r.accuracy - 1.0).abs() < 1e-6);
        assert!((r.f1 - 1.0).abs() < 1e-6);
        assert_eq!(r.false_triggers, 0);
    }

    #[test]
    fn false_trigger_is_penalised() {
        // A negative scenario whose utterance actually contains a tool term.
        let mut s = set();
        s.scenarios.push(Scenario {
            id: "e".into(),
            utterance: "just ingest".into(), // will fire memory_store
            expected_tool: Some("none".into()),
            arguments: None,
        });
        let r = score_selection(&s, &selector());
        assert_eq!(r.false_triggers, 1);
        assert!(r.precision < 1.0);
    }

    #[test]
    fn gold_normalises_none_sentinels() {
        let sc = Scenario { id: "x".into(), utterance: "".into(), expected_tool: Some("none".into()), arguments: None };
        assert_eq!(sc.gold(), None);
    }
}
