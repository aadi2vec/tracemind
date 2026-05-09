//! L1 prefetch orchestrator — bridge between `tm-intent` Anticipations
//! and the `tm-retrieval` prefetch cache.
//!
//! `tm-intent::IntentStore` knows *what* the world model expects the user
//! to ask. `tm-retrieval::RetrievalEngine` knows *how* to warm a query.
//! Neither crate depends on the other (by design — see
//! `tm-retrieval::prefetch` module docs). This module is the seam: it
//! pulls active `AnticipationKind::PrefetchQuery` entries and hands the
//! query strings to the engine.
//!
//! Calling `warm_l1` is idempotent: `PrefetchCache::prime` overwrites
//! existing entries in place. The intended call site is right before a
//! retrieval-heavy interaction begins (brief generation, hotkey trigger,
//! agent turn) — exactly the trigger surface called out in
//! `docs/INTENT_SYSTEM.md` §6.1.
//!
//! This module is pure orchestration — no async, no scheduling. The
//! caller decides cadence.
//!
//! ## Failure semantics
//!
//! Per-anticipation embedder/graph failures are *non-fatal*. We log
//! them via the returned `WarmReport` and keep going so a single bad
//! prediction doesn't block the rest of the warm pass.

use chrono::{DateTime, Utc};
use tm_intent::IntentStore;
use tm_retrieval::RetrievalEngine;

/// Outcome of one `warm_l1` pass. Inspected by the caller for
/// telemetry / logging.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WarmReport {
    /// How many anticipations the store returned to us as eligible.
    pub considered: usize,
    /// How many we successfully primed.
    pub warmed: usize,
    /// How many failed (per-anticipation embedder/graph errors).
    pub failed: usize,
    /// First few error strings for diagnostic surfaces (capped at 8).
    pub errors: Vec<String>,
}

const MAX_RECORDED_ERRORS: usize = 8;

/// Fetch up to `max_per_run` active prefetch anticipations from
/// `store`, warm each one in `engine`, and return a `WarmReport`.
///
/// Errors talking to the *store* itself (e.g. SQLite I/O) bubble up.
/// Errors warming an individual query are recorded in the report and
/// don't abort the pass.
pub fn warm_l1(
    engine: &mut RetrievalEngine,
    store: &IntentStore,
    now: DateTime<Utc>,
    max_per_run: usize,
) -> Result<WarmReport, tm_intent::store::StoreError> {
    if max_per_run == 0 {
        return Ok(WarmReport::default());
    }
    let queries = store.list_active_prefetch_queries(now, max_per_run)?;
    let mut report = WarmReport {
        considered: queries.len(),
        ..Default::default()
    };
    for (_id, q) in queries {
        match engine.prime_prefetch(&q) {
            Ok(()) => report.warmed += 1,
            Err(e) => {
                report.failed += 1;
                if report.errors.len() < MAX_RECORDED_ERRORS {
                    report.errors.push(format!("{q:?}: {e}"));
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tm_intent::types::{
        Anticipation, AnticipationKind, CommitmentDraft, CommitmentKind, Stakes, TriggerContext,
    };
    use tm_retrieval::RetrievalEngine;
    use uuid::Uuid;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tm_pf_orch_{tag}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn fresh_engine(dir: &std::path::Path) -> RetrievalEngine {
        let db = dir.join("memory.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
        // hash_embed=true so tests don't need ONNX models.
        RetrievalEngine::open(&db, &traces, true).expect("engine")
    }

    fn mk_prefetch(
        statement: &str,
        confidence: f32,
        expires_at: DateTime<Utc>,
    ) -> Anticipation {
        Anticipation {
            id: Uuid::new_v4(),
            generated_at: Utc::now(),
            trigger: TriggerContext {
                working_memory_hash: "wm-test".into(),
                topic_centroid: vec![],
                time_of_day: 9,
                source_app: None,
                matched_phrase: None,
            },
            kind: AnticipationKind::PrefetchQuery,
            predicted_commitment: Some(CommitmentDraft {
                kind: CommitmentKind::Intent,
                statement: statement.into(),
                options_considered: vec![],
                horizon: None,
                stakes: Stakes::Medium,
                tags: vec![],
            }),
            grounded_in: vec![],
            confidence,
            surfaced_at: None,
            user_response: None,
            eventual_match: None,
            expires_at,
        }
    }

    #[test]
    fn warm_l1_primes_each_active_anticipation() {
        let dir = temp_dir("ok");
        let mut engine = fresh_engine(&dir);
        let store = IntentStore::open_in_memory().unwrap();
        let now = Utc::now();
        let later = now + Duration::minutes(10);

        store.insert_anticipation(&mk_prefetch("q one", 0.9, later)).unwrap();
        store.insert_anticipation(&mk_prefetch("q two", 0.6, later)).unwrap();

        let report = warm_l1(&mut engine, &store, now, 16).unwrap();
        assert_eq!(report.considered, 2);
        assert_eq!(report.warmed, 2);
        assert_eq!(report.failed, 0);
        assert!(report.errors.is_empty());
        assert_eq!(engine.prefetch_stats().primes, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_l1_with_zero_budget_short_circuits() {
        let dir = temp_dir("zero");
        let mut engine = fresh_engine(&dir);
        let store = IntentStore::open_in_memory().unwrap();
        store
            .insert_anticipation(&mk_prefetch("ignored", 0.99, Utc::now() + Duration::minutes(5)))
            .unwrap();
        let report = warm_l1(&mut engine, &store, Utc::now(), 0).unwrap();
        assert_eq!(report, WarmReport::default());
        assert_eq!(engine.prefetch_stats().primes, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_l1_skips_expired_and_responded() {
        let dir = temp_dir("filter");
        let mut engine = fresh_engine(&dir);
        let store = IntentStore::open_in_memory().unwrap();
        let now = Utc::now();
        let later = now + Duration::minutes(10);
        let earlier = now - Duration::minutes(5);

        // expired
        store.insert_anticipation(&mk_prefetch("dead", 0.99, earlier)).unwrap();
        // responded
        let mut responded = mk_prefetch("dismissed", 0.99, later);
        responded.user_response = Some(tm_intent::types::UserResponse::Dismissed);
        store.insert_anticipation(&responded).unwrap();
        // live
        store.insert_anticipation(&mk_prefetch("live", 0.5, later)).unwrap();

        let report = warm_l1(&mut engine, &store, now, 16).unwrap();
        assert_eq!(report.considered, 1);
        assert_eq!(report.warmed, 1);
        assert_eq!(engine.prefetch_stats().primes, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_l1_caps_at_max_per_run() {
        let dir = temp_dir("cap");
        let mut engine = fresh_engine(&dir);
        let store = IntentStore::open_in_memory().unwrap();
        let now = Utc::now();
        let later = now + Duration::minutes(10);
        for i in 0..5 {
            store
                .insert_anticipation(&mk_prefetch(
                    &format!("q{i}"),
                    0.1 + (i as f32) * 0.1,
                    later,
                ))
                .unwrap();
        }

        let report = warm_l1(&mut engine, &store, now, 2).unwrap();
        assert_eq!(report.considered, 2);
        assert_eq!(report.warmed, 2);
        assert_eq!(engine.prefetch_stats().primes, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_l1_is_idempotent_on_repeat_runs() {
        let dir = temp_dir("idem");
        let mut engine = fresh_engine(&dir);
        let store = IntentStore::open_in_memory().unwrap();
        let now = Utc::now();
        let later = now + Duration::minutes(10);
        store.insert_anticipation(&mk_prefetch("repeat", 0.7, later)).unwrap();

        let r1 = warm_l1(&mut engine, &store, now, 8).unwrap();
        let r2 = warm_l1(&mut engine, &store, now, 8).unwrap();
        assert_eq!(r1.warmed, 1);
        assert_eq!(r2.warmed, 1);
        // Cache stays at one *live* entry — repeated primes overwrite in place.
        // The PrefetchCache stats count both primes (lifetime metric).
        assert_eq!(engine.prefetch_stats().primes, 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
