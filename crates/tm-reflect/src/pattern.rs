//! Pattern detector — Sprint C, `docs/INTENT_SYSTEM.md` §5.
//!
//! Walks the user's `Completed` commitments, buckets them into
//! feature *cells*, and surfaces cells whose polarity distribution
//! diverges from the global baseline by enough to be worth showing.
//!
//! ## Why deterministic, not learned
//!
//! §5.2 is explicit: no prose, no causation, no psychometric
//! framing. The detector is a logbook with statistics, rendered with
//! a fixed template. That makes the whole module a pure function:
//! same inputs → same output → trivial to test, trivial to audit.
//!
//! ## v0 scope
//!
//! The full §5.1 cell key combines `(tag-set ∩, stakes, source-app
//! cluster, time-of-day band, context-topic centroid bucket)`. Our
//! v0 drops the topic-centroid bucket — that needs a 16-cluster
//! k-means over per-commitment topic embeddings, which depends on a
//! pipeline that doesn't yet land embeddings on commitments. We ship
//! the deterministic part now and leave a structured `cell_key`
//! enough that adding the centroid bucket later is purely additive.
//!
//! v0 cell key = `(stakes, time_band, primary_tag_or_none, source)`.
//!
//! ## Output
//!
//! `Vec<DetectedPattern>` — pure data, no DB writes. The brief
//! inlines them; the calibration panel (future) will persist them as
//! `Anticipation` rows with `AnticipationKind::PatternMatch` so the
//! detector can be scored by the user's accept/dismiss/silence
//! signal.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use tm_intent::{Commitment, Polarity, Source, Stakes};

/// Time-of-day band per §5.1 — five-AM-to-midday-evening style cuts
/// matched to how a user remembers their day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBand {
    Morning,   // 05:00 – 11:59
    Afternoon, // 12:00 – 16:59
    Evening,   // 17:00 – 21:59
    Night,     // 22:00 – 04:59
}

impl TimeBand {
    /// Classify a UTC instant into a band. Caller is responsible for
    /// converting to local time first if a per-user timezone is
    /// configured — UTC is the safe default and is what the rest of
    /// the system stores.
    pub fn classify(at: DateTime<Utc>) -> Self {
        match at.hour() {
            5..=11 => TimeBand::Morning,
            12..=16 => TimeBand::Afternoon,
            17..=21 => TimeBand::Evening,
            _ => TimeBand::Night,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TimeBand::Morning => "morning",
            TimeBand::Afternoon => "afternoon",
            TimeBand::Evening => "evening",
            TimeBand::Night => "night",
        }
    }
}

/// Cell key — the bucketing tuple the detector groups commitments
/// by. Hash-stable and serializable so callers can store / compare /
/// silence specific cells.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellKey {
    pub stakes: Stakes,
    pub time_band: TimeBand,
    /// Primary tag = first tag alphabetically when multiple — keeps
    /// cell-keys deterministic for a commitment that has ["api",
    /// "deploy"]. `None` when the commitment has no tags.
    pub primary_tag: Option<String>,
    pub source: Source,
}

impl CellKey {
    pub fn from_commitment(c: &Commitment) -> Self {
        let mut tags = c.tags.clone();
        tags.sort();
        Self {
            stakes: c.stakes,
            time_band: TimeBand::classify(c.made_at),
            primary_tag: tags.into_iter().next(),
            source: c.source,
        }
    }

    /// Human-readable cell label used in the brief render.
    pub fn label(&self) -> String {
        let stakes = format!("{:?}", self.stakes).to_lowercase();
        let source = format!("{:?}", self.source).to_lowercase();
        let band = self.time_band.as_str();
        match &self.primary_tag {
            Some(t) => format!("stakes={stakes}, {band}, tag={t}, source={source}"),
            None => format!("stakes={stakes}, {band}, source={source}"),
        }
    }

    /// Stable, content-addressed cell identity. 16 hex chars (~64
    /// bits) — collision-resistant for the few-hundred-cells scale a
    /// single user generates, short enough for the user to type into
    /// `tracemind patterns silence <hash>`.
    ///
    /// Hash domain is the canonical-JSON of the CellKey via serde,
    /// which already enforces the field order. Uses `DefaultHasher`
    /// (Rust stdlib SipHash) for a no-extra-deps build; collisions
    /// at this scale are not a security concern (the user can always
    /// unsilence by typing the next 16 hex). If we later need
    /// stronger guarantees we can swap in blake3 — the storage
    /// surface is just the hex string.
    pub fn cell_hash(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Stable serialization first (so a future field reordering
        // doesn't silently invalidate a user's silences).
        let canonical = serde_json::to_string(self).unwrap_or_default();
        let mut h = DefaultHasher::new();
        canonical.hash(&mut h);
        format!("{:016x}", h.finish())
    }
}

/// Polarity distribution within a cell, normalized so they sum to
/// 1.0 (modulo float error). `n` is the underlying sample count —
/// callers should treat the proportions as unreliable when `n` is
/// small (below the support gate).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct PolarityDist {
    pub better: f32,
    pub as_expected: f32,
    pub worse: f32,
    pub mixed: f32,
    pub no_outcome: f32,
}

impl PolarityDist {
    fn from_counts(counts: &PolarityCounts) -> Self {
        let n = counts.total().max(1) as f32; // avoid div-by-zero
        Self {
            better: counts.better as f32 / n,
            as_expected: counts.as_expected as f32 / n,
            worse: counts.worse as f32 / n,
            mixed: counts.mixed as f32 / n,
            no_outcome: counts.no_outcome as f32 / n,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PolarityCounts {
    better: usize,
    as_expected: usize,
    worse: usize,
    mixed: usize,
    no_outcome: usize,
}

impl PolarityCounts {
    fn add(&mut self, p: Polarity) {
        match p {
            Polarity::Better => self.better += 1,
            Polarity::AsExpected => self.as_expected += 1,
            Polarity::Worse => self.worse += 1,
            Polarity::Mixed => self.mixed += 1,
            Polarity::NoOutcome => self.no_outcome += 1,
        }
    }
    fn total(&self) -> usize {
        self.better + self.as_expected + self.worse + self.mixed + self.no_outcome
    }
}

/// Wilson lower bound at 95% CI for a binomial proportion `p_hat =
/// k/n`. Used as a "support strength" gate per §5.1.3 — we only
/// surface a pattern when the confidence interval doesn't straddle
/// the null effect direction.
///
/// Returns 0.0 when `n == 0`. Uses z = 1.96 (95% CI, two-sided).
///
/// Reference: Wilson, E.B. (1927). *Probable inference, the law of
/// succession, and statistical inference*. JASA 22(158): 209–212.
pub fn wilson_lower_bound(k: usize, n: usize) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let z: f32 = 1.96;
    let n = n as f32;
    let p = k as f32 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = p + z2 / (2.0 * n);
    let margin = z * ((p * (1.0 - p) / n) + (z2 / (4.0 * n * n))).sqrt();
    ((center - margin) / denom).max(0.0).min(1.0)
}

/// One detected pattern — a cell whose polarity distribution
/// diverges from the global baseline by enough to surface.
///
/// `lift_worse` is the diff `cell.p_worse - global.p_worse` — positive
/// means the cell is *worse than baseline*, the most common signal we
/// want to surface ("this kind of commitment goes sideways more than
/// usual"). We also compute `lift_better` for symmetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectedPattern {
    pub cell: CellKey,
    pub n: usize,
    pub dist: PolarityDist,
    pub global_dist: PolarityDist,
    pub lift_worse: f32,
    pub lift_better: f32,
    /// Wilson-LB on whichever direction the lift points — i.e. if
    /// `lift_worse > 0`, this is LB on `p_worse`; if `lift_better >
    /// 0`, LB on `p_better`. The gate then checks the LB stays on
    /// the same side of the global baseline as the point estimate.
    pub support_lb: f32,
    /// Pre-rendered, no-LLM template per §5.1.4. Caller can override.
    pub render: String,
}

/// Tunables. Defaults match §5.1.3 of the spec.
#[derive(Debug, Clone)]
pub struct PatternConfig {
    /// Minimum number of completed commitments in the cell.
    pub min_n: usize,
    /// `|lift|` floor (absolute proportion-difference).
    pub min_abs_lift: f32,
    /// Maximum patterns returned per detector run — keeps the brief
    /// from flooding when many cells qualify.
    pub max_patterns: usize,
    /// Minimum total Completed-with-polarity sample size; below this
    /// we don't report any pattern (the *global* baseline itself is
    /// untrustworthy).
    pub min_global_n: usize,
}

impl Default for PatternConfig {
    fn default() -> Self {
        Self {
            min_n: 6,
            min_abs_lift: 0.25,
            max_patterns: 5,
            min_global_n: 12,
        }
    }
}

/// Scan completed commitments + their polarity, return the surfaceable
/// patterns sorted by `|lift|` desc. Pure: no I/O, no clock reads.
pub fn detect_patterns(
    completed: &[(Commitment, Polarity)],
    cfg: &PatternConfig,
) -> Vec<DetectedPattern> {
    if completed.len() < cfg.min_global_n {
        return Vec::new();
    }

    // Global baseline.
    let mut global_counts = PolarityCounts::default();
    for (_, p) in completed {
        global_counts.add(*p);
    }
    let global = PolarityDist::from_counts(&global_counts);

    // Bucket.
    let mut cells: HashMap<CellKey, PolarityCounts> = HashMap::new();
    for (c, p) in completed {
        cells.entry(CellKey::from_commitment(c)).or_default().add(*p);
    }

    // Score each cell.
    let mut found: Vec<DetectedPattern> = cells
        .into_iter()
        .filter_map(|(cell, counts)| {
            let n = counts.total();
            if n < cfg.min_n {
                return None;
            }
            let dist = PolarityDist::from_counts(&counts);
            let lift_worse = dist.worse - global.worse;
            let lift_better = dist.better - global.better;
            let abs_lift = lift_worse.abs().max(lift_better.abs());
            if abs_lift < cfg.min_abs_lift {
                return None;
            }

            // Support gate: Wilson-LB on the dominant direction must
            // stay on the same side of the global baseline as the
            // point estimate. If lift_worse > 0, LB(worse) > global.worse.
            // Symmetric for lift_better.
            let (support_lb, support_ok) = if lift_worse.abs() >= lift_better.abs() {
                let lb = wilson_lower_bound(counts.worse, n);
                let ok = if lift_worse > 0.0 {
                    lb > global.worse
                } else {
                    // lift_worse < 0 means cell is *better than baseline*
                    // on the worse-axis. Use Wilson upper bound by
                    // symmetry: 1 - LB(n-k, n). But we already capture
                    // this as lift_better when relevant — skip.
                    false
                };
                (lb, ok)
            } else {
                let lb = wilson_lower_bound(counts.better, n);
                let ok = if lift_better > 0.0 {
                    lb > global.better
                } else {
                    false
                };
                (lb, ok)
            };
            if !support_ok {
                return None;
            }

            let render = render_pattern(&cell, n, &dist, &global, lift_worse, lift_better);

            Some(DetectedPattern {
                cell,
                n,
                dist,
                global_dist: global,
                lift_worse,
                lift_better,
                support_lb,
                render,
            })
        })
        .collect();

    // Sort by |lift| desc, then by n desc as a tiebreak for stability.
    found.sort_by(|a, b| {
        let la = a.lift_worse.abs().max(a.lift_better.abs());
        let lb = b.lift_worse.abs().max(b.lift_better.abs());
        lb.partial_cmp(&la)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.n.cmp(&a.n))
    });
    found.truncate(cfg.max_patterns);
    found
}

/// Render the §5.1.4 template. Picked deterministically — no LLM.
/// The dominant lift direction wins the headline; we never claim
/// causation.
fn render_pattern(
    cell: &CellKey,
    n: usize,
    dist: &PolarityDist,
    global: &PolarityDist,
    lift_worse: f32,
    lift_better: f32,
) -> String {
    let pct = |x: f32| (x * 100.0).round() as u32;
    if lift_worse.abs() >= lift_better.abs() && lift_worse > 0.0 {
        format!(
            "in last {} commitments ({}), {}% had worse-than-expected outcomes — vs {}% overall",
            n,
            cell.label(),
            pct(dist.worse),
            pct(global.worse),
        )
    } else if lift_better > 0.0 {
        format!(
            "in last {} commitments ({}), {}% had better-than-expected outcomes — vs {}% overall",
            n,
            cell.label(),
            pct(dist.better),
            pct(global.better),
        )
    } else {
        // lift_worse < 0 — cell is unusually *good*; phrase as such
        format!(
            "in last {} commitments ({}), only {}% had worse-than-expected outcomes — vs {}% overall",
            n,
            cell.label(),
            pct(dist.worse),
            pct(global.worse),
        )
    }
}

/// Sunday convenience helper for callers running nightly: returns
/// the typical 12-month window-start the spec calls for.
pub fn default_window_start(now: DateTime<Utc>) -> DateTime<Utc> {
    // Subtract ~365 days. Avoid `Months::new(12)` to keep this
    // dependency-free; clock-sloppy is fine for a lookback window.
    now - chrono::Duration::days(365)
}

// Quiet a clippy lint we hit when day_of_week is unused — keeps the
// import live for the future centroid-bucket addition.
#[allow(dead_code)]
fn _keep_datelike_imported(d: DateTime<Utc>) -> u32 {
    d.weekday().num_days_from_monday()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tm_intent::{CommitmentKind, Source, Stakes};

    fn mk(stakes: Stakes, hour: u32, tag: Option<&str>, source: Source) -> Commitment {
        let mut c = Commitment::new(CommitmentKind::Intent, "x", source);
        c.stakes = stakes;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 1, hour, 0, 0).unwrap();
        if let Some(t) = tag {
            c.tags = vec![t.to_string()];
        }
        c
    }

    fn dataset_worse_morning_high() -> Vec<(Commitment, Polarity)> {
        // 8 high-stakes morning commitments, 6 went worse — vs a
        // global baseline of 18 commitments where worse rate is much
        // lower.
        let mut v = Vec::new();
        for i in 0..6 {
            v.push((
                mk(Stakes::High, 9, Some("deploy"), Source::Manual),
                Polarity::Worse,
            ));
            let _ = i;
        }
        for _ in 0..2 {
            v.push((
                mk(Stakes::High, 9, Some("deploy"), Source::Manual),
                Polarity::AsExpected,
            ));
        }
        // Other cells: mostly good, drives the global baseline.
        for _ in 0..10 {
            v.push((
                mk(Stakes::Medium, 14, Some("research"), Source::Manual),
                Polarity::AsExpected,
            ));
        }
        for _ in 0..2 {
            v.push((
                mk(Stakes::Medium, 14, Some("research"), Source::Manual),
                Polarity::Better,
            ));
        }
        v
    }

    #[test]
    fn time_band_boundaries() {
        let at = |h| Utc.with_ymd_and_hms(2026, 4, 1, h, 0, 0).unwrap();
        assert_eq!(TimeBand::classify(at(5)), TimeBand::Morning);
        assert_eq!(TimeBand::classify(at(11)), TimeBand::Morning);
        assert_eq!(TimeBand::classify(at(12)), TimeBand::Afternoon);
        assert_eq!(TimeBand::classify(at(16)), TimeBand::Afternoon);
        assert_eq!(TimeBand::classify(at(17)), TimeBand::Evening);
        assert_eq!(TimeBand::classify(at(21)), TimeBand::Evening);
        assert_eq!(TimeBand::classify(at(22)), TimeBand::Night);
        assert_eq!(TimeBand::classify(at(4)), TimeBand::Night);
        assert_eq!(TimeBand::classify(at(0)), TimeBand::Night);
    }

    #[test]
    fn wilson_lb_known_values() {
        // n=10, k=8 => p_hat=0.8, LB ≈ 0.49
        let lb = wilson_lower_bound(8, 10);
        assert!(lb > 0.45 && lb < 0.55, "lb = {lb}");
        // n=0 — guard
        assert_eq!(wilson_lower_bound(0, 0), 0.0);
        // n=k — LB strictly < 1
        let lb_full = wilson_lower_bound(10, 10);
        assert!(lb_full < 1.0 && lb_full > 0.5);
    }

    #[test]
    fn empty_input_yields_no_patterns() {
        let r = detect_patterns(&[], &PatternConfig::default());
        assert!(r.is_empty());
    }

    #[test]
    fn below_global_threshold_yields_no_patterns() {
        let v = vec![
            (
                mk(Stakes::High, 9, Some("deploy"), Source::Manual),
                Polarity::Worse,
            );
            5
        ];
        let r = detect_patterns(&v, &PatternConfig::default());
        assert!(r.is_empty(), "global n=5 < min_global_n=12, so empty");
    }

    #[test]
    fn surfaces_worse_pattern_with_strong_lift() {
        let data = dataset_worse_morning_high();
        let r = detect_patterns(&data, &PatternConfig::default());
        assert!(!r.is_empty(), "expected at least one pattern");
        let p = &r[0];
        assert_eq!(p.cell.stakes, Stakes::High);
        assert_eq!(p.cell.time_band, TimeBand::Morning);
        assert!(p.lift_worse > 0.25, "lift_worse = {}", p.lift_worse);
        assert!(p.render.contains("worse"));
        assert!(p.render.contains("vs"));
    }

    #[test]
    fn small_cell_n_filtered() {
        // Same dataset but bump min_n above the cell size.
        let data = dataset_worse_morning_high();
        let cfg = PatternConfig {
            min_n: 100,
            ..Default::default()
        };
        let r = detect_patterns(&data, &cfg);
        assert!(r.is_empty());
    }

    #[test]
    fn weak_lift_filtered() {
        // Build a dataset with no real cell deviation.
        let mut v = Vec::new();
        for _ in 0..20 {
            v.push((
                mk(Stakes::Medium, 14, Some("research"), Source::Manual),
                Polarity::AsExpected,
            ));
        }
        let r = detect_patterns(&v, &PatternConfig::default());
        assert!(r.is_empty());
    }

    #[test]
    fn cell_hash_is_stable_and_distinct_for_distinct_cells() {
        let a = CellKey {
            stakes: Stakes::High,
            time_band: TimeBand::Morning,
            primary_tag: Some("deploy".into()),
            source: Source::Manual,
        };
        let b = a.clone();
        assert_eq!(a.cell_hash(), b.cell_hash(), "deterministic");
        assert_eq!(a.cell_hash().len(), 16);

        let c = CellKey {
            time_band: TimeBand::Afternoon, // changed
            ..a.clone()
        };
        assert_ne!(a.cell_hash(), c.cell_hash());
    }

    #[test]
    fn round_trips_json() {
        let data = dataset_worse_morning_high();
        let r = detect_patterns(&data, &PatternConfig::default());
        assert!(!r.is_empty());
        let s = serde_json::to_string(&r[0]).unwrap();
        let back: DetectedPattern = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r[0]);
    }

    #[test]
    fn results_capped_at_max_patterns() {
        // Build several distinct cells, each with strong lift_worse.
        let mut v: Vec<(Commitment, Polarity)> = Vec::new();
        // 4 cells of 6 commitments each, 5/6 worse.
        for tag in ["a", "b", "c", "d"] {
            for _ in 0..5 {
                v.push((
                    mk(Stakes::High, 9, Some(tag), Source::Manual),
                    Polarity::Worse,
                ));
            }
            v.push((
                mk(Stakes::High, 9, Some(tag), Source::Manual),
                Polarity::AsExpected,
            ));
        }
        // Add lots of "as_expected" elsewhere so the global baseline
        // is dominated by good outcomes.
        for _ in 0..30 {
            v.push((
                mk(Stakes::Medium, 14, Some("z"), Source::Manual),
                Polarity::AsExpected,
            ));
        }
        let cfg = PatternConfig {
            max_patterns: 2,
            ..Default::default()
        };
        let r = detect_patterns(&v, &cfg);
        assert!(r.len() <= 2);
    }
}
