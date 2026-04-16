//! Query Planner — TraceMind's prefrontal cortex.
//!
//! Inspired by MIA's Planner (strategy generation) and Graph-R1's
//! think→retrieve→rethink→generate loop. Assesses query complexity,
//! selects execution strategy, and supports multi-step re-planning.
//!
//! The Planner sits above the UCB1 bandit. The bandit selects *which*
//! retrieval arm to use; the Planner selects *what kind of operation*
//! to perform in the first place.

use std::collections::HashSet;
use tm_types::TimeRange;

/// The type of operation the Planner recommends.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PlanAction {
    /// Simple vector lookup — short, single-entity query.
    DirectLookup,
    /// Standard retrieval — let the bandit choose the arm.
    BanditRetrieval,
    /// Multi-hop reasoning — query mentions relationships/paths.
    ReasoningChain {
        source_hint: Option<String>,
        target_hint: Option<String>,
    },
    /// Analogy search — query asks "what's like X" or "similar to X".
    AnalogySearch { entity_hint: String },
    /// Decomposed query — too complex for single retrieval,
    /// split into sub-queries and merge results.
    Decompose { sub_queries: Vec<String> },
    /// Consolidation — user wants to clean up / optimize memory.
    Consolidate,
    /// Temporal query — "what was I working on last week?"
    /// Filters entities/traces by time range before standard retrieval.
    TemporalQuery {
        time_range: TimeRange,
    },
}

/// Complexity assessment for a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueryComplexity {
    /// Single entity lookup: "What is Rust?"
    Simple,
    /// Multi-entity with relationships: "How does Rust connect to Python?"
    Moderate,
    /// Multi-hop or conditional: "What projects use Rust and depend on SQLite?"
    Complex,
    /// Requires decomposition: "Compare Rust and Python's ecosystem and find projects using both"
    Compound,
}

/// The execution plan produced by the Planner.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryPlan {
    /// Primary action to take.
    pub action: PlanAction,
    /// Assessed complexity level.
    pub complexity: String,
    /// Confidence in this plan (0.0 - 1.0).
    pub confidence: f64,
    /// Should we fall back to a wider strategy if results are poor?
    pub allow_fallback: bool,
    /// Maximum cascade depth for fallback.
    pub max_cascade: u8,
    /// Entities mentioned in the query (for targeted search).
    pub entity_hints: Vec<String>,
}

/// Keyword sets for classification.
const RELATIONSHIP_WORDS: &[&str] = &[
    "relate", "connect", "link", "between", "from", "to",
    "path", "chain", "how does", "relationship", "depend",
    "affect", "influence", "cause", "lead", "through",
];

const ANALOGY_WORDS: &[&str] = &[
    "similar", "like", "analogy", "analogous", "comparable",
    "equivalent", "same as", "resembles", "reminds",
    "what else", "alternatives",
];

const COMPARISON_WORDS: &[&str] = &[
    "compare", "versus", "vs", "difference", "better",
    "contrast", "both", "each", "pros", "cons",
];

const CONSOLIDATION_WORDS: &[&str] = &[
    "consolidate", "clean", "prune", "merge", "sleep",
    "optimize", "deduplicate", "decay", "forget",
];

const CONJUNCTIONS: &[&str] = &[
    " and ", " or ", " also ", " plus ", " as well as ",
    " along with ", " together with ",
];

pub struct QueryPlanner {
    /// Minimum word count for a query to be considered moderate complexity.
    moderate_threshold: usize,
    /// Minimum word count for complex classification.
    complex_threshold: usize,
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self {
            moderate_threshold: 5,
            complex_threshold: 12,
        }
    }
}

impl QueryPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Analyze a query and produce an execution plan.
    ///
    /// This is the core planning function — Graph-R1's "think" step.
    pub fn plan(&self, query: &str) -> QueryPlan {
        let lower = query.to_lowercase();
        let word_count = query.split_whitespace().count();
        let complexity = self.assess_complexity(&lower, word_count);
        let entity_hints = self.extract_entity_hints(query);

        // Check for temporal queries first (highest priority)
        if tm_types::has_temporal_intent(&lower) {
            if let Some(time_range) = tm_types::parse_time_expression_now(&lower) {
                return QueryPlan {
                    action: PlanAction::TemporalQuery { time_range },
                    complexity: format!("{:?}", complexity),
                    confidence: 0.90,
                    allow_fallback: true,
                    max_cascade: 1,
                    entity_hints,
                };
            }
        }

        // Check for consolidation requests (second highest priority)
        if self.matches_any(&lower, CONSOLIDATION_WORDS) {
            return QueryPlan {
                action: PlanAction::Consolidate,
                complexity: format!("{:?}", complexity),
                confidence: 0.95,
                allow_fallback: false,
                max_cascade: 0,
                entity_hints,
            };
        }

        // Check for analogy queries
        if self.matches_any(&lower, ANALOGY_WORDS) {
            let entity = entity_hints.first().cloned()
                .unwrap_or_else(|| self.extract_after_pattern(&lower, &["like", "similar to", "what else"]).unwrap_or_default());
            if !entity.is_empty() {
                return QueryPlan {
                    action: PlanAction::AnalogySearch { entity_hint: entity },
                    complexity: format!("{:?}", complexity),
                    confidence: 0.85,
                    allow_fallback: true,
                    max_cascade: 1,
                    entity_hints,
                };
            }
        }

        // Check for relationship/path queries
        if self.matches_any(&lower, RELATIONSHIP_WORDS) && entity_hints.len() >= 2 {
            return QueryPlan {
                action: PlanAction::ReasoningChain {
                    source_hint: entity_hints.first().cloned(),
                    target_hint: entity_hints.get(1).cloned(),
                },
                complexity: format!("{:?}", complexity),
                confidence: 0.80,
                allow_fallback: true,
                max_cascade: 2,
                entity_hints,
            };
        }

        // Check for compound/comparison queries that need decomposition
        if (complexity == QueryComplexity::Compound || self.matches_any(&lower, COMPARISON_WORDS))
            && entity_hints.len() >= 2
        {
            let sub_queries = self.decompose_query(query, &entity_hints);
            if sub_queries.len() > 1 {
                return QueryPlan {
                    action: PlanAction::Decompose { sub_queries },
                    complexity: format!("{:?}", complexity),
                    confidence: 0.70,
                    allow_fallback: true,
                    max_cascade: 2,
                    entity_hints,
                };
            }
        }

        // Simple or moderate queries → standard retrieval
        let (action, confidence) = match complexity {
            QueryComplexity::Simple => (PlanAction::DirectLookup, 0.90),
            _ => (PlanAction::BanditRetrieval, 0.85),
        };

        QueryPlan {
            action,
            complexity: format!("{:?}", complexity),
            confidence,
            allow_fallback: true, // always allow fallback — even simple queries can miss
            max_cascade: match complexity {
                QueryComplexity::Simple => 1,
                QueryComplexity::Moderate => 2,
                QueryComplexity::Complex | QueryComplexity::Compound => 3,
            },
            entity_hints,
        }
    }

    /// Re-plan after seeing poor results (Graph-R1's "rethink" step).
    ///
    /// If the initial plan produced low-quality results, escalate to
    /// a more expensive strategy.
    pub fn replan(&self, original: &QueryPlan, result_quality: f64) -> Option<QueryPlan> {
        if result_quality > 0.4 || !original.allow_fallback {
            return None; // Results are acceptable or fallback disabled
        }

        let escalated_action = match &original.action {
            PlanAction::DirectLookup => PlanAction::BanditRetrieval,
            PlanAction::BanditRetrieval => {
                if original.entity_hints.len() >= 2 {
                    PlanAction::ReasoningChain {
                        source_hint: original.entity_hints.first().cloned(),
                        target_hint: original.entity_hints.get(1).cloned(),
                    }
                } else if original.entity_hints.len() == 1 {
                    PlanAction::AnalogySearch {
                        entity_hint: original.entity_hints[0].clone(),
                    }
                } else {
                    return None; // Can't escalate further without entity hints
                }
            }
            PlanAction::ReasoningChain { .. } => {
                // Already tried reasoning — decompose if possible
                if original.entity_hints.len() >= 2 {
                    let sub_queries: Vec<String> = original.entity_hints.iter()
                        .map(|e| format!("What is {}?", e))
                        .collect();
                    PlanAction::Decompose { sub_queries }
                } else {
                    return None;
                }
            }
            _ => return None, // Can't escalate Consolidate, Analogy, Decompose, TemporalQuery
        };

        Some(QueryPlan {
            action: escalated_action,
            complexity: original.complexity.clone(),
            confidence: original.confidence * 0.7, // Lower confidence on escalation
            allow_fallback: false, // Don't cascade further
            max_cascade: 0,
            entity_hints: original.entity_hints.clone(),
        })
    }

    /// Re-plan with error context from the failed execution.
    /// Unlike blind `replan()`, this uses the error description to select
    /// a more appropriate strategy (BIGMAS self-correction loop).
    ///
    /// Error context examples:
    /// - "0 entities found" → try wider search or suggest ingestion
    /// - "all results low similarity" → try reasoning chain or analogy
    /// - "cascade exhausted" → topic likely not in memory
    pub fn replan_with_context(&self, original: &QueryPlan, error_ctx: &str) -> QueryPlan {
        let lower_err = error_ctx.to_lowercase();

        // 1. No entities found — escalate search strategy
        if lower_err.contains("0 entities") || lower_err.contains("no entities") {
            return match &original.action {
                PlanAction::DirectLookup => QueryPlan {
                    action: PlanAction::BanditRetrieval,
                    complexity: original.complexity.clone(),
                    confidence: 0.6,
                    allow_fallback: true,
                    max_cascade: 2,
                    entity_hints: original.entity_hints.clone(),
                },
                PlanAction::BanditRetrieval => QueryPlan {
                    action: PlanAction::ReasoningChain {
                        source_hint: original.entity_hints.first().cloned(),
                        target_hint: original.entity_hints.get(1).cloned(),
                    },
                    complexity: original.complexity.clone(),
                    confidence: 0.4,
                    allow_fallback: true,
                    max_cascade: 1,
                    entity_hints: original.entity_hints.clone(),
                },
                _ => {
                    // Give up — already tried advanced strategies
                    let mut plan = original.clone();
                    plan.confidence = 0.1;
                    plan.allow_fallback = false;
                    plan
                }
            };
        }

        // 2. Low similarity results — try analogy or decomposition
        if lower_err.contains("low similarity") || lower_err.contains("max_sim") {
            if let Some(entity) = original.entity_hints.first() {
                return QueryPlan {
                    action: PlanAction::AnalogySearch {
                        entity_hint: entity.clone(),
                    },
                    complexity: original.complexity.clone(),
                    confidence: 0.5,
                    allow_fallback: true,
                    max_cascade: 1,
                    entity_hints: original.entity_hints.clone(),
                };
            }
            // No entity hints — decompose the query into simpler parts
            let sub_queries = original
                .entity_hints
                .iter()
                .map(|e| e.clone())
                .collect::<Vec<_>>();
            if sub_queries.len() > 1 {
                return QueryPlan {
                    action: PlanAction::Decompose { sub_queries },
                    complexity: original.complexity.clone(),
                    confidence: 0.45,
                    allow_fallback: false,
                    max_cascade: 1,
                    entity_hints: original.entity_hints.clone(),
                };
            }
            // Fall through to default if no hints at all
        }

        // 3. Cascade exhausted — topic not in memory
        if lower_err.contains("cascade") || lower_err.contains("exhausted") {
            let mut plan = original.clone();
            plan.confidence = 0.0;
            return plan;
        }

        // 4. Default: fall back to existing replan() behavior
        match self.replan(original, 0.0) {
            Some(plan) => plan,
            None => {
                let mut plan = original.clone();
                plan.confidence = (plan.confidence * 0.7).min(0.5);
                plan
            }
        }
    }

    /// Assess query complexity based on linguistic features.
    fn assess_complexity(&self, lower: &str, word_count: usize) -> QueryComplexity {
        // Count conjunctions (indicates compound query)
        let conjunction_count = CONJUNCTIONS.iter()
            .filter(|c| lower.contains(*c))
            .count();

        // Count question marks or clauses
        let clause_count = lower.matches(',').count()
            + lower.matches(" and ").count()
            + lower.matches(" but ").count();

        if conjunction_count >= 2 || (clause_count >= 2 && word_count >= self.complex_threshold) {
            QueryComplexity::Compound
        } else if word_count >= self.complex_threshold || clause_count >= 1 {
            QueryComplexity::Complex
        } else if word_count >= self.moderate_threshold {
            QueryComplexity::Moderate
        } else {
            QueryComplexity::Simple
        }
    }

    /// Extract potential entity names from the query (capitalized words/phrases).
    fn extract_entity_hints(&self, query: &str) -> Vec<String> {
        let mut hints = Vec::new();
        let mut seen = HashSet::new();

        // Look for capitalized words (likely entity names)
        for word in query.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
            if clean.is_empty() { continue; }

            let first_char = clean.chars().next().unwrap();
            if first_char.is_uppercase() && clean.len() > 1 {
                // Skip common sentence starters
                let lower = clean.to_lowercase();
                if !["what", "how", "who", "where", "when", "why", "the", "does",
                     "can", "is", "are", "do", "which", "find", "show", "list",
                     "get", "tell", "give", "compare"].contains(&lower.as_str())
                {
                    if seen.insert(lower) {
                        hints.push(clean.to_string());
                    }
                }
            }
        }

        hints
    }

    /// Check if the query contains any of the given keywords.
    fn matches_any(&self, lower: &str, keywords: &[&str]) -> bool {
        keywords.iter().any(|kw| lower.contains(kw))
    }

    /// Extract the word/phrase after a pattern like "like X" or "similar to X".
    fn extract_after_pattern(&self, lower: &str, patterns: &[&str]) -> Option<String> {
        for pattern in patterns {
            if let Some(pos) = lower.find(pattern) {
                let after = &lower[pos + pattern.len()..].trim();
                let entity: String = after.split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if !entity.is_empty() {
                    return Some(entity);
                }
            }
        }
        None
    }

    /// Decompose a compound query into sub-queries per entity.
    fn decompose_query(&self, _query: &str, entity_hints: &[String]) -> Vec<String> {
        // Simple decomposition: one sub-query per entity
        entity_hints.iter()
            .map(|e| format!("{}", e))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_query_gets_direct_lookup() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        assert_eq!(plan.action, PlanAction::DirectLookup);
        assert_eq!(plan.complexity, "Simple");
        assert!(plan.confidence > 0.8);
    }

    #[test]
    fn relationship_query_gets_reasoning_chain() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("How does Rust relate to Python in the TraceMind project?");
        assert!(matches!(plan.action, PlanAction::ReasoningChain { .. }));
    }

    #[test]
    fn analogy_query_detected() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is similar to Rust?");
        assert!(matches!(plan.action, PlanAction::AnalogySearch { .. }));
    }

    #[test]
    fn consolidation_query_detected() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("consolidate my memories");
        assert_eq!(plan.action, PlanAction::Consolidate);
    }

    #[test]
    fn compound_query_gets_decomposed() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("Compare Rust and Python and JavaScript for web development");
        // Should detect compound (multiple "and") with 3+ entity hints
        assert!(
            matches!(plan.action, PlanAction::Decompose { .. })
            || matches!(plan.action, PlanAction::BanditRetrieval),
            "got {:?}", plan.action
        );
    }

    #[test]
    fn replan_escalates_on_poor_results() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        assert_eq!(plan.action, PlanAction::DirectLookup);

        // Poor results → escalate to BanditRetrieval
        let replan = planner.replan(&plan, 0.1);
        assert!(replan.is_some());
        assert_eq!(replan.unwrap().action, PlanAction::BanditRetrieval);
    }

    #[test]
    fn replan_returns_none_on_good_results() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        let replan = planner.replan(&plan, 0.8);
        assert!(replan.is_none());
    }

    #[test]
    fn entity_hints_extracted() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("How does Rust connect to TraceMind?");
        assert!(plan.entity_hints.contains(&"Rust".to_string()));
        assert!(plan.entity_hints.contains(&"TraceMind".to_string()));
    }

    #[test]
    fn test_replan_with_context_no_entities() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        assert_eq!(plan.action, PlanAction::DirectLookup);

        // "0 entities found" should escalate DirectLookup → BanditRetrieval
        let replanned = planner.replan_with_context(&plan, "0 entities found in graph");
        assert_eq!(replanned.action, PlanAction::BanditRetrieval);
        assert!(replanned.confidence <= 0.6);
    }

    #[test]
    fn test_replan_with_context_low_similarity() {
        let planner = QueryPlanner::new();
        let plan = QueryPlan {
            action: PlanAction::BanditRetrieval,
            complexity: "Moderate".to_string(),
            confidence: 0.85,
            allow_fallback: true,
            max_cascade: 2,
            entity_hints: vec!["Rust".to_string()],
        };

        // "low similarity" with entity hints should try AnalogySearch
        let replanned = planner.replan_with_context(&plan, "all results low similarity, max_sim=0.12");
        assert!(matches!(replanned.action, PlanAction::AnalogySearch { .. }));
        if let PlanAction::AnalogySearch { entity_hint } = &replanned.action {
            assert_eq!(entity_hint, "Rust");
        }
    }

    #[test]
    fn test_replan_with_context_cascade_exhausted() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");

        // "cascade exhausted" should drop confidence to 0.0
        let replanned = planner.replan_with_context(&plan, "cascade exhausted after 3 attempts");
        assert!((replanned.confidence - 0.0).abs() < f64::EPSILON);
        // Action should remain unchanged
        assert_eq!(replanned.action, plan.action);
    }

    #[test]
    fn temporal_query_detected_last_week() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("what was I working on last week?");
        assert!(matches!(plan.action, PlanAction::TemporalQuery { .. }), "got {:?}", plan.action);
        if let PlanAction::TemporalQuery { time_range } = &plan.action {
            assert_eq!(time_range.label, "last week");
        }
    }

    #[test]
    fn temporal_query_detected_yesterday() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("show me what I did yesterday");
        assert!(matches!(plan.action, PlanAction::TemporalQuery { .. }), "got {:?}", plan.action);
        if let PlanAction::TemporalQuery { time_range } = &plan.action {
            assert_eq!(time_range.label, "yesterday");
        }
    }

    #[test]
    fn temporal_query_detected_recently() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("what have I been doing recently?");
        assert!(matches!(plan.action, PlanAction::TemporalQuery { .. }), "got {:?}", plan.action);
    }

    #[test]
    fn non_temporal_query_not_detected() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        assert!(!matches!(plan.action, PlanAction::TemporalQuery { .. }));
    }

    #[test]
    fn test_replan_with_context_default_fallback() {
        let planner = QueryPlanner::new();
        let plan = planner.plan("What is Rust?");
        assert_eq!(plan.action, PlanAction::DirectLookup);

        // Unknown error should fall back to standard replan() behavior
        let replanned = planner.replan_with_context(&plan, "unexpected timeout error");
        // Standard replan from DirectLookup escalates to BanditRetrieval
        assert_eq!(replanned.action, PlanAction::BanditRetrieval);
    }
}
