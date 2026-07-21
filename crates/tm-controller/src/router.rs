//! Q3.2 — Memory-routing gate.
//!
//! Binary `should_retrieve: bool` decision that fires in `QueryPlanner`
//! *before* `LinUcbBandit.select()`. This is a routing layer, not a bandit
//! arm — conflating the two would corrupt arm statistics with a qualitatively
//! different action.
//!
//! Signal inputs (in priority order):
//! 1. `host_id` policy — some hosts (e.g., capture daemon) never retrieve.
//! 2. Working-memory cache hit — if the answer is already pinned, skip retrieval.
//! 3. Query entropy — one-word commands and pure code are unlikely to benefit.
//! 4. Recent miss-rate — if recent behavioral signals show persistent misses
//!    for this query pattern, the gate may route around retrieval entirely.

/// Context signals passed to the routing gate.
#[derive(Debug, Clone, Default)]
pub struct RouterContext {
    /// MCP host ID, if known (e.g., "claude-code", "goose", "capture").
    pub host_id: Option<String>,
    /// True if the answer is already available in working memory.
    pub working_memory_hit: bool,
    /// Fraction of recent retrievals that were rated as misses by the
    /// feedback signal fabric (0.0 = all hits, 1.0 = all misses).
    pub recent_miss_rate: f32,
    /// Caller hint: the query is known to be a short/code/command fragment
    /// where retrieval adds no value.
    pub is_command: bool,
}

/// Routing gate that decides whether retrieval should run at all.
///
/// Constructed once and reused. Stateless — all decisions are
/// deterministic from the `RouterContext`.
#[derive(Debug, Clone)]
pub struct MemoryRouter {
    /// Hosts that should always skip retrieval (e.g., clipboard capture).
    skip_hosts: Vec<String>,
    /// Miss-rate threshold above which the gate starts blocking.
    miss_rate_threshold: f32,
}

impl Default for MemoryRouter {
    fn default() -> Self {
        Self {
            skip_hosts: vec!["capture".to_string(), "clipboard".to_string()],
            miss_rate_threshold: 0.8,
        }
    }
}

impl MemoryRouter {
    pub fn new(skip_hosts: Vec<String>, miss_rate_threshold: f32) -> Self {
        Self { skip_hosts, miss_rate_threshold }
    }

    /// Main gate. Returns `true` if retrieval should proceed.
    pub fn should_retrieve(&self, query: &str, ctx: &RouterContext) -> bool {
        // 1. Host policy: capture daemons never retrieve.
        if let Some(ref host) = ctx.host_id {
            if self.skip_hosts.iter().any(|h| h == host) {
                return false;
            }
        }

        // 2. Working-memory hit: the answer is already available.
        if ctx.working_memory_hit {
            return false;
        }

        // 3. Command/code fragment hint from the caller.
        if ctx.is_command {
            return false;
        }

        // 4. Entropy heuristic: very short, low-entropy queries rarely need memory.
        if query_entropy(query) < 0.15 {
            return false;
        }

        // 5. Miss-rate gate: if >80% of recent retrievals were miss-signaled,
        //    the system is likely in a domain where retrieval doesn't help.
        if ctx.recent_miss_rate > self.miss_rate_threshold {
            return false;
        }

        true
    }
}

/// Lightweight entropy proxy: character-level bigram diversity normalised
/// by query length. Returns 0.0 for single-token queries, ~1.0 for rich prose.
///
/// Deliberately cheap — this runs synchronously before every retrieval.
fn query_entropy(query: &str) -> f32 {
    let query = query.trim();
    if query.is_empty() {
        return 0.0;
    }

    let tokens: Vec<&str> = query.split_whitespace().collect();
    let n = tokens.len();

    // Very short queries (1-2 words) are likely commands or entity lookups.
    if n <= 2 {
        // Still retrieve if it contains a question mark or uncertainty marker.
        let has_question = query.contains('?');
        let has_uncertainty = ["what", "who", "when", "where", "why", "how", "which"]
            .iter()
            .any(|w| query.to_lowercase().starts_with(w));
        return if has_question || has_uncertainty { 0.6 } else { 0.1 };
    }

    // For longer queries: proportion of unique tokens / total tokens as a
    // simple diversity proxy.
    let unique: std::collections::HashSet<_> = tokens.iter().collect();
    let diversity = unique.len() as f32 / n as f32;

    // Scale to [0.2, 1.0] — multi-word queries always get some retrieval signal.
    0.2 + 0.8 * diversity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_host_always_skips() {
        let router = MemoryRouter::default();
        let ctx = RouterContext {
            host_id: Some("capture".to_string()),
            ..Default::default()
        };
        assert!(!router.should_retrieve("what did I eat yesterday", &ctx));
    }

    #[test]
    fn working_memory_hit_skips() {
        let router = MemoryRouter::default();
        let ctx = RouterContext {
            working_memory_hit: true,
            ..Default::default()
        };
        assert!(!router.should_retrieve("who is Alice", &ctx));
    }

    #[test]
    fn command_hint_skips() {
        let router = MemoryRouter::default();
        let ctx = RouterContext {
            is_command: true,
            ..Default::default()
        };
        assert!(!router.should_retrieve("ls -la", &ctx));
    }

    #[test]
    fn rich_query_retrieves() {
        let router = MemoryRouter::default();
        let ctx = RouterContext::default();
        assert!(router.should_retrieve(
            "what were the main decisions we made about the API design last sprint",
            &ctx,
        ));
    }

    #[test]
    fn single_word_no_question_skips() {
        let router = MemoryRouter::default();
        let ctx = RouterContext::default();
        assert!(!router.should_retrieve("summarize", &ctx));
    }

    #[test]
    fn question_mark_overrides_short_query() {
        let router = MemoryRouter::default();
        let ctx = RouterContext::default();
        assert!(router.should_retrieve("what?", &ctx));
    }

    #[test]
    fn high_miss_rate_skips() {
        let router = MemoryRouter::default();
        let ctx = RouterContext {
            recent_miss_rate: 0.9,
            ..Default::default()
        };
        // Even a rich query is skipped when miss-rate is very high.
        assert!(!router.should_retrieve(
            "what were the main decisions we made about the API design last sprint",
            &ctx,
        ));
    }

    #[test]
    fn claude_code_host_retrieves() {
        let router = MemoryRouter::default();
        let ctx = RouterContext {
            host_id: Some("claude-code".to_string()),
            ..Default::default()
        };
        assert!(router.should_retrieve("what did we decide about the auth flow", &ctx));
    }
}
